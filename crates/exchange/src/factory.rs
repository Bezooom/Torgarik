//! Build exchange stack from settings.

use std::sync::Arc;

use trading_common::{ExchangeSettings, RunMode, TradingError, TradingResult, VenueKind};

use crate::hyperliquid_client::HyperliquidClient;
use crate::paper::{PaperExchange, PaperFees};
use crate::traits::Exchange;

/// Opaque exchange handle used by bots.
pub type ExchangeHandle = Arc<dyn Exchange>;

/// Construct primary venue client, wrapping with PaperExchange in dry-run/paper modes.
pub fn build_exchange(
    exchange: &ExchangeSettings,
    mode: RunMode,
    starting_bankroll: f64,
) -> TradingResult<ExchangeHandle> {
    let (handle, _) = build_exchange_pair(exchange, mode, starting_bankroll)?;
    Ok(handle)
}

/// Same as [`build_exchange`], but also returns the shared Hyperliquid client.
///
/// Callers that refresh market data (risk shell, alpha loop) **must** use this
/// same `Arc` for quotes — a second client would keep a stale mid cache.
pub fn build_exchange_pair(
    exchange: &ExchangeSettings,
    mode: RunMode,
    starting_bankroll: f64,
) -> TradingResult<(ExchangeHandle, Arc<HyperliquidClient>)> {
    let proxy = exchange.proxy_url.as_deref();
    let venue = exchange.venue_kind();

    match venue {
        VenueKind::Hyperliquid => {
            let pk = std::env::var("WALLET_PRIVATE_KEY")
                .or_else(|_| std::env::var("HL_API_PRIVATE_KEY"))
                .ok();
            // Live: always attach key. Paper: optional (for portfolio queries only).
            let pk_ref = match mode {
                RunMode::Live => Some(pk.as_deref().unwrap_or("")),
                _ => pk.as_deref(),
            };
            if matches!(mode, RunMode::Live) && pk.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
                return Err(TradingError::Config(
                    "live mode requires WALLET_PRIVATE_KEY or HL_API_PRIVATE_KEY".into(),
                ));
            }
            let hl = Arc::new(HyperliquidClient::with_signer(
                &exchange.hyperliquid_base_url,
                &exchange.wallet_address,
                pk_ref.filter(|s| !s.is_empty()),
                proxy,
            )?);

            let handle = match mode {
                RunMode::Live => hl.clone() as ExchangeHandle,
                RunMode::DryRun | RunMode::Paper | RunMode::Backtest => {
                    // Paper book survives restarts: otherwise every shell restart
                    // silently reopens positions at full cash and the sim-vs-paper
                    // parity protocol measures a different book each day.
                    let paper = PaperExchange::persistent(
                        hl.clone(),
                        starting_bankroll,
                        PaperFees::default(),
                        crate::paper::PAPER_STATE_FILE,
                    );
                    Arc::new(paper) as ExchangeHandle
                }
            };
            Ok((handle, hl))
        }
        VenueKind::Moex => Err(TradingError::Config(
            "moex is a read-only data venue (exchange::moex::MoexClient); \
             it is not on the order path"
                .into(),
        )),
        VenueKind::Other => {
            Err(TradingError::Config(format!(
                "venue '{venue}' not enabled in v1 money path (use primary_venue = hyperliquid)"
            )))
        }
    }
}
