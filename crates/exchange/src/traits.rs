//! Venue-agnostic exchange interface.

use async_trait::async_trait;
use trading_common::{
    InstrumentId, InstrumentRef, OrderAck, OrderRequest, PortfolioSnapshot, Quote, TradingResult,
    VenueKind,
};

/// Normalized L2 book level.
#[derive(Debug, Clone)]
pub struct BookLevel {
    pub price: f64,
    pub size: f64,
}

/// Normalized order book.
#[derive(Debug, Clone)]
pub struct Book {
    pub instrument_id: InstrumentId,
    pub bids: Vec<BookLevel>,
    pub asks: Vec<BookLevel>,
}

/// Read-only market data surface.
///
/// Split out from [`Exchange`] so a venue can join the workspace without any
/// order path at all: MOEX (ISS), a CSV replay, an index feed. Research,
/// dashboard and backtester consume `dyn MarketData`; only the money path
/// requires the order-placement half.
///
/// Every [`Exchange`] is automatically a `MarketData` through the blanket impl
/// below, so nothing on the trading path has to change.
#[async_trait]
pub trait MarketData: Send + Sync {
    fn venue(&self) -> VenueKind;

    async fn list_instruments(&self) -> TradingResult<Vec<InstrumentRef>>;

    async fn get_quote(&self, id: &InstrumentId) -> TradingResult<Quote>;

    async fn get_orderbook(&self, id: &InstrumentId, depth: usize) -> TradingResult<Book>;
}

#[async_trait]
impl<T: Exchange + ?Sized> MarketData for T {
    fn venue(&self) -> VenueKind {
        Exchange::venue(self)
    }

    async fn list_instruments(&self) -> TradingResult<Vec<InstrumentRef>> {
        Exchange::list_instruments(self).await
    }

    async fn get_quote(&self, id: &InstrumentId) -> TradingResult<Quote> {
        Exchange::get_quote(self, id).await
    }

    async fn get_orderbook(&self, id: &InstrumentId, depth: usize) -> TradingResult<Book> {
        Exchange::get_orderbook(self, id, depth).await
    }
}

/// Abstraction over any trading venue (Hyperliquid primary, others optional).
///
/// Money path only. Venues that cannot place orders implement [`MarketData`]
/// instead and are reachable through `dyn MarketData`.
#[async_trait]
pub trait Exchange: Send + Sync {
    fn venue(&self) -> VenueKind;

    async fn list_instruments(&self) -> TradingResult<Vec<InstrumentRef>>;

    async fn get_quote(&self, id: &InstrumentId) -> TradingResult<Quote>;

    async fn get_orderbook(&self, id: &InstrumentId, depth: usize) -> TradingResult<Book>;

    async fn place_order(&self, req: &OrderRequest) -> TradingResult<OrderAck>;

    async fn cancel_order(&self, exchange_id: &str) -> TradingResult<()>;

    async fn portfolio(&self) -> TradingResult<PortfolioSnapshot>;
}
