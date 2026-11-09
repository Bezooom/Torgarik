//! Majors alpha: EMA trend + funding veto + **vol regime** + **next-bar** intent.
//!
//! Signal on 1h bar close; live paper only acts after a new hour bucket
//! (matches backtest next-bar cadence as closely as quote loop allows).

use std::collections::{HashMap, VecDeque};

use trading_common::{
    InstrumentRef, OrderRequest, OrderSide, PortfolioSnapshot, Quote, StrategySettings, TimeInForce,
};

use crate::filters::{UniverseFilter, UniverseMode};
use crate::funding::Strategy;

#[derive(Debug, Clone)]
pub struct MajorsAlphaConfig {
    pub ema_fast: usize,
    pub ema_slow: usize,
    pub risk_fraction: f64,
    pub stop_loss: f64,
    pub min_notional: f64,
    pub funding_veto: f64,
    pub min_trend_strength: f64,
    pub vol_lookback: usize,
    /// Min mean |hourly return| to allow new entries.
    pub min_vol_ratio: f64,
}

impl Default for MajorsAlphaConfig {
    fn default() -> Self {
        Self {
            ema_fast: 8,
            ema_slow: 34,
            risk_fraction: 0.08,
            stop_loss: 0.06,
            min_notional: 12.0,
            funding_veto: 0.00015,
            min_trend_strength: 0.0008,
            vol_lookback: 24,
            min_vol_ratio: 0.0025,
        }
    }
}

struct CoinState {
    mids: VecDeque<f64>,
    ema_f: Option<f64>,
    ema_s: Option<f64>,
    last_bar_ts: Option<i64>,
    /// Signal computed at last closed bar; executed on next new bar (next-bar).
    pending_side: Option<OrderSide>,
    /// Flat pending (explicit)
    pending_flat: bool,
}

pub struct MajorsAlphaStrategy {
    pub cfg: MajorsAlphaConfig,
    pub filter: UniverseFilter,
    pub bar_secs: i64,
    state: HashMap<String, CoinState>,
}

impl MajorsAlphaStrategy {
    pub fn new(cfg: MajorsAlphaConfig, filter: UniverseFilter) -> Self {
        assert!(cfg.ema_fast < cfg.ema_slow);
        Self {
            cfg,
            filter: filter.normalize(),
            bar_secs: 3600,
            state: HashMap::new(),
        }
    }

    pub fn from_settings(
        s: &StrategySettings,
        allowlist: &[String],
        max_pos_frac: f64,
        stop_loss: f64,
    ) -> Self {
        let mode = match s.universe_mode.to_ascii_lowercase().as_str() {
            "filtered" => UniverseMode::Filtered,
            "open" => UniverseMode::Open,
            _ => UniverseMode::Allowlist,
        };
        let filter = UniverseFilter {
            mode,
            allowlist: if allowlist.is_empty() {
                vec!["ETH".into(), "SOL".into()]
            } else {
                allowlist.to_vec()
            },
            blacklist: s.blacklist.clone(),
            min_oi_notional_usd: s.min_oi_notional_usd,
            min_mid: s.min_mid,
            max_abs_funding: s.max_abs_funding,
            max_positions: s.max_positions.max(1),
            top_n: s.top_n.max(1),
        };
        let cfg = MajorsAlphaConfig {
            ema_fast: s.ema_fast.max(2),
            ema_slow: s.ema_slow.max(s.ema_fast + 1),
            risk_fraction: (s.kelly_fraction * max_pos_frac.max(0.05)).clamp(0.02, 0.25),
            stop_loss,
            min_notional: 12.0,
            funding_veto: s.funding_veto,
            min_trend_strength: s.min_trend_strength,
            vol_lookback: s.vol_lookback.max(2),
            min_vol_ratio: s.min_vol_ratio,
        };
        Self::new(cfg, filter)
    }

    fn alpha(n: usize) -> f64 {
        2.0 / (n as f64 + 1.0)
    }

    fn update_ema(prev: Option<f64>, price: f64, n: usize) -> f64 {
        let a = Self::alpha(n);
        match prev {
            Some(e) => a * price + (1.0 - a) * e,
            None => price,
        }
    }

    fn mean_abs_ret(mids: &VecDeque<f64>, lookback: usize) -> f64 {
        if mids.len() < lookback + 1 {
            return 0.0;
        }
        let start = mids.len() - lookback - 1;
        let mut s = 0.0;
        let mut c = 0usize;
        for i in start + 1..mids.len() {
            let p0 = mids[i - 1];
            let p1 = mids[i];
            if p0 > 0.0 {
                s += ((p1 - p0) / p0).abs();
                c += 1;
            }
        }
        if c == 0 {
            0.0
        } else {
            s / c as f64
        }
    }

