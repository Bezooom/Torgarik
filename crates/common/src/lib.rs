//! Common types, configuration, errors, and utilities shared across trading bots.
//!
//! Clean Architecture: this crate is the innermost domain layer — it has zero
//! dependencies on infrastructure (network, DB, exchange APIs).

pub mod config;
pub mod errors;
pub mod fx;
pub mod ids;
pub mod instruments;
pub mod journal;
pub mod metrics;
pub mod money;
pub mod tax;
pub mod time;
pub mod types;
pub mod version;

pub use config::*;
pub use errors::*;
pub use fx::*;
pub use ids::{BrokerId, OpaqueAssetId, TradeId, WalletId};
pub use instruments::{Config, ConfigStore, Instrument, MarketType, OrderType, Side, TradingMode};
pub use journal::*;
pub use money::*;
pub use tax::*;
pub use time::*;
// Venue-agnostic trading primitives (InstrumentId, Quote, PortfolioSnapshot, …)
pub use types::*;
pub use version::*;
