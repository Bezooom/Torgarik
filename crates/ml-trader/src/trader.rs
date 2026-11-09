//! Main trading loop: majors_alpha (default) or funding scanner.
//! Writes `runtime/state.json` each cycle.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use serde_json::json;
use tokio::time::{interval, Duration};
use tracing::{info, warn};

use trading_common::{ConfigStore, RunMode};
use trading_exchange::{ExchangeHandle, FundingOpp, HyperliquidClient};
use trading_strategies::{check_order, ConvictionPenalty, FundingScannerStrategy, Strategy};

/// Copilot execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopilotMode {
    /// Pure advisory — log suggestions, never place orders.
    Advisory,
    /// Auto — execute orders (subject to kill-switch / risk gates).
    Auto,
}

/// A single advisory signal from the copilot.
#[derive(Debug, Clone)]
pub struct AdvisorySignal {
    pub symbol: String,
    pub conviction: f64,
    /// Decayed edge strength after ConvictionPenalty.
    pub decayed_strength: f64,
    /// Fraction of strength lost to staleness (0.0 = fresh).
    pub penalty_for_flat: f64,
    pub recommendation: &'static str,
    pub is_stale: bool,
}

/// Venue-agnostic trader.
pub struct Trader {
    config: ConfigStore,
    exchange: ExchangeHandle,
    hl_scanner: Option<Arc<HyperliquidClient>>,
    strategy: Box<dyn Strategy>,
    /// Optional funding-scanner path (legacy B).
    scanner: Option<FundingScannerStrategy>,
    /// When true, work list = allowlist (majors_alpha), not funding hot set.
    use_allowlist_loop: bool,
}

impl Trader {
    pub fn new(
        config: ConfigStore,
        exchange: ExchangeHandle,
        strategy: Box<dyn Strategy>,
    ) -> Self {
        let name = strategy.name().to_string();
        let use_allowlist_loop = name == "majors_alpha" || name == "trend_mid";
        Self {
            config,
            exchange,
            hl_scanner: None,
            strategy,
            scanner: None,
            use_allowlist_loop,
        }
    }

    pub fn with_scanner(
        mut self,
        hl: Arc<HyperliquidClient>,
        scanner: FundingScannerStrategy,
    ) -> Self {
        self.hl_scanner = Some(hl);
        self.scanner = Some(scanner);
        self.use_allowlist_loop = false;
        self
    }

    pub fn with_hl_quotes(mut self, hl: Arc<HyperliquidClient>) -> Self {
        self.hl_scanner = Some(hl);
        self
    }

