//! Domain-level error types.
//!
//! Clean Architecture: domain errors have no dependency on infrastructure layers.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TradingError {
    #[error("Exchange API error: {0}")]
    ExchangeApi(String),

    #[error("Invalid order parameters: {0}")]
    InvalidOrder(String),

    #[error("Insufficient margin: required={required}, available={available}")]
    InsufficientMargin {
        required: f64,
        available: f64,
    },

    #[error("Rate limit exceeded for {0}")]
    RateLimit(String),

    #[error("Authentication failed: {0}")]
    Auth(String),

    #[error("Signature verification failed: {0}")]
    BadSignature(String),

    #[error("Transaction reverted on-chain: {0}")]
    TxReverted(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Strategy error: {0}")]
    Strategy(String),

    #[error("Risk limit exceeded: {0}")]
    RiskLimit(String),

    #[error("Not enough market data to make a decision")]
    InsufficientData,

    #[error("Position not found: {0}")]
    PositionNotFound(String),

    #[error("Instrument not found: {0}")]
    InstrumentNotFound(String),

    #[error("External service error: {0}")]
    External(String),

    #[error("Signing error: {0}")]
    Signing(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("API error: {0}")]
    Api(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type TradingResult<T> = std::result::Result<T, TradingError>;
