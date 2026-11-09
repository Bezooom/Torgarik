//! Prometheus metrics for trading bots.

use std::net::SocketAddr;

use crate::TradingError;

/// Initialise the Prometheus metrics exporter.
///
/// Call this once at application startup *before* spawning any async tasks.
/// The `addr` should be a full bind address, e.g. `"0.0.0.0:9090"`.
pub fn init(addr: &str) -> Result<(), TradingError> {
    let socket_addr: SocketAddr = addr
        .parse()
        .map_err(|e| TradingError::Config(format!("invalid metrics addr: {}", e)))?;

    let builder =
        metrics_exporter_prometheus::PrometheusBuilder::new().with_http_listener(socket_addr);

    let _handle = builder
        .install()
        .map_err(|e| TradingError::Config(format!("metrics install: {}", e)))?;

    Ok(())
}

/// Increment the orders_total counter.
pub fn record_order(_market: &str, _side: &str, _status: &str) {
    metrics::counter!("orders_total");
}

/// Set current position size for a market.
pub fn set_position(_market: &str, _outcome: &str, size: f64) {
    tracing::info!(position_size = size, "position updated");
}

/// Record the confidence of an incoming signal.
pub fn record_signal_confidence(_source: &str, confidence: f64) {
    tracing::info!(signal_confidence = confidence, "signal recorded");
}

/// Portfolio P&L gauge.
pub fn record_pnl(pnl: f64) {
    tracing::info!(pnl, "portfolio PnL updated");
}

/// Operation latency histogram (milliseconds).
pub fn record_latency(_operation: &str, ms: f64) {
    tracing::info!(latency_ms = ms, "operation latency recorded");
}