    /// Advisory copilot: evaluate a symbol with conviction penalty, no order flow.
    ///
    /// Returns an `AdvisorySignal` summarizing the effective edge after decay.
    /// In `Advisory` mode this is purely informational; in `Auto` mode the
    /// caller can gate execution on `conviction > threshold`.
    pub fn evaluate(
        &self,
        symbol: &str,
        raw_edge: f64,
        bars_since_signal: u64,
        mode: CopilotMode,
    ) -> AdvisorySignal {
        let cp = ConvictionPenalty {
            full_strength: raw_edge.max(0.0),
            bars_since_signal,
            ..ConvictionPenalty::default()
        };
        let decayed = cp.decayed_strength();
        let stale = cp.is_stale();
        let rec = if stale || decayed < 0.1 {
            "hold"
        } else if decayed > 0.5 {
            "strong_entry"
        } else {
            "weak_entry"
        };

        if mode == CopilotMode::Advisory {
            tracing::info!(
                "ADVISORY {:?} conv={:.3} decayed={:.3} stale={} rec={}",
                symbol, raw_edge, decayed, stale, rec
            );
        }

        AdvisorySignal {
            symbol: symbol.to_string(),
            conviction: raw_edge,
            decayed_strength: decayed,
            penalty_for_flat: cp.penalty_for_flat(),
            recommendation: rec,
            is_stale: stale,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let mode = self.config.settings.mode;
        let secs = self.config.settings.strategy.loop_interval_secs.max(5);
        let mut symbols = self.config.settings.exchange.symbol_whitelist.clone();

        info!(
            "Trader started mode={:?} venue={} strategy={} symbols_n={} allowlist_loop={}",
            mode,
            self.exchange.venue(),
            self.strategy.name(),
            symbols.len(),
            self.use_allowlist_loop
        );

        if matches!(mode, RunMode::Live) {
            warn!("LIVE MODE — real funds at risk");
        } else {
            info!("Paper/dry-run: real HL quotes, simulated fills");
        }

        let mut ticker = interval(Duration::from_secs(secs));
        let mut cycle: u64 = 0;

        loop {
            ticker.tick().await;
            cycle += 1;

            // Optional refresh (funding scanner universe)
            if let (Some(hl), Some(scan)) = (&self.hl_scanner, &mut self.scanner) {
                if let Err(e) = hl.refresh_market_data().await {
                    warn!("HL refresh failed: {e}");
                } else {
                    let enter = self.config.settings.strategy.funding_enter;
                    match hl.rank_by_funding(enter * 0.25, 0.0).await {
                        Ok(opps) => {
                            let raw: Vec<(String, f64, f64, f64)> = opps
                                .iter()
                                .map(|o| (o.coin.clone(), o.funding, o.mid, o.open_interest))
                                .collect();
                            scan.update_from_opps(&raw);
                            if cycle % 4 == 1 {
                                log_top_funding(
                                    &opps
                                        .iter()
                                        .filter(|o| {
                                            scan.filter.allows(
                                                &o.coin,
                                                o.mid,
                                                o.open_interest,
                                                o.funding,
                                            )
                                        })
                                        .cloned()
                                        .collect::<Vec<_>>(),
                                );
                            }
                            if !self.use_allowlist_loop {
                                let mut set = scan.hot_symbols();
                                if set.is_empty() {
                                    set = self.config.settings.exchange.symbol_whitelist.clone();
                                }
                                symbols = set;
                            }
                        }
                        Err(e) => warn!("rank_by_funding: {e}"),
                    }
                }
            } else if let Some(hl) = &self.hl_scanner {
                let _ = hl.refresh_market_data().await;
            }

            if self.use_allowlist_loop {
                symbols = self.config.settings.exchange.symbol_whitelist.clone();
            }

            let mut portfolio = match self.exchange.portfolio().await {
                Ok(p) => p,
                Err(e) => {
                    warn!("portfolio: {e}");
                    continue;
                }
            };

            let day_start = (portfolio.equity - portfolio.daily_pnl).max(1e-9);
            if portfolio.daily_pnl < 0.0
                && (-portfolio.daily_pnl) / day_start
                    >= self.config.settings.risk.max_daily_loss
            {
                warn!(
                    "KILL-SWITCH max_daily_loss daily_pnl={:.2} equity={:.2}",
                    portfolio.daily_pnl, portfolio.equity
                );
                write_state(&portfolio, &symbols, cycle, "kill_switch", self.strategy.name());
                continue;
            }

            let mut work_list = symbols.clone();
            for p in &portfolio.positions {
                if !work_list.contains(&p.instrument_id) {
                    work_list.push(p.instrument_id.clone());
                }
            }

            for sym in &work_list {
                if let Ok(p) = self.exchange.portfolio().await {
                    portfolio = p;
                }

                let quote = match self.exchange.get_quote(sym).await {
                    Ok(q) => q,
                    Err(e) => {
                        warn!("quote {sym}: {e}");
                        continue;
                    }
                };

                let req = if let Some(scan) = &mut self.scanner {
                    scan.on_quote(&quote, &portfolio)
                } else {
                    self.strategy.on_quote(&quote, &portfolio)
                };

                let Some(req) = req else {
                    continue;
                };

                if let Err(e) =
                    check_order(&req, &portfolio, &self.config.settings.risk, quote.mid)
                {
                    warn!("risk reject {sym}: {e}");
                    continue;
                }

                match self.exchange.place_order(&req).await {
                    Ok(ack) => info!(
                        "ORDER {:?} {} {} status={:?} @ {:?} — {}",
                        req.side,
                        req.size,
                        sym,
                        ack.status,
                        ack.avg_price,
                        req.reasoning
                    ),
                    Err(e) => warn!("place_order {sym}: {e}"),
                }
            }

            if let Ok(p) = self.exchange.portfolio().await {
                info!(
                    "equity={:.2} avail={:.2} daily_pnl={:.2} pos={}",
                    p.equity,
                    p.available,
                    p.daily_pnl,
                    p.positions.len()
                );
                for pos in &p.positions {
                    info!(
                        "  pos {} {:?} size={:.6} entry={:.4} upnl={:.2}",
                        pos.instrument_id, pos.side, pos.size, pos.entry_price, pos.unrealized_pnl
                    );
                }
                write_state(&p, &work_list, cycle, "ok", self.strategy.name());
            }
        }
    }
}

fn log_top_funding(opps: &[FundingOpp]) {
    info!("=== filtered funding snapshot ===");
    for o in opps.iter().take(8) {
        info!(
            "  {:12} funding={:+.6e} mid={:.4} oi={:.0}",
            o.coin, o.funding, o.mid, o.open_interest
        );
    }
}

fn write_state(
    portfolio: &trading_common::PortfolioSnapshot,
    symbols: &[String],
    cycle: u64,
    status: &str,
    strategy: &str,
) {
    let path = Path::new("runtime/state.json");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let body = json!({
        "cycle": cycle,
        "status": status,
        "strategy": strategy,
        "ts": portfolio.ts.to_rfc3339(),
        "venue": format!("{}", portfolio.venue),
        "equity": portfolio.equity,
        "available": portfolio.available,
        "daily_pnl": portfolio.daily_pnl,
        "positions": portfolio.positions.iter().map(|p| json!({
            "id": p.instrument_id,
            "side": format!("{:?}", p.side),
            "size": p.size,
            "entry": p.entry_price,
            "upnl": p.unrealized_pnl,
        })).collect::<Vec<_>>(),
        "watch": symbols,
    });
    let _ = fs::write(path, serde_json::to_string_pretty(&body).unwrap_or_default());
}

pub fn default_strategy(cfg: &ConfigStore) -> Box<dyn Strategy> {
    let s = &cfg.settings.strategy;
    let allow = &cfg.settings.exchange.symbol_whitelist;
    let max_pos = cfg.settings.risk.max_position_per_instrument;
    let stop = cfg.settings.risk.stop_loss;
    match s.name.as_str() {
        "funding_carry" | "funding_scanner" => {
            Box::new(trading_strategies::FundingCarryStrategy::from_settings(
                s, max_pos, stop,
            ))
        }
        "majors_alpha" | "trend_mid" | _ => Box::new(
            trading_strategies::MajorsAlphaStrategy::from_settings(s, allow, max_pos, stop),
        ),
    }
}

pub type MlTrader = Trader;
