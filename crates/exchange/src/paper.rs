//! Paper exchange: real market data, simulated fills & portfolio.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};
use trading_common::{
    InstrumentId, InstrumentRef, OrderAck, OrderRequest, OrderSide, OrderStatus, PortfolioSnapshot,
    Position, Quote, TradingError, TradingResult, VenueKind,
};
use uuid::Uuid;

use crate::traits::{Book, Exchange};

/// Fee model for paper fills (fraction of notional).
#[derive(Debug, Clone, Copy)]
pub struct PaperFees {
    pub taker_bps: f64,
}

impl Default for PaperFees {
    fn default() -> Self {
        Self { taker_bps: 3.5 }
    }
}

/// Default location of the persisted paper book, relative to the CWD.
pub const PAPER_STATE_FILE: &str = "runtime/paper_state.json";

/// Schema version of the persisted paper book.
pub const PAPER_STATE_VERSION: &str = "1";

struct PaperState {
    cash: f64,
    daily_pnl: f64,
    /// UTC date `daily_pnl` belongs to; a new day resets it on load.
    day: NaiveDate,
    positions: HashMap<String, Position>,
}

impl PaperState {
    fn fresh(cash: f64) -> Self {
        Self {
            cash,
            daily_pnl: 0.0,
            day: Utc::now().date_naive(),
            positions: HashMap::new(),
        }
    }
}

/// On-disk shape. Kept separate from `PaperState` so the in-memory layout can
/// change without silently reinterpreting old files.
#[derive(Debug, Serialize, Deserialize)]
struct PersistedPaperState {
    version: String,
    cash: f64,
    daily_pnl: f64,
    /// `YYYY-MM-DD` (UTC).
    day: String,
    positions: Vec<Position>,
}

/// Wraps a live-data exchange and simulates order execution.
pub struct PaperExchange<E: Exchange> {
    inner: Arc<E>,
    fees: PaperFees,
    state: RwLock<PaperState>,
    /// `None` = purely in-memory (tests, throwaway tooling).
    state_path: Option<PathBuf>,
}

impl<E: Exchange> PaperExchange<E> {
    pub fn new(inner: Arc<E>, starting_bankroll: f64, fees: PaperFees) -> Self {
        Self {
            inner,
            fees,
            state: RwLock::new(PaperState::fresh(starting_bankroll)),
            state_path: None,
        }
    }

    /// Paper book that survives restarts: loads `path` when present.
    ///
    /// Missing file = fresh book at `starting_bankroll`. Corrupt file is
    /// quarantined (`.bad` suffix) and the book starts fresh, so a half-written
    /// JSON never silently becomes "zero positions, full cash".
    pub fn persistent(
        inner: Arc<E>,
        starting_bankroll: f64,
        fees: PaperFees,
        path: impl Into<PathBuf>,
    ) -> Self {
        let path = path.into();
        let state = match load_state(&path) {
            Ok(Some(st)) => {
                info!(
                    "paper state restored from {}: cash={:.2} positions={} daily_pnl={:.2}",
                    path.display(),
                    st.cash,
                    st.positions.len(),
                    st.daily_pnl
                );
                st
            }
            Ok(None) => {
                info!(
                    "paper state {} absent – starting bankroll {:.2}",
                    path.display(),
                    starting_bankroll
                );
                PaperState::fresh(starting_bankroll)
            }
            Err(e) => {
                let bad = path.with_extension("json.bad");
                warn!(
                    "paper state {} unreadable ({e}) – quarantined to {}",
                    path.display(),
                    bad.display()
                );
                let _ = std::fs::rename(&path, &bad);
                PaperState::fresh(starting_bankroll)
            }
        };
        Self {
            inner,
            fees,
            state: RwLock::new(state),
            state_path: Some(path),
        }
    }

    /// Persist the book if a path is configured. Best-effort: a write failure
    /// is logged, never fatal for the trading loop.
    fn persist(&self, st: &PaperState) {
        let Some(path) = self.state_path.as_ref() else {
            return;
        };
        let body = PersistedPaperState {
            version: PAPER_STATE_VERSION.to_string(),
            cash: st.cash,
            daily_pnl: st.daily_pnl,
            day: st.day.format("%Y-%m-%d").to_string(),
            positions: st.positions.values().cloned().collect(),
        };
        let json = match serde_json::to_string_pretty(&body) {
            Ok(j) => j,
            Err(e) => {
                warn!("paper state serialize: {e}");
                return;
            }
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Write-then-rename so a crash mid-write cannot truncate the book.
        let tmp = path.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, json) {
            warn!("paper state write {}: {e}", tmp.display());
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, path) {
            warn!("paper state rename {}: {e}", path.display());
        }
    }

