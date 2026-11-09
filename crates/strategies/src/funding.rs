//! Funding carry + multi-asset scanner for perpetual futures (Hyperliquid-first).
//!
//! When funding is strongly positive, shorts collect funding → prefer short.
//! When funding is strongly negative, longs collect → prefer long.
//! Scanner ranks the universe by |funding| and feeds candidates into carry logic.

use std::collections::HashMap;

use trading_common::{
    InstrumentRef, MarketType, OrderRequest, OrderSide, PortfolioSnapshot, Quote, StrategySettings,
    TimeInForce, VenueKind,
};

use crate::filters::{UniverseFilter, UniverseMode};

/// Strategy interface for quote-driven bots.
pub trait Strategy: Send {
    fn name(&self) -> &str;
    fn on_quote(&mut self, quote: &Quote, portfolio: &PortfolioSnapshot) -> Option<OrderRequest>;
}

/// Single-instrument funding carry (used per coin).
#[derive(Clone)]
pub struct FundingCarryStrategy {
    pub enter: f64,
    pub exit: f64,
    pub risk_fraction: f64,
    pub min_notional: f64,
    /// Optional hard stop on unrealized loss fraction of entry notional.
    pub stop_loss: f64,
}

impl Default for FundingCarryStrategy {
    fn default() -> Self {
        Self {
            enter: 0.00005,
            exit: 0.000015,
            risk_fraction: 0.08,
            min_notional: 12.0,
            stop_loss: 0.08,
        }
    }
}

impl FundingCarryStrategy {
    pub fn from_settings(s: &StrategySettings, max_pos_frac: f64, stop_loss: f64) -> Self {
        Self {
            enter: s.funding_enter,
            exit: s.funding_exit,
            risk_fraction: (s.kelly_fraction * max_pos_frac.max(0.05)).clamp(0.02, 0.35),
            min_notional: 12.0,
            stop_loss,
        }
    }

    fn desired_side(funding: f64, enter: f64) -> Option<OrderSide> {
        if funding >= enter {
            Some(OrderSide::Sell) // longs pay → short collects
        } else if funding <= -enter {
            Some(OrderSide::Buy)
        } else {
            None
        }
    }

    fn should_exit(funding: f64, exit: f64, position_side: OrderSide) -> bool {
        match position_side {
            OrderSide::Sell => funding <= exit,
            OrderSide::Buy => funding >= -exit,
        }
    }
}

impl Strategy for FundingCarryStrategy {
    fn name(&self) -> &str {
        "funding_carry"
    }

    fn on_quote(&mut self, quote: &Quote, portfolio: &PortfolioSnapshot) -> Option<OrderRequest> {
        let funding = quote.funding_rate?;
        let mid = quote.mid;
        if mid <= 0.0 || portfolio.equity <= 0.0 {
            return None;
        }

        let pos = portfolio.position_for(&quote.instrument.id);
        let target_notional = portfolio.equity * self.risk_fraction;
        let target_size = target_notional / mid;

        if let Some(p) = pos {
            // Stop-loss on adverse mark
            let dir = match p.side {
                OrderSide::Buy => 1.0,
                OrderSide::Sell => -1.0,
            };
            let ret = dir * (mid - p.entry_price) / p.entry_price.max(1e-12);
            if ret <= -self.stop_loss {
                return Some(OrderRequest {
                    instrument: quote.instrument.clone(),
                    side: match p.side {
                        OrderSide::Buy => OrderSide::Sell,
                        OrderSide::Sell => OrderSide::Buy,
                    },
                    price: None,
                    size: p.size,
                    reduce_only: true,
                    tif: TimeInForce::Ioc,
                    reasoning: format!(
                        "stop_loss hit ret={ret:.4} thr=-{:.4} {}",
                        self.stop_loss, quote.instrument.id
                    ),
                });
            }

            if Self::should_exit(funding, self.exit, p.side) {
                return Some(OrderRequest {
                    instrument: quote.instrument.clone(),
                    side: match p.side {
                        OrderSide::Buy => OrderSide::Sell,
                        OrderSide::Sell => OrderSide::Buy,
                    },
                    price: None,
                    size: p.size,
                    reduce_only: true,
                    tif: TimeInForce::Ioc,
                    reasoning: format!(
                        "funding_carry exit: {} funding={funding:.6}",
                        quote.instrument.id
                    ),
                });
            }
            if let Some(want) = Self::desired_side(funding, self.enter) {
                if want == p.side {
                    return None;
                }
                return Some(OrderRequest {
                    instrument: quote.instrument.clone(),
                    side: match p.side {
                        OrderSide::Buy => OrderSide::Sell,
                        OrderSide::Sell => OrderSide::Buy,
                    },
                    price: None,
                    size: p.size,
                    reduce_only: true,
                    tif: TimeInForce::Ioc,
                    reasoning: format!("funding_carry flip close: {}", quote.instrument.id),
                });
            }
            return None;
        }

        let side = Self::desired_side(funding, self.enter)?;
        if target_notional < self.min_notional {
            return None;
        }

        Some(OrderRequest {
            instrument: InstrumentRef {
                venue: quote.instrument.venue,
                id: quote.instrument.id.clone(),
                market_type: quote.instrument.market_type,
            },
            side,
            price: None,
            size: target_size,
            reduce_only: false,
            tif: TimeInForce::Ioc,
            reasoning: format!(
                "funding_carry enter {:?} {} funding={funding:.6} size={target_size:.6}",
                side, quote.instrument.id
            ),
        })
    }
}

