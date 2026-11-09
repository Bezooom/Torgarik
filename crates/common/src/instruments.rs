//! Instrument (market) types and metadata.

use serde::{Deserialize, Serialize};

pub use crate::config::{RunMode, Settings};
pub use crate::types::*;

/// Market type identifies where / how an instrument trades.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MarketType {
    /// Perpetual futures (Hyperliquid primary path)
    #[default]
    Perp,
    /// Spot / CEX
    Spot,
}

/// A tradeable instrument metadata record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instrument {
    pub symbol: String,
    pub market_type: MarketType,
    /// Optional on-chain token address
    pub token_address: Option<String>,
    pub price_floor: f64,
    pub price_ceiling: f64,
    pub tick_size: f64,
}

/// Side alias used by some modules (prefer OrderSide).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn is_buy(&self) -> bool {
        matches!(self, Side::Buy)
    }
    pub fn is_sell(&self) -> bool {
        matches!(self, Side::Sell)
    }
}

impl From<OrderSide> for Side {
    fn from(s: OrderSide) -> Self {
        match s {
            OrderSide::Buy => Side::Buy,
            OrderSide::Sell => Side::Sell,
        }
    }
}

/// Order type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    Market,
    Limit,
}

/// Trading mode alias.
pub type TradingMode = RunMode;

/// Configuration store wrapper for bots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigStore {
    pub settings: Settings,
    pub trading_mode: TradingMode,
    pub rpc_url: String,
    pub chain_id: u64,
    pub metrics_port: u16,
}

impl ConfigStore {
    pub async fn get(&self) -> &Self {
        self
    }

    pub async fn load(_path: Option<&str>) -> anyhow::Result<Self> {
        let settings = Settings::load()?;
        settings.validate()?;
        Ok(Self {
            trading_mode: settings.mode,
            rpc_url: settings.exchange.rpc_url.clone(),
            chain_id: settings.exchange.chain_id,
            metrics_port: settings.metrics_port,
            settings,
        })
    }

    /// Load without validate() — for unit tests / MCP.
    pub fn from_settings(settings: Settings) -> Self {
        Self {
            trading_mode: settings.mode,
            rpc_url: settings.exchange.rpc_url.clone(),
            chain_id: settings.exchange.chain_id,
            metrics_port: settings.metrics_port,
            settings,
        }
    }
}

/// Alias for ConfigStore
pub type Config = ConfigStore;