    /// On new hour: update EMA, compute signal for *next* bar, return order to execute
    /// pending from *previous* hour.
    fn on_new_bar(
        &mut self,
        coin: &str,
        mid: f64,
        funding: f64,
        portfolio: &PortfolioSnapshot,
        instrument: &InstrumentRef,
    ) -> Option<OrderRequest> {
        let vol_lb = self.cfg.vol_lookback;
        let min_vol = self.cfg.min_vol_ratio;
        let min_str = self.cfg.min_trend_strength;
        let veto = self.cfg.funding_veto;
        let ema_f_n = self.cfg.ema_fast;
        let ema_s_n = self.cfg.ema_slow;
        let risk_frac = self.cfg.risk_fraction;
        let min_notional = self.cfg.min_notional;
        let max_pos = self.filter.max_positions;

        let st = self.state.entry(coin.to_string()).or_insert_with(|| CoinState {
            mids: VecDeque::new(),
            ema_f: None,
            ema_s: None,
            last_bar_ts: None,
            pending_side: None,
            pending_flat: false,
        });

        // Execute pending from previous bar first
        let pending_side = st.pending_side;
        let pending_flat = st.pending_flat;
        st.pending_side = None;
        st.pending_flat = false;

        // Update series / EMA for this closed bar
        st.mids.push_back(mid);
        while st.mids.len() > ema_s_n * 4 {
            st.mids.pop_front();
        }
        st.ema_f = Some(Self::update_ema(st.ema_f, mid, ema_f_n));
        st.ema_s = Some(Self::update_ema(st.ema_s, mid, ema_s_n));

        let ready = st.mids.len() >= ema_s_n;
        let vol = Self::mean_abs_ret(&st.mids, vol_lb);
        let vol_ok = min_vol <= 0.0 || vol >= min_vol;

        // Compute new pending signal for next bar
        if ready {
            if let (Some(f), Some(s)) = (st.ema_f, st.ema_s) {
                let strength = (f - s).abs() / mid.max(1e-12);
                let mut want: Option<OrderSide> = if strength < min_str {
                    None
                } else if f > s {
                    Some(OrderSide::Buy)
                } else {
                    Some(OrderSide::Sell)
                };
                if let Some(side) = want {
                    if veto > 0.0 {
                        match side {
                            OrderSide::Buy if funding >= veto => want = None,
                            OrderSide::Sell if funding <= -veto => want = None,
                            _ => {}
                        }
                    }
                    if !vol_ok {
                        want = None; // no new risk in quiet regime
                    }
                }
                match want {
                    Some(side) => {
                        st.pending_side = Some(side);
                        st.pending_flat = false;
                    }
                    None => {
                        st.pending_side = None;
                        st.pending_flat = true; // flatten if in position next bar
                    }
                }
            }
        }

        // Build order from *previous* pending (next-bar)
        let pos = portfolio.position_for(coin);
        let target_notional = portfolio.equity * risk_frac;
        let target_size = target_notional / mid;

        // Stop always immediate
        if let Some(p) = pos {
            let dir = match p.side {
                OrderSide::Buy => 1.0,
                OrderSide::Sell => -1.0,
            };
            let ret = dir * (mid - p.entry_price) / p.entry_price.max(1e-12);
            if ret <= -self.cfg.stop_loss {
                return Some(order(
                    instrument.clone(),
                    opposite(p.side),
                    p.size,
                    true,
                    format!("majors_alpha stop_loss ret={ret:.4} {coin}"),
                ));
            }
        }

        if pending_flat {
            if let Some(p) = pos {
                return Some(order(
                    instrument.clone(),
                    opposite(p.side),
                    p.size,
                    true,
                    format!("majors_alpha next_bar flatten {coin} vol={vol:.5}"),
                ));
            }
            return None;
        }

        if let Some(want) = pending_side {
            if let Some(p) = pos {
                if p.side == want {
                    return None;
                }
                return Some(order(
                    instrument.clone(),
                    opposite(p.side),
                    p.size,
                    true,
                    format!("majors_alpha next_bar flip close {:?}→{:?} {coin}", p.side, want),
                ));
            }
            if portfolio.positions.len() >= max_pos {
                return None;
            }
            if target_notional < min_notional {
                return None;
            }
            if self.filter.max_abs_funding > 0.0 && funding.abs() > self.filter.max_abs_funding {
                return None;
            }
            return Some(order(
                instrument.clone(),
                want,
                target_size,
                false,
                format!(
                    "majors_alpha next_bar enter {:?} {coin} funding={funding:.6} vol={vol:.5}",
                    want
                ),
            ));
        }
        None
    }
}

fn opposite(s: OrderSide) -> OrderSide {
    match s {
        OrderSide::Buy => OrderSide::Sell,
        OrderSide::Sell => OrderSide::Buy,
    }
}

