//! Exchange layer: Hyperliquid (primary money path), MOEX ISS (read-only data),
//! CBR FX rates, paper wrapper.

pub mod cbr;
pub mod factory;
pub mod hl_signer;
pub mod hyperliquid_client;
pub mod moex;
pub mod paper;
pub mod traits;

pub use cbr::{CbrClient, DEFAULT_CBR_URL};
pub use factory::{build_exchange, build_exchange_pair, ExchangeHandle};
pub use hl_signer::{float_to_wire, wallet_from_private_key, HlSignature};
pub use hyperliquid_client::{build_hyperliquid, FundingOpp, HyperliquidClient};
pub use moex::{MoexCandle, MoexClient, MoexDividend};
pub use paper::{PaperExchange, PaperFees};
pub use traits::{Book, BookLevel, Exchange, MarketData};

pub use trading_common::{TradingError, TradingResult};
