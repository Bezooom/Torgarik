//! Risk / journal shell – no auto alpha.
//!
//! - Polls HL quotes + portfolio
//! - Enforces daily loss / exposure (advisory + hard block on external intents)
//! - OOS kill-switch: **hard-blocks live execution**; paper/shell advisory always allowed
//! - Reads optional `runtime/signals.jsonl` for human / external signals
//! - Writes versioned journal under `runtime/journal/`

use std::fs::{create_dir_all, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use trading_common::{
    ConfigStore, InstrumentRef, MarketType, OosStats, OrderRequest, OrderSide, RunMode,
    TimeInForce, VenueKind,
};
use trading_exchange::{build_exchange_pair, Exchange, ExchangeHandle, HyperliquidClient};
use trading_strategies::check_order;
use trading_strategies::protections::{ProtectionStore, PROTECTIONS_FILE};

use crate::bracket::{Bracket, BracketParams, BracketStore, BRACKETS_FILE};
use crate::journal::{Journal, JournalEvent};

#[derive(Debug, Deserialize)]
struct ExternalSignalLine {
    /// coin e.g. ETH
    coin: String,
    /// buy | sell | flat
    side: String,
    /// size in coin units (optional if flat)
    #[serde(default)]
    size: Option<f64>,
    #[serde(default)]
    note: String,
    #[serde(default)]
    source: Option<String>,
    // ── Optional local bracket params (SL/TP), fractions like 0.02 = 2% ──
    #[serde(default)]
    sl_pct: Option<f64>,
    #[serde(default)]
    tp_pct: Option<f64>,
    // ── Or absolute prices (take precedence over pcts when both given) ──
    #[serde(default)]
    sl_price: Option<f64>,
    #[serde(default)]
    tp_price: Option<f64>,
}

/// Pure policy: should live order placement be forced to dry-run?
///
/// - Non-live modes → always dry
/// - Live without `--execute` → dry
/// - Live + execute + OOS blocking → dry (hard lock)
/// - Live + execute + OOS green / missing → live
pub fn force_dry_run(mode: RunMode, execute: bool, oos_blocking: bool) -> bool {
    if !matches!(mode, RunMode::Live) {
        return true;
    }
    if !execute {
        return true;
    }
    if oos_blocking {
        return true;
    }
    false
}

#[derive(Debug, Clone)]
struct ProcessedSignal {
    coin: String,
    side: String,
    size: Option<f64>,
    ts: DateTime<Utc>,
}

/// Positions the shell created itself while `dry` is true.
///
/// In dry mode nothing is filled at the venue, so the paper book stays empty
/// and a registered bracket would have no position to monitor: every SL/TP
/// rehearsal would be silently dropped. This tiny book closes that gap so the
/// dry path exercises exactly the same bracket and protection code as paper.
#[derive(Debug, Default)]
struct DryBook {
    positions: std::collections::HashMap<String, trading_common::Position>,
}

impl DryBook {
    fn apply(&mut self, req: &OrderRequest, fill_px: f64) {
        use trading_common::Position;
        if req.reduce_only {
            self.positions.remove(&req.instrument.id);
            return;
        }
        let id = req.instrument.id.clone();
        self.positions.insert(
            id.clone(),
            Position {
                instrument_id: id,
                side: req.side,
                size: req.size,
                entry_price: fill_px,
                unrealized_pnl: 0.0,
                timestamp: Utc::now(),
                market_id: None,
                outcome_id: None,
            },
        );
    }

    fn close(&mut self, coin: &str) {
        self.positions.remove(coin);
    }

    fn list(&self) -> Vec<trading_common::Position> {
        self.positions.values().cloned().collect()
    }

    fn position_for(&self, coin: &str) -> Option<&trading_common::Position> {
        self.positions.get(coin)
    }
}

pub struct RiskShell {
    config: ConfigStore,
    exchange: ExchangeHandle,
    hl: Arc<HyperliquidClient>,
    journal: Journal,
    signals_path: PathBuf,
    signal_offset: u64,
    execute: bool,
    /// Cached OOS blocking flag (re-read each cycle).
    oos_blocking: bool,
    processed_signals: Vec<ProcessedSignal>,
    /// Locally managed SL/TP brackets, persisted to runtime/brackets.json.
    brackets: BracketStore,
    /// Stop-loss guard / cooldown / drawdown circuit breaker. Persisted only
    /// for live execution; dry and paper sessions keep their history in memory
    /// so practice runs never poison the live guard.
    protections: ProtectionStore,
    /// One-shot: brackets are reconciled against the first portfolio snapshot
    /// so a bracket cannot outlive the position it was protecting.
    brackets_reconciled: bool,
    /// Positions the shell filled itself in dry mode (see [`DryBook`]).
    dry_book: DryBook,
    /// Last observed state of `runtime/stop.flag` (remote kill switch).
    stop_flag_active: bool,
}

impl RiskShell {
    pub fn new(
        config: ConfigStore,
        exchange: ExchangeHandle,
        hl: Arc<HyperliquidClient>,
        journal: Journal,
        execute: bool,
    ) -> Self {
        let signals_path = PathBuf::from("runtime/signals.jsonl");
        if let Some(p) = signals_path.parent() {
            let _ = create_dir_all(p);
        }
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&signals_path);
        // Only process signals appended *after* shell start (skip historical tail).
        let signal_offset = std::fs::metadata(&signals_path)
            .map(|m| m.len())
            .unwrap_or(0);
        // Brackets survive restarts; corrupt file → empty store (fail-soft).
        let brackets = BracketStore::load(PathBuf::from(BRACKETS_FILE));
        if !brackets.list().is_empty() {
            info!(
                "brackets restored from {}: {} active",
                BRACKETS_FILE,
                brackets.list().len()
            );
        }
        let protections = if matches!(config.settings.mode, RunMode::Live) && execute {
            ProtectionStore::load(PathBuf::from(PROTECTIONS_FILE))
        } else {
            ProtectionStore::in_memory()
        };
        if !protections.state().stops.is_empty() {
            info!(
                "protections restored: {} stops, peak_equity={:.2}",
                protections.state().stops.len(),
                protections.state().peak_equity
            );
        }

        Self {
            config,
            exchange,
            hl,
            journal,
            signals_path,
            signal_offset,
            execute,
            oos_blocking: false,
            processed_signals: Vec::new(),
            brackets,
            protections,
            brackets_reconciled: false,
            dry_book: DryBook::default(),
            stop_flag_active: false,
        }
    }

    fn refresh_oos(&mut self) {
        self.oos_blocking = OosStats::load()
            .map(|s| s.is_blocking_with_settings(&self.config.settings.oos))
            .unwrap_or(false);
    }

    fn log_oos_status(&self, note: &str) {
        match OosStats::load() {
            Some(s) => {
                let blocking = s.is_blocking_with_settings(&self.config.settings.oos);
                self.journal.write(&JournalEvent::OosStatus {
                    ts: Utc::now(),
                    avg_sharpe: s.avg_sharpe,
                    avg_pnl: s.avg_pnl,
                    n_folds: s.n_folds,
                    profitable_folds: s.profitable_folds,
                    blocking,
                    note: note.into(),
                });
                if blocking {
                    warn!(
                        "OOS kill-switch ACTIVE sharpe={:.3} pnl={:.2} folds={}/{} – live execute locked",
                        s.avg_sharpe, s.avg_pnl, s.profitable_folds, s.n_folds
                    );
                } else {
                    info!(
                        "OOS ok sharpe={:.3} pnl={:.2} folds={}",
                        s.avg_sharpe, s.avg_pnl, s.n_folds
                    );
                }
            }
            None => {
                self.journal.write(&JournalEvent::OosStatus {
                    ts: Utc::now(),
                    avg_sharpe: 0.0,
                    avg_pnl: 0.0,
                    n_folds: 0,
                    profitable_folds: 0,
                    blocking: false,
                    note: format!("{note}; no wf_latest.json (advisory only)"),
                });
                warn!("OOS report missing – live still gated by Settings::validate");
            }
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let secs = self.config.settings.strategy.loop_interval_secs.max(10);
        let symbols = self.config.settings.exchange.symbol_whitelist.clone();
        let mode = self.config.settings.mode;

        self.refresh_oos();
        let dry = force_dry_run(mode, self.execute, self.oos_blocking);

        info!(
            "RISK SHELL started dry={} execute={} oos_blocking={} symbols={:?} journal={}",
            dry,
            self.execute,
            self.oos_blocking,
            symbols,
            self.journal.path().display()
        );
        info!(
            "External signals: append lines to {}  (see docs/RISK_SHELL.md)",
            self.signals_path.display()
        );

        self.journal.write(&JournalEvent::Note {
            ts: Utc::now(),
            text: format!(
                "risk_shell start dry={dry} execute={} mode={mode:?}",
                self.execute
            ),
        });
        self.log_oos_status("shell_start");

        if matches!(mode, RunMode::Live) && self.execute && self.oos_blocking {
            self.journal.write(&JournalEvent::RiskBlock {
                ts: Utc::now(),
                reason: "oos_kill_switch".into(),
                detail: "live execute requested but OOS red – forced dry-run".into(),
            });
        }

        let mut ticker = interval(Duration::from_secs(secs));
        loop {
            ticker.tick().await;
            self.refresh_oos();
            let dry = force_dry_run(mode, self.execute, self.oos_blocking);

            if let Err(e) = self.hl.refresh_market_data().await {
                warn!("refresh_market_data: {e}");
                // Continue with stale cache if any; still emit heartbeat when portfolio works.
            }

            let portfolio = match self.exchange.portfolio().await {
                Ok(p) => p,
                Err(e) => {
                    warn!("portfolio: {e}");
                    continue;
                }
            };

            if !self.brackets_reconciled {
                self.brackets_reconciled = true;
                let mut open = portfolio.positions.clone();
                open.extend(self.dry_book.list());
                for coin in self.brackets.reconcile(&open) {
                    warn!("BRACKET dropped {coin} – no matching open position after restart");
                    self.journal.write(&JournalEvent::Bracket {
                        ts: Utc::now(),
                        coin,
                        action: "removed".into(),
                        level: 0.0,
                        mid: 0.0,
                    });
                }
            }

            // Remote stop flag: a file, re-read every cycle. Presence blocks new
            // entries; closing positions stays allowed (brackets run below).
            let flag_now = crate::kill_flag::is_set(&crate::kill_flag::default_path());
            if flag_now != self.stop_flag_active {
                self.stop_flag_active = flag_now;
                let reason = crate::kill_flag::read_reason(&crate::kill_flag::default_path())
                    .unwrap_or_else(|| "(no reason given)".into());
                if flag_now {
                    warn!("STOP FLAG set – new entries blocked: {reason}");
                } else {
                    warn!("STOP FLAG cleared – entries allowed again");
                }
                self.journal.write(&JournalEvent::Note {
                    ts: Utc::now(),
                    text: if flag_now {
                        format!("stop_flag set: {reason}")
                    } else {
                        "stop_flag cleared".into()
                    },
                });
            }

            // Protections need the current book before any entry is judged.
            self.protections.observe_equity(portfolio.equity);
            self.protections
                .prune_at(Utc::now(), &self.config.settings.risk.protections);

            // Local bracket monitor (SL/TP). Runs every cycle, including the
            // daily-loss kill-switch branch below: closing a position is a
            // risk-reducing action and stays allowed there (dry respected).
            self.check_brackets(&portfolio, dry).await;

            // Kill-switch (daily loss)
            let day_start = (portfolio.equity - portfolio.daily_pnl).max(1e-9);
            let dd = if portfolio.daily_pnl < 0.0 {
                (-portfolio.daily_pnl) / day_start
            } else {
                0.0
            };
            let limit = self.config.settings.risk.max_daily_loss;
            if dd >= limit {
                warn!(
                    "KILL-SWITCH daily_pnl={:.2} equity={:.2} limit={}",
                    portfolio.daily_pnl, portfolio.equity, limit
                );
                self.journal.write(&JournalEvent::KillSwitch {
                    ts: Utc::now(),
                    daily_pnl: portfolio.daily_pnl,
                    equity: portfolio.equity,
                    limit,
                });
                self.drain_signals(&portfolio, true, dry).await;
                continue;
            }

            // Quote snapshot from the **same** HL client we just refreshed.
            let mut lines = Vec::new();
            for sym in &symbols {
                match self.hl.get_quote(sym).await {
                    Ok(q) => {
                        lines.push(format!("{sym} mid={:.4} fund={:?}", q.mid, q.funding_rate))
                    }
                    Err(e) => {
                        warn!("quote {sym}: {e}");
                        lines.push(format!("{sym}=ERR"));
                    }
                }
            }
            info!(
                "equity={:.2} daily_pnl={:.2} pos={} dry={} | {}",
                portfolio.equity,
                portfolio.daily_pnl,
                portfolio.positions.len(),
                dry,
                lines.join(" · ")
            );
            self.journal.write(&JournalEvent::Heartbeat {
                ts: Utc::now(),
                equity: portfolio.equity,
                available: portfolio.available,
                daily_pnl: portfolio.daily_pnl,
                n_positions: portfolio.positions.len(),
                mode: if dry { "dry".into() } else { "live".into() },
            });

            self.drain_signals(&portfolio, self.stop_flag_active, dry)
                .await;
        }
    }

    async fn drain_signals(
        &mut self,
        portfolio: &trading_common::PortfolioSnapshot,
        force_block: bool,
        dry: bool,
    ) {
        let file = match OpenOptions::new().read(true).open(&self.signals_path) {
            Ok(f) => f,
            Err(_) => return,
        };
        // Truncation / rotation: offset past EOF → reset and re-read (dedup handles replays).
        if let Ok(meta) = file.metadata() {
            let len = meta.len();
            if self.signal_offset > len {
                warn!(
                    "signals file truncated (offset {} > len {}); resetting offset",
                    self.signal_offset, len
                );
                self.signal_offset = 0;
            }
        }
        let mut reader = BufReader::new(file);
        if reader.seek(SeekFrom::Start(self.signal_offset)).is_err() {
            warn!(
                "signals seek failed at {}; resetting offset",
                self.signal_offset
            );
            self.signal_offset = 0;
            return;
        }
        let mut line = String::new();
        loop {
            line.clear();
            let n = match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            self.signal_offset += n as u64;
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            let sig: ExternalSignalLine = match serde_json::from_str(t) {
                Ok(s) => s,
                Err(e) => {
                    warn!("bad signal line: {e} | {t}");
                    continue;
                }
            };

            // Clean up processed signals older than 5 minutes.
            let now = Utc::now();
            self.processed_signals
                .retain(|s| now.signed_duration_since(s.ts).num_seconds() < 300);

            // Check if this signal is a duplicate (same coin, side, size within last 5 minutes).
            let coin_upper = sig.coin.trim().to_ascii_uppercase();
            let side_lower = sig.side.to_ascii_lowercase();
            let is_dup = self
                .processed_signals
                .iter()
                .any(|ps| ps.coin == coin_upper && ps.side == side_lower && ps.size == sig.size);

            if is_dup {
                debug!(
                    "Skipping duplicate external signal for {} {}",
                    coin_upper, side_lower
                );
                continue;
            }

            // Save to processed list.
            self.processed_signals.push(ProcessedSignal {
                coin: coin_upper,
                side: side_lower,
                size: sig.size,
                ts: now,
            });

            self.handle_signal(&sig, portfolio, force_block, dry).await;
        }
    }

    async fn handle_signal(
        &mut self,
        sig: &ExternalSignalLine,
        portfolio: &trading_common::PortfolioSnapshot,
        force_block: bool,
        dry: bool,
    ) {
        let source = sig.source.clone().unwrap_or_else(|| "external".into());
        let side_l = sig.side.to_ascii_lowercase();
        let coin = sig.coin.trim().to_ascii_uppercase();

        self.journal.write(&JournalEvent::ExternalSignal {
            ts: Utc::now(),
            source: source.clone(),
            coin: coin.clone(),
            side: side_l.clone(),
            size: sig.size.unwrap_or(0.0),
            note: sig.note.clone(),
        });

        if force_block {
            self.journal.write(&JournalEvent::RiskBlock {
                ts: Utc::now(),
                reason: "kill_switch".into(),
                detail: format!("{coin} {side_l} blocked"),
            });
            warn!("signal blocked by kill-switch: {coin} {side_l}");
            return;
        }

        // Allowlist gate (S1: only configured symbols unless flat of existing pos).
        let whitelist = &self.config.settings.exchange.symbol_whitelist;
        let is_flat = matches!(side_l.as_str(), "flat" | "close");
        let has_pos = portfolio.position_for(&coin).is_some();
        if !whitelist.is_empty()
            && !whitelist.iter().any(|s| s.eq_ignore_ascii_case(&coin))
            && !(is_flat && has_pos)
        {
            self.journal.write(&JournalEvent::RiskBlock {
                ts: Utc::now(),
                reason: "not_in_whitelist".into(),
                detail: format!("{coin} not in symbol_whitelist"),
            });
            warn!("signal blocked: {coin} not in whitelist");
            return;
        }

        let quote = match self.hl.get_quote(&coin).await {
            Ok(q) => q,
            Err(e) => {
                warn!("quote {coin}: {e}");
                self.journal.write(&JournalEvent::RiskBlock {
                    ts: Utc::now(),
                    reason: "quote_error".into(),
                    detail: e.to_string(),
                });
                return;
            }
        };
        if !quote.mid.is_finite() || quote.mid <= 0.0 {
            self.journal.write(&JournalEvent::RiskBlock {
                ts: Utc::now(),
                reason: "bad_mid".into(),
                detail: format!("mid={}", quote.mid),
            });
            return;
        }

        let (side, size, reduce_only) = match side_l.as_str() {
            "flat" | "close" => {
                let pos = match portfolio.position_for(&coin) {
                    Some(p) => p,
                    None => {
                        info!("flat {coin} – no position");
                        return;
                    }
                };
                let close_side = match pos.side {
                    OrderSide::Buy => OrderSide::Sell,
                    OrderSide::Sell => OrderSide::Buy,
                };
                (close_side, pos.size, true)
            }
            "buy" | "long" => (
                OrderSide::Buy,
                sig.size.unwrap_or_else(|| {
                    portfolio.equity * self.config.settings.risk.max_position_per_instrument
                        / quote.mid.max(1e-9)
                }),
                false,
            ),
            "sell" | "short" => (
                OrderSide::Sell,
                sig.size.unwrap_or_else(|| {
                    portfolio.equity * self.config.settings.risk.max_position_per_instrument
                        / quote.mid.max(1e-9)
                }),
                false,
            ),
            other => {
                warn!("unknown side {other}");
                self.journal.write(&JournalEvent::RiskBlock {
                    ts: Utc::now(),
                    reason: "bad_side".into(),
                    detail: other.into(),
                });
                return;
            }
        };

        if !size.is_finite() || size <= 0.0 {
            self.journal.write(&JournalEvent::RiskBlock {
                ts: Utc::now(),
                reason: "bad_size".into(),
                detail: format!("size={size}"),
            });
            return;
        }

        // Protection gates (entry only): stop-loss guard, cooldown, drawdown.
        if !reduce_only {
            let cfg = self.config.settings.risk.protections;
            if let Err(block) =
                self.protections
                    .check_entry(&coin, Utc::now(), portfolio.equity, &cfg)
            {
                self.journal.write(&JournalEvent::RiskBlock {
                    ts: Utc::now(),
                    reason: block.reason().into(),
                    detail: block.to_string(),
                });
                warn!("signal blocked by protection: {block}");
                return;
            }
        }

        // Bracket params: validate early – bad values must NOT place the order.
        let bracket_params = BracketParams {
            sl_pct: sig.sl_pct,
            tp_pct: sig.tp_pct,
            sl_price: sig.sl_price,
            tp_price: sig.tp_price,
        };
        if !bracket_params.is_empty() {
            if let Err(detail) = bracket_params.validate() {
                self.journal.write(&JournalEvent::RiskBlock {
                    ts: Utc::now(),
                    reason: "bad_bracket".into(),
                    detail,
                });
                warn!("signal blocked: bad bracket params {coin} {side_l}");
                return;
            }
        }

        let req = OrderRequest {
            instrument: InstrumentRef {
                venue: VenueKind::Hyperliquid,
                id: coin.clone(),
                market_type: MarketType::Perp,
            },
            side,
            price: None,
            size,
            reduce_only,
            tif: TimeInForce::Ioc,
            reasoning: format!("external:{source} {}", sig.note),
        };

        if let Err(e) = check_order(&req, portfolio, &self.config.settings.risk, quote.mid) {
            self.journal.write(&JournalEvent::RiskBlock {
                ts: Utc::now(),
                reason: "risk_gate".into(),
                detail: e.to_string(),
            });
            warn!("risk reject: {e}");
            return;
        }

        // Resolve concrete SL/TP levels for entry orders (gates already passed).
        let levels = if reduce_only {
            None
        } else {
            match crate::bracket::build_levels(side, quote.mid, bracket_params) {
                Ok(lv) => lv,
                Err(detail) => {
                    self.journal.write(&JournalEvent::RiskBlock {
                        ts: Utc::now(),
                        reason: "bad_bracket".into(),
                        detail,
                    });
                    warn!("signal blocked: bad bracket levels {coin}");
                    return;
                }
            }
        };

        let make_bracket = |entry_price: f64| {
            levels.map(|lv| Bracket {
                coin: coin.clone(),
                side,
                size,
                entry_price,
                sl_price: lv.sl_price,
                tp_price: lv.tp_price,
                created_ts: Utc::now(),
            })
        };

        self.journal.write(&JournalEvent::OrderIntent {
            ts: Utc::now(),
            coin: coin.clone(),
            side: format!("{side:?}"),
            size,
            reduce_only,
            reasoning: req.reasoning.clone(),
            dry_run: dry,
        });

        if dry {
            info!(
                "[DRY] would {:?} {} {} – {}",
                side, size, coin, req.reasoning
            );
            self.journal.write(&JournalEvent::OrderResult {
                ts: Utc::now(),
                coin: coin.clone(),
                status: "dry_run".into(),
                filled: 0.0,
                avg_price: None,
                ok: true,
                error: None,
            });
            // Dry fills go into the shell's own book so the bracket monitor
            // has a position to watch, then register the bracket as usual.
            self.dry_book.apply(&req, quote.mid);
            if let Some(b) = make_bracket(quote.mid) {
                self.register_bracket(b);
            }
            return;
        }

        match self.exchange.place_order(&req).await {
            Ok(ack) => {
                info!("ORDER {:?} {} status={:?}", side, coin, ack.status);
                self.journal.write(&JournalEvent::OrderResult {
                    ts: Utc::now(),
                    coin: coin.clone(),
                    status: format!("{:?}", ack.status),
                    filled: ack.filled_size,
                    avg_price: ack.avg_price,
                    ok: true,
                    error: None,
                });
                if reduce_only {
                    // Manual close: start the coin's cooldown.
                    self.protections.record_close(&coin, Utc::now());
                } else if let Some(b) = make_bracket(quote.mid) {
                    // Refine entry with the real fill price when the venue reports it.
                    let mut b = b;
                    if let Some(avg) = ack.avg_price.filter(|p| p.is_finite() && *p > 0.0) {
                        b.entry_price = avg;
                    }
                    self.register_bracket(b);
                }
            }
            Err(e) => {
                warn!("place_order: {e}");
                self.journal.write(&JournalEvent::OrderResult {
                    ts: Utc::now(),
                    coin,
                    status: "error".into(),
                    filled: 0.0,
                    avg_price: None,
                    ok: false,
                    error: Some(e.to_string()),
                });
                // Entry failed → no bracket (nothing to protect).
            }
        }
    }

    /// Register (or replace) the bracket for a coin and persist it.
    fn register_bracket(&mut self, b: Bracket) {
        info!(
            "BRACKET register {} {:?} entry={:.6} sl={:?} tp={:?}",
            b.coin, b.side, b.entry_price, b.sl_price, b.tp_price
        );
        self.journal.write(&JournalEvent::Bracket {
            ts: Utc::now(),
            coin: b.coin.clone(),
            action: "register".into(),
            level: b.repr_level(),
            mid: b.entry_price,
        });
        self.brackets.upsert(b);
    }

    /// Drop a bracket whose position no longer exists (or flipped externally).
    fn remove_bracket_stale(&mut self, b: &Bracket) {
        if self.brackets.remove(&b.coin) {
            info!("BRACKET removed {} – no matching open position", b.coin);
            self.journal.write(&JournalEvent::Bracket {
                ts: Utc::now(),
                coin: b.coin.clone(),
                action: "removed".into(),
                level: b.repr_level(),
                mid: 0.0,
            });
        }
    }

    /// Monitor registered brackets against open positions each cycle.
    ///
    /// Closes go through the normal path (`check_order` → dry-run or
    /// `place_order`). A bracket is removed **before** the close attempt so a
    /// failing venue call cannot double-fire next cycle (v1 policy: an
    /// un-bracketed position is surfaced via journal/warn, not retried).
    async fn check_brackets(&mut self, portfolio: &trading_common::PortfolioSnapshot, dry: bool) {
        // In dry mode the venue book is empty by definition; monitor the
        // shell's own dry fills instead so SL/TP rehearsal is real.
        let snapshot: Vec<Bracket> = self.brackets.list().to_vec();
        for b in snapshot {
            let coin = b.coin.clone();

            let pos = if dry {
                self.dry_book.position_for(&coin).cloned()
            } else {
                portfolio.position_for(&coin).cloned()
            };
            let Some(pos) = pos else {
                self.remove_bracket_stale(&b);
                continue;
            };
            if pos.side != b.side {
                // Direction changed underneath us (manual flip) – not ours anymore.
                self.remove_bracket_stale(&b);
                continue;
            }

            let quote = match self.hl.get_quote(&coin).await {
                Ok(q) => q,
                Err(e) => {
                    warn!("bracket quote {coin}: {e} – retry next cycle");
                    continue;
                }
            };

            let Some(action) =
                crate::bracket::trigger_for(b.side, quote.mid, b.sl_price, b.tp_price)
            else {
                continue;
            };

            info!(
                "BRACKET {:?} {} mid={:.6} sl={:?} tp={:?}",
                action, coin, quote.mid, b.sl_price, b.tp_price
            );
            self.journal.write(&JournalEvent::Bracket {
                ts: Utc::now(),
                coin: coin.clone(),
                action: action.as_str().into(),
                level: match action {
                    crate::bracket::BracketAction::StopLoss => b.sl_price,
                    crate::bracket::BracketAction::TakeProfit => b.tp_price,
                }
                .unwrap_or(quote.mid),
                mid: quote.mid,
            });
            self.brackets.remove(&coin);
            // Feed the guard: a stop-out counts against the coin, any exit
            // starts its cooldown.
            match action {
                crate::bracket::BracketAction::StopLoss => {
                    self.protections.record_stop(&coin, Utc::now())
                }
                crate::bracket::BracketAction::TakeProfit => {
                    self.protections.record_close(&coin, Utc::now())
                }
            }

            let close_side = match b.side {
                OrderSide::Buy => OrderSide::Sell,
                OrderSide::Sell => OrderSide::Buy,
            };
            if !pos.size.is_finite() || pos.size <= 0.0 {
                warn!("bracket {coin}: bad live size {} – skipped", pos.size);
                continue;
            }
            let req = OrderRequest {
                instrument: InstrumentRef {
                    venue: VenueKind::Hyperliquid,
                    id: coin.clone(),
                    market_type: MarketType::Perp,
                },
                side: close_side,
                price: None,
                size: pos.size,
                reduce_only: true,
                tif: TimeInForce::Ioc,
                reasoning: format!("bracket:{}", action.as_str()),
            };

            if let Err(e) = check_order(&req, portfolio, &self.config.settings.risk, quote.mid) {
                self.journal.write(&JournalEvent::RiskBlock {
                    ts: Utc::now(),
                    reason: "risk_gate".into(),
                    detail: format!("bracket close {coin}: {e}"),
                });
                warn!("bracket close rejected by risk gate: {e}");
                continue;
            }

            if dry {
                info!(
                    "[DRY] would close {} {} {} – bracket:{}",
                    close_side_str(close_side),
                    pos.size,
                    coin,
                    action.as_str()
                );
                self.dry_book.close(&coin);
                self.journal.write(&JournalEvent::OrderResult {
                    ts: Utc::now(),
                    coin: coin.clone(),
                    status: "dry_run".into(),
                    filled: 0.0,
                    avg_price: None,
                    ok: true,
                    error: None,
                });
                continue;
            }

            match self.exchange.place_order(&req).await {
                Ok(ack) => {
                    info!(
                        "BRACKET CLOSE {:?} {} {} status={:?}",
                        close_side, pos.size, coin, ack.status
                    );
                    self.journal.write(&JournalEvent::OrderResult {
                        ts: Utc::now(),
                        coin: coin.clone(),
                        status: format!("{:?}", ack.status),
                        filled: ack.filled_size,
                        avg_price: ack.avg_price,
                        ok: true,
                        error: None,
                    });
                }
                Err(e) => {
                    warn!("bracket close place_order {coin}: {e} – position left UNPROTECTED");
                    self.journal.write(&JournalEvent::Note {
                        ts: Utc::now(),
                        text: format!("bracket close failed {coin}: {e} – position unprotected"),
                    });
                }
            }
        }
    }
}

/// Human-facing side label for logs ("buy"/"sell").
fn close_side_str(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "buy",
        OrderSide::Sell => "sell",
    }
}