/// Multi-asset wrapper: remember last scan ranking; on_quote only acts if coin is ranked
/// or already has a position.
pub struct FundingScannerStrategy {
    inner: FundingCarryStrategy,
    /// coin -> rank score (|funding|)
    hot: HashMap<String, f64>,
    pub filter: UniverseFilter,
}

impl FundingScannerStrategy {
    pub fn new(inner: FundingCarryStrategy, filter: UniverseFilter) -> Self {
        Self {
            inner,
            hot: HashMap::new(),
            filter: filter.normalize(),
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
                crate::filters::default_majors()
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
        Self::new(
            FundingCarryStrategy::from_settings(s, max_pos_frac, stop_loss),
            filter,
        )
    }

    /// Update hot set from raw opportunities (coin, funding, mid, oi_coins).
    pub fn update_from_opps(&mut self, opps: &[(String, f64, f64, f64)]) {
        let filtered = self.filter.filter_ranked(opps);
        self.hot.clear();
        for (coin, f, _, _) in filtered {
            self.hot.insert(coin, f.abs());
        }
    }

    /// Backward-compatible: funding-only ranks without liquidity (Allowlist still gates coin).
    pub fn update_universe(&mut self, ranked: &[(String, f64)]) {
        let opps: Vec<_> = ranked
            .iter()
            .map(|(c, f)| {
                (
                    c.clone(),
                    *f,
                    1.0,            // dummy mid — coin gate only if allowlist/blacklist
                    f64::MAX / 1e9, // pass OI if mid dummy
                )
            })
            .collect();
        // For funding-only path re-check allows_coin
        self.hot.clear();
        let mut n = 0;
        for (coin, f) in ranked {
            if !self.filter.allows_coin(coin) {
                continue;
            }
            if self.filter.max_abs_funding > 0.0 && f.abs() > self.filter.max_abs_funding {
                continue;
            }
            self.hot.insert(coin.clone(), f.abs());
            n += 1;
            if n >= self.filter.top_n {
                break;
            }
        }
        let _ = opps;
    }

    pub fn hot_symbols(&self) -> Vec<String> {
        let mut v: Vec<_> = self.hot.keys().cloned().collect();
        v.sort();
        v
    }
}

impl Strategy for FundingScannerStrategy {
    fn name(&self) -> &str {
        "funding_scanner"
    }

    fn on_quote(&mut self, quote: &Quote, portfolio: &PortfolioSnapshot) -> Option<OrderRequest> {
        let has_pos = portfolio.position_for(&quote.instrument.id).is_some();
        let is_hot = self.hot.contains_key(&quote.instrument.id);
        if !has_pos && !is_hot {
            return None;
        }
        // Cap concurrent positions
        if !has_pos && portfolio.positions.len() >= self.filter.max_positions {
            return None;
        }
        // Hard gates on mid / extreme funding even for hot set
        let funding = quote.funding_rate.unwrap_or(0.0);
        if quote.mid < self.filter.min_mid {
            return None;
        }
        if self.filter.max_abs_funding > 0.0
            && funding.abs() > self.filter.max_abs_funding
            && !has_pos
        {
            return None;
        }
        self.inner.on_quote(quote, portfolio)
    }
}

/// Build a synthetic quote for scanner pipeline.
pub fn make_perp_quote(coin: &str, mid: f64, funding: f64) -> Quote {
    let half = mid * 0.00005;
    Quote {
        instrument: InstrumentRef {
            venue: VenueKind::Hyperliquid,
            id: coin.to_string(),
            market_type: MarketType::Perp,
        },
        bid: mid - half,
        ask: mid + half,
        mid,
        mark: Some(mid),
        funding_rate: Some(funding),
        ts: chrono::Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trading_common::VenueKind;

    #[test]
    fn enter_short_on_positive_funding() {
        let mut s = FundingCarryStrategy::default();
        let port = PortfolioSnapshot::empty(VenueKind::Hyperliquid, 10_000.0);
        let o = s
            .on_quote(&make_perp_quote("BTC", 100.0, 0.0002), &port)
            .unwrap();
        assert_eq!(o.side, OrderSide::Sell);
    }

    #[test]
    fn scanner_allowlist_blocks_meme() {
        let filter = crate::filters::UniverseFilter {
            mode: crate::filters::UniverseMode::Allowlist,
            allowlist: vec!["BTC".into(), "ETH".into()],
            blacklist: crate::filters::default_blacklist(),
            min_oi_notional_usd: 1.0,
            min_mid: 0.01,
            max_abs_funding: 0.001,
            max_positions: 2,
            top_n: 4,
        };
        let mut s = FundingScannerStrategy::new(FundingCarryStrategy::default(), filter);
        s.update_from_opps(&[
            ("ACE".into(), -0.002, 0.1, 1e9),
            ("BTC".into(), 0.0002, 60_000.0, 200.0),
        ]);
        assert!(!s.hot_symbols().contains(&"ACE".to_string()));
        assert!(s.hot_symbols().contains(&"BTC".to_string()));
    }
}