    fn fee_on(&self, notional: f64) -> f64 {
        notional * self.fees.taker_bps / 10_000.0
    }

    fn apply_fill(
        st: &mut PaperState,
        id: &str,
        side: OrderSide,
        size: f64,
        fill_px: f64,
        fee: f64,
        reduce_only: bool,
    ) -> TradingResult<()> {
        st.cash -= fee;

        let existing = st.positions.get(id).cloned();

        match (existing, side) {
            (None, _) if reduce_only => {
                return Err(TradingError::InvalidOrder(
                    "reduce_only but no position".into(),
                ));
            }
            (None, OrderSide::Buy) | (None, OrderSide::Sell) => {
                st.positions.insert(
                    id.to_string(),
                    Position {
                        instrument_id: id.to_string(),
                        side,
                        size,
                        entry_price: fill_px,
                        unrealized_pnl: 0.0,
                        timestamp: Utc::now(),
                        market_id: None,
                        outcome_id: None,
                    },
                );
            }
            (Some(p), order_side) if p.side == order_side => {
                // Add to same side
                let new_size = p.size + size;
                let entry = (p.entry_price * p.size + fill_px * size) / new_size;
                st.positions.insert(
                    id.to_string(),
                    Position {
                        instrument_id: id.to_string(),
                        side: p.side,
                        size: new_size,
                        entry_price: entry,
                        unrealized_pnl: 0.0,
                        timestamp: Utc::now(),
                        market_id: None,
                        outcome_id: None,
                    },
                );
            }
            (Some(p), _) => {
                // Opposite side: close then residual open
                let close = size.min(p.size);
                let pnl = match p.side {
                    OrderSide::Buy => (fill_px - p.entry_price) * close,
                    OrderSide::Sell => (p.entry_price - fill_px) * close,
                };
                st.cash += pnl;
                st.daily_pnl += pnl;
                let remain_pos = p.size - close;
                let remain_order = size - close;

                if remain_pos > 1e-12 {
                    st.positions.insert(
                        id.to_string(),
                        Position {
                            instrument_id: id.to_string(),
                            side: p.side,
                            size: remain_pos,
                            entry_price: p.entry_price,
                            unrealized_pnl: 0.0,
                            timestamp: Utc::now(),
                            market_id: None,
                            outcome_id: None,
                        },
                    );
                } else {
                    st.positions.remove(id);
                }

                if remain_order > 1e-12 && !reduce_only {
                    st.positions.insert(
                        id.to_string(),
                        Position {
                            instrument_id: id.to_string(),
                            side,
                            size: remain_order,
                            entry_price: fill_px,
                            unrealized_pnl: 0.0,
                            timestamp: Utc::now(),
                            market_id: None,
                            outcome_id: None,
                        },
                    );
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl<E: Exchange + 'static> Exchange for PaperExchange<E> {
    fn venue(&self) -> VenueKind {
        self.inner.venue()
    }

    async fn list_instruments(&self) -> TradingResult<Vec<InstrumentRef>> {
        self.inner.list_instruments().await
    }

    async fn get_quote(&self, id: &InstrumentId) -> TradingResult<Quote> {
        self.inner.get_quote(id).await
    }

    async fn get_orderbook(&self, id: &InstrumentId, depth: usize) -> TradingResult<Book> {
        self.inner.get_orderbook(id, depth).await
    }

    async fn place_order(&self, req: &OrderRequest) -> TradingResult<OrderAck> {
        if req.size <= 0.0 {
            return Err(TradingError::InvalidOrder("size must be > 0".into()));
        }

        let quote = self.inner.get_quote(&req.instrument.id).await?;
        let fill_px = match req.side {
            OrderSide::Buy => req.price.unwrap_or(quote.ask).max(quote.ask),
            OrderSide::Sell => req.price.unwrap_or(quote.bid).min(quote.bid),
        };

        let notional = fill_px * req.size;
        let fee = self.fee_on(notional);

        let mut st = self.state.write().await;
        Self::apply_fill(
            &mut st,
            &req.instrument.id,
            req.side,
            req.size,
            fill_px,
            fee,
            req.reduce_only,
        )?;

        let client_id = Uuid::new_v4().to_string();
        info!(
            "[PAPER] {:?} {} {} @ {:.4} fee={:.4} cash={:.2}",
            req.side, req.size, req.instrument.id, fill_px, fee, st.cash
        );
        self.persist(&st);

        Ok(OrderAck {
            client_id: client_id.clone(),
            exchange_id: Some(format!("paper-{client_id}")),
            status: OrderStatus::PaperFilled,
            filled_size: req.size,
            avg_price: Some(fill_px),
        })
    }

    async fn cancel_order(&self, exchange_id: &str) -> TradingResult<()> {
        info!("[PAPER] cancel {} (no-op for immediate fills)", exchange_id);
        Ok(())
    }

    async fn portfolio(&self) -> TradingResult<PortfolioSnapshot> {
        let st = self.state.read().await;
        let mut positions: Vec<Position> = st.positions.values().cloned().collect();

        // Perps-style: cash is free collateral (fees/realized only);
        // equity = cash + sum(unrealized). Do NOT add size*entry (double-counts).
        let mut equity = st.cash;
        for p in positions.iter_mut() {
            if let Ok(q) = self.inner.get_quote(&p.instrument_id).await {
                let dir = match p.side {
                    OrderSide::Buy => 1.0,
                    OrderSide::Sell => -1.0,
                };
                p.unrealized_pnl = dir * p.size * (q.mid - p.entry_price);
                equity += p.unrealized_pnl;
            }
        }

        Ok(PortfolioSnapshot {
            equity,
            available: st.cash,
            positions,
            daily_pnl: st.daily_pnl,
            venue: self.inner.venue(),
            ts: Utc::now(),
        })
    }
}

/// Load a persisted book. `Ok(None)` = file absent.
fn load_state(path: &Path) -> Result<Option<PaperState>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let parsed: PersistedPaperState =
        serde_json::from_str(&raw).map_err(|e| format!("parse: {e}"))?;
    if parsed.version != PAPER_STATE_VERSION {
        return Err(format!(
            "version {} != {PAPER_STATE_VERSION}",
            parsed.version
        ));
    }
    if !parsed.cash.is_finite() || parsed.cash < 0.0 {
        return Err(format!("bad cash {}", parsed.cash));
    }
    if !parsed.daily_pnl.is_finite() {
        return Err(format!("bad daily_pnl {}", parsed.daily_pnl));
    }
    let mut positions = HashMap::new();
    for p in parsed.positions {
        if !p.size.is_finite() || p.size <= 0.0 {
            return Err(format!("bad size {} for {}", p.size, p.instrument_id));
        }
        if !p.entry_price.is_finite() || p.entry_price <= 0.0 {
            return Err(format!(
                "bad entry_price {} for {}",
                p.entry_price, p.instrument_id
            ));
        }
        positions.insert(p.instrument_id.clone(), p);
    }
    let day = NaiveDate::parse_from_str(&parsed.day, "%Y-%m-%d")
        .map_err(|e| format!("bad day {}: {e}", parsed.day))?;
    let today = Utc::now().date_naive();
    // Daily PnL is a per-day counter: carry it over only inside the same day.
    let daily_pnl = if day == today { parsed.daily_pnl } else { 0.0 };
    Ok(Some(PaperState {
        cash: parsed.cash,
        daily_pnl,
        day: today,
        positions,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use trading_common::{InstrumentRef, MarketType, TimeInForce};

    struct FakeEx {
        mid: f64,
    }

    #[async_trait]
    impl Exchange for FakeEx {
        fn venue(&self) -> VenueKind {
            VenueKind::Hyperliquid
        }
        async fn list_instruments(&self) -> TradingResult<Vec<InstrumentRef>> {
            Ok(vec![])
        }
        async fn get_quote(&self, id: &InstrumentId) -> TradingResult<Quote> {
            Ok(Quote {
                instrument: InstrumentRef {
                    venue: VenueKind::Hyperliquid,
                    id: id.clone(),
                    market_type: MarketType::Perp,
                },
                bid: self.mid - 1.0,
                ask: self.mid + 1.0,
                mid: self.mid,
                mark: Some(self.mid),
                funding_rate: Some(0.0),
                ts: Utc::now(),
            })
        }
        async fn get_orderbook(&self, id: &InstrumentId, _: usize) -> TradingResult<Book> {
            Ok(Book {
                instrument_id: id.clone(),
                bids: vec![],
                asks: vec![],
            })
        }
        async fn place_order(&self, _: &OrderRequest) -> TradingResult<OrderAck> {
            unreachable!()
        }
        async fn cancel_order(&self, _: &str) -> TradingResult<()> {
            Ok(())
        }
        async fn portfolio(&self) -> TradingResult<PortfolioSnapshot> {
            Ok(PortfolioSnapshot::empty(VenueKind::Hyperliquid, 0.0))
        }
    }

    fn tmp_path(tag: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir()
            .join(format!("paper-{tag}-{nanos}-{}", std::process::id()))
            .join("paper_state.json")
    }

    fn entry(side: OrderSide, size: f64) -> OrderRequest {
        OrderRequest {
            instrument: InstrumentRef {
                venue: VenueKind::Hyperliquid,
                id: "BTC".into(),
                market_type: MarketType::Perp,
            },
            side,
            price: None,
            size,
            reduce_only: false,
            tif: TimeInForce::Ioc,
            reasoning: "test".into(),
        }
    }

    #[tokio::test]
    async fn persistent_paper_book_survives_restart() {
        let path = tmp_path("restart");
        let inner = Arc::new(FakeEx { mid: 100.0 });
        let cash_after_fill;
        {
            let paper =
                PaperExchange::persistent(inner.clone(), 10_000.0, PaperFees::default(), &path);
            paper
                .place_order(&entry(OrderSide::Buy, 2.0))
                .await
                .unwrap();
            cash_after_fill = paper.portfolio().await.unwrap().available;
        }
        // Fresh process, same file: the position must be back, not zeroed.
        let paper = PaperExchange::persistent(inner, 10_000.0, PaperFees::default(), &path);
        let port = paper.portfolio().await.unwrap();
        assert_eq!(port.positions.len(), 1, "position restored");
        assert_eq!(port.positions[0].instrument_id, "BTC");
        assert_eq!(port.positions[0].size, 2.0);
        assert!(
            (port.available - cash_after_fill).abs() < 1e-9,
            "cash restored"
        );
        // Bankroll argument must NOT reset an existing book.
        assert!((port.available - 10_000.0).abs() > 1e-9);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn corrupt_paper_state_is_quarantined_not_silently_zeroed() {
        let path = tmp_path("corrupt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{\"version\":\"1\", broken").unwrap();
        let inner = Arc::new(FakeEx { mid: 100.0 });
        let paper = PaperExchange::persistent(inner, 5_000.0, PaperFees::default(), &path);
        let port = paper.portfolio().await.unwrap();
        assert!(port.positions.is_empty());
        assert!((port.available - 5_000.0).abs() < 1e-9, "fresh bankroll");
        let bad = path.with_extension("json.bad");
        assert!(bad.exists(), "corrupt file kept for forensics");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn stale_daily_pnl_resets_on_new_day() {
        let path = tmp_path("day");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let stale = serde_json::json!({
            "version": PAPER_STATE_VERSION,
            "cash": 9_000.0,
            "daily_pnl": -1_000.0,
            "day": "2000-01-01",
            "positions": [],
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&stale).unwrap()).unwrap();
        let inner = Arc::new(FakeEx { mid: 100.0 });
        let paper = PaperExchange::persistent(inner, 1.0, PaperFees::default(), &path);
        let port = paper.portfolio().await.unwrap();
        assert_eq!(port.daily_pnl, 0.0, "yesterday's daily pnl must not carry");
        assert!((port.available - 9_000.0).abs() < 1e-9, "cash kept");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn paper_buy_and_sell_roundtrip() {
        let inner = Arc::new(FakeEx { mid: 100.0 });
        let paper = PaperExchange::new(inner, 10_000.0, PaperFees::default());
        let req = OrderRequest {
            instrument: InstrumentRef {
                venue: VenueKind::Hyperliquid,
                id: "BTC".into(),
                market_type: MarketType::Perp,
            },
            side: OrderSide::Buy,
            price: None,
            size: 1.0,
            reduce_only: false,
            tif: TimeInForce::Ioc,
            reasoning: "test".into(),
        };
        let ack = paper.place_order(&req).await.unwrap();
        assert_eq!(ack.status, OrderStatus::PaperFilled);
        let port = paper.portfolio().await.unwrap();
        assert_eq!(port.positions.len(), 1);

        let close = OrderRequest {
            side: OrderSide::Sell,
            size: 1.0,
            reduce_only: true,
            ..req.clone()
        };
        paper.place_order(&close).await.unwrap();
        let port2 = paper.portfolio().await.unwrap();
        assert!(port2.positions.is_empty());
    }
}