pub async fn run_shell(execute: bool) -> Result<()> {
    let settings = trading_common::Settings::load()?;
    // Shell is advisory by default. Live+execute still needs validate (CONFIRM_LIVE, keys).
    // When OOS is red, validate() hard-blocks Live – shell would never start for live.
    // For paper/dryrun, validate only soft-warns.
    settings.validate()?;
    let bankroll = settings.risk.starting_bankroll;
    let mode = settings.mode;
    let (exchange, hl) = build_exchange_pair(&settings.exchange, mode, bankroll)?;
    let config = ConfigStore::from_settings(settings);
    let journal = Journal::open("runtime/journal")?;
    let mut shell = RiskShell::new(config, exchange, hl, journal, execute);
    shell.run().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use trading_common::{OosStats, Settings};

    #[test]
    fn dry_run_policy_oos_locks_live_execute() {
        assert!(force_dry_run(RunMode::DryRun, true, false));
        assert!(force_dry_run(RunMode::Paper, true, false));
        assert!(force_dry_run(RunMode::Live, false, false));
        assert!(force_dry_run(RunMode::Live, true, true));
        assert!(!force_dry_run(RunMode::Live, true, false));
    }

    /// Phase 4 e2e policy: red OOS → live blocked, paper/dry allowed, execute forced dry.
    #[test]
    fn e2e_red_oos_blocks_live_allows_shell_paper() {
        let red = OosStats::new(-25.0, -1.4, 6, 1);
        assert!(red.is_blocking());

        // Settings gates
        let mut live = Settings::default();
        live.mode = RunMode::Live;
        let err = live.validate_with_oos(Some(&red)).unwrap_err().to_string();
        assert!(
            err.contains("kill-switch") || err.contains("locked"),
            "{err}"
        );

        let mut paper = Settings::default();
        paper.mode = RunMode::Paper;
        paper
            .validate_with_oos(Some(&red))
            .expect("paper allowed under red OOS");

        let dry = Settings::default();
        dry.validate_with_oos(Some(&red))
            .expect("dryrun allowed under red OOS");

        // Shell execute path
        assert!(force_dry_run(RunMode::Live, true, red.is_blocking()));
        assert!(force_dry_run(RunMode::Paper, true, red.is_blocking()));
        assert!(force_dry_run(RunMode::DryRun, false, red.is_blocking()));
    }

    #[test]
    fn e2e_green_oos_allows_live_path_to_reach_confirm_gate() {
        let green = OosStats::new(12.0, 0.6, 6, 5);
        assert!(!green.is_blocking());
        assert!(!force_dry_run(RunMode::Live, true, green.is_blocking()));

        let mut live = Settings::default();
        live.mode = RunMode::Live;
        // Still needs CONFIRM_LIVE + keys – but not kill-switch.
        let err = live
            .validate_with_oos(Some(&green))
            .unwrap_err()
            .to_string();
        assert!(
            !err.contains("kill-switch"),
            "should not be kill-switch: {err}"
        );
        assert!(
            err.contains("CONFIRM_LIVE") || err.contains("WALLET") || err.contains("HL_API"),
            "{err}"
        );
    }

    #[test]
    fn e2e_holdout_red_blocks_even_if_fold_metrics_green() {
        let mut stats = OosStats::new(5.0, 0.3, 6, 4);
        stats.holdout_blocking = true;
        assert!(stats.is_blocking());
        assert!(force_dry_run(RunMode::Live, true, stats.is_blocking()));

        let mut live = Settings::default();
        live.mode = RunMode::Live;
        let err = live
            .validate_with_oos(Some(&stats))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("kill-switch") || err.contains("locked"),
            "{err}"
        );
    }
}