fn order(
    instrument: InstrumentRef,
    side: OrderSide,
    size: f64,
    reduce_only: bool,
    reasoning: String,
) -> OrderRequest {
    OrderRequest {
        instrument,
        side,
        price: None,
        size,
        reduce_only,
        tif: TimeInForce::Ioc,
        reasoning,
    }
}

impl Strategy for MajorsAlphaStrategy {
    fn name(&self) -> &str {
        "majors_alpha"
    }

    fn on_quote(&mut self, quote: &Quote, portfolio: &PortfolioSnapshot) -> Option<OrderRequest> {
        let coin = &quote.instrument.id;
        let mid = quote.mid;
        if mid <= 0.0 || portfolio.equity <= 0.0 {
            return None;
        }
        if !self.filter.allows_coin(coin) {
            return None;
        }
        if mid < self.filter.min_mid {
            return None;
        }

        let bar_ts = quote.ts.timestamp() / self.bar_secs * self.bar_secs;
        let is_new = self
            .state
            .get(coin)
            .map(|s| s.last_bar_ts != Some(bar_ts))
            .unwrap_or(true);

        // Same hour: only stop-loss checks
        if !is_new {
            if let Some(p) = portfolio.position_for(coin) {
                let dir = match p.side {
                    OrderSide::Buy => 1.0,
                    OrderSide::Sell => -1.0,
                };
                let ret = dir * (mid - p.entry_price) / p.entry_price.max(1e-12);
                if ret <= -self.cfg.stop_loss {
                    return Some(order(
                        quote.instrument.clone(),
                        opposite(p.side),
                        p.size,
                        true,
                        format!("majors_alpha stop_loss ret={ret:.4} {coin}"),
                    ));
                }
            }
            return None;
        }

        // Mark bar ts then process new bar
        {
            let st = self.state.entry(coin.to_string()).or_insert_with(|| CoinState {
                mids: VecDeque::new(),
                ema_f: None,
                ema_s: None,
                last_bar_ts: None,
                pending_side: None,
                pending_flat: false,
            });
            st.last_bar_ts = Some(bar_ts);
        }

        let funding = quote.funding_rate.unwrap_or(0.0);
        self.on_new_bar(coin, mid, funding, portfolio, &quote.instrument)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use trading_common::{MarketType, PortfolioSnapshot, VenueKind};

    fn q(coin: &str, mid: f64, funding: f64, hour: i64) -> Quote {
        Quote {
            instrument: InstrumentRef {
                venue: VenueKind::Hyperliquid,
                id: coin.into(),
                market_type: MarketType::Perp,
            },
            bid: mid * 0.999,
            ask: mid * 1.001,
            mid,
            mark: Some(mid),
            funding_rate: Some(funding),
            ts: Utc::now() + chrono::Duration::hours(hour),
        }
    }

    #[test]
    fn eth_allowlist_rejects_btc() {
        let filter = UniverseFilter {
            mode: UniverseMode::Allowlist,
            allowlist: vec!["ETH".into(), "SOL".into()],
            blacklist: vec![],
            min_oi_notional_usd: 0.0,
            min_mid: 0.01,
            max_abs_funding: 0.01,
            max_positions: 2,
            top_n: 4,
        };
        let mut s = MajorsAlphaStrategy::new(
            MajorsAlphaConfig {
                min_vol_ratio: 0.0,
                ..Default::default()
            },
            filter,
        );
        let port = PortfolioSnapshot::empty(VenueKind::Hyperliquid, 10_000.0);
        assert!(s.on_quote(&q("BTC", 60_000.0, 0.0, 0), &port).is_none());
    }

    #[test]
    fn uptrend_enters_after_warmup_next_bar() {
        let filter = UniverseFilter {
            mode: UniverseMode::Allowlist,
            allowlist: vec!["ETH".into()],
            blacklist: vec![],
            min_oi_notional_usd: 0.0,
            min_mid: 0.01,
            max_abs_funding: 0.01,
            max_positions: 2,
            top_n: 4,
        };
        let mut s = MajorsAlphaStrategy::new(
            MajorsAlphaConfig {
                ema_fast: 3,
                ema_slow: 8,
                min_trend_strength: 0.0001,
                funding_veto: 0.0,
                min_vol_ratio: 0.0,
                vol_lookback: 3,
                ..Default::default()
            },
            filter,
        );
        let port = PortfolioSnapshot::empty(VenueKind::Hyperliquid, 10_000.0);
        let mut mid = 100.0;
        let mut saw_entry = false;
        for i in 0..50 {
            mid *= 1.003;
            if let Some(o) = s.on_quote(&q("ETH", mid, 0.0, i), &port) {
                if !o.reduce_only && o.side == OrderSide::Buy {
                    saw_entry = true;
                }
            }
        }
        assert!(saw_entry);
    }
}
