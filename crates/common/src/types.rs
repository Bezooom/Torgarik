//! Shared domain types: venue-agnostic trading primitives.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─── Venue-agnostic core (money path) ───────────────────────────────────────

/// Venue-native instrument id (e.g. `"BTC"`, market slug, token id).
pub type InstrumentId = String;

/// Where an instrument trades.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VenueKind {
    #[default]
    Hyperliquid,
    /// Moscow Exchange, ISS public data (read-only in this workspace).
    Moex,
    Other,
}

impl fmt::Display for VenueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VenueKind::Hyperliquid => write!(f, "hyperliquid"),
            VenueKind::Moex => write!(f, "moex"),
            VenueKind::Other => write!(f, "other"),
        }
    }
}

impl VenueKind {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "hyperliquid" | "hl" => VenueKind::Hyperliquid,
            "moex" | "moex_iss" | "iss" => VenueKind::Moex,
            _ => VenueKind::Other,
        }
    }
}

/// Reference to a tradeable instrument on a venue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstrumentRef {
    pub venue: VenueKind,
    pub id: InstrumentId,
    pub market_type: crate::instruments::MarketType,
}

/// Top-of-book / mark quote (perps + spot).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub instrument: InstrumentRef,
    pub bid: f64,
    pub ask: f64,
    pub mid: f64,
    pub mark: Option<f64>,
    /// Hourly or 8h funding rate as returned by venue (raw; strategy interprets).
    pub funding_rate: Option<f64>,
    pub ts: DateTime<Utc>,
}

/// Time-in-force for order requests.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TimeInForce {
    #[default]
    Gtc,
    Ioc,
    Fok,
}

/// Side of a trade.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl fmt::Display for OrderSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderSide::Buy => write!(f, "buy"),
            OrderSide::Sell => write!(f, "sell"),
        }
    }
}

/// Intent to place an order (strategy → risk → exchange).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRequest {
    pub instrument: InstrumentRef,
    pub side: OrderSide,
    /// None = market / IOC at touch.
    pub price: Option<f64>,
    pub size: f64,
    pub reduce_only: bool,
    pub tif: TimeInForce,
    pub reasoning: String,
}

/// Lifecycle status of an order ack.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    Accepted,
    PartiallyFilled,
    Filled,
    Rejected,
    Cancelled,
    PaperFilled,
}

/// Exchange acknowledgement for a placed order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderAck {
    pub client_id: String,
    pub exchange_id: Option<String>,
    pub status: OrderStatus,
    pub filled_size: f64,
    pub avg_price: Option<f64>,
}

/// Open position (venue-agnostic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    /// Venue instrument id (`BTC`, etc.). Legacy PM code may encode market/outcome elsewhere.
    pub instrument_id: InstrumentId,
    pub side: OrderSide,
    pub size: f64,
    pub entry_price: f64,
    pub unrealized_pnl: f64,
    pub timestamp: DateTime<Utc>,
    // Legacy PM fields (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_id: Option<Uuid>,
}

/// Snapshot of account equity and positions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioSnapshot {
    pub equity: f64,
    pub available: f64,
    pub positions: Vec<Position>,
    pub daily_pnl: f64,
    pub venue: VenueKind,
    pub ts: DateTime<Utc>,
}

impl PortfolioSnapshot {
    pub fn empty(venue: VenueKind, equity: f64) -> Self {
        Self {
            equity,
            available: equity,
            positions: Vec::new(),
            daily_pnl: 0.0,
            venue,
            ts: Utc::now(),
        }
    }

    pub fn position_for(&self, instrument_id: &str) -> Option<&Position> {
        self.positions
            .iter()
            .find(|p| p.instrument_id == instrument_id)
    }
}

/// Portfolio state tracking cash, positions, and PnL (legacy + paper).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioState {
    pub starting_capital: f64,
    pub cash_balance: f64,
    pub positions: Vec<Position>,
    pub realized_pnl: f64,
    pub daily_pnl: f64,
    pub peak_capital: f64,
}

impl PortfolioState {
    pub fn new(initial_capital: f64) -> Self {
        Self {
            starting_capital: initial_capital,
            cash_balance: initial_capital,
            positions: Vec::new(),
            realized_pnl: 0.0,
            daily_pnl: 0.0,
            peak_capital: initial_capital,
        }
    }

    /// Equity using mark prices keyed by instrument id.
    pub fn total_equity_by_instrument(
        &self,
        marks: &std::collections::HashMap<String, f64>,
    ) -> f64 {
        let position_value: f64 = self
            .positions
            .iter()
            .map(|p| {
                let px = marks
                    .get(&p.instrument_id)
                    .copied()
                    .unwrap_or(p.entry_price);
                match p.side {
                    OrderSide::Buy => p.size * px,
                    OrderSide::Sell => p.size * (2.0 * p.entry_price - px), // rough short mtm
                }
            })
            .sum();
        self.cash_balance + position_value
    }

    /// Legacy helper: prices keyed by (market_id, outcome_id).
    pub fn total_equity(
        &self,
        market_prices: &std::collections::HashMap<(Uuid, Uuid), f64>,
    ) -> f64 {
        let position_value: f64 = self
            .positions
            .iter()
            .map(|p| {
                if let (Some(mid), Some(oid)) = (p.market_id, p.outcome_id) {
                    let current_price = market_prices
                        .get(&(mid, oid))
                        .copied()
                        .unwrap_or(p.entry_price);
                    p.size * current_price
                } else {
                    p.size * p.entry_price
                }
            })
            .sum();
        self.cash_balance + position_value
    }

    pub fn total_exposure(&self) -> f64 {
        let total_pos_cost: f64 = self.positions.iter().map(|p| p.size * p.entry_price).sum();
        let denom = self.cash_balance + total_pos_cost;
        if denom > 0.0 {
            total_pos_cost / denom
        } else {
            0.0
        }
    }

    pub fn to_snapshot(&self, venue: VenueKind) -> PortfolioSnapshot {
        PortfolioSnapshot {
            equity: self.cash_balance
                + self
                    .positions
                    .iter()
                    .map(|p| p.size * p.entry_price)
                    .sum::<f64>(),
            available: self.cash_balance,
            positions: self.positions.clone(),
            daily_pnl: self.daily_pnl,
            venue,
            ts: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instruments::MarketType;

    #[test]
    fn venue_kind_parse() {
        assert_eq!(VenueKind::parse("hl"), VenueKind::Hyperliquid);
        assert_eq!(VenueKind::parse("HYPERLIQUID"), VenueKind::Hyperliquid);
        assert_eq!(VenueKind::parse("unknown-venue"), VenueKind::Other);
    }

    #[test]
    fn quote_serde_roundtrip() {
        let q = Quote {
            instrument: InstrumentRef {
                venue: VenueKind::Hyperliquid,
                id: "BTC".into(),
                market_type: MarketType::Perp,
            },
            bid: 100.0,
            ask: 100.5,
            mid: 100.25,
            mark: Some(100.2),
            funding_rate: Some(0.0001),
            ts: Utc::now(),
        };
        let s = serde_json::to_string(&q).unwrap();
        let back: Quote = serde_json::from_str(&s).unwrap();
        assert_eq!(back.instrument.id, "BTC");
        assert!((back.funding_rate.unwrap() - 0.0001).abs() < 1e-12);
    }
}
