//! Portfolio risk checks before sending orders.

use trading_common::{OrderRequest, PortfolioSnapshot, RiskSettings, TradingError};

/// Returns Ok(()) or risk error.
pub fn check_order(
    req: &OrderRequest,
    portfolio: &PortfolioSnapshot,
    risk: &RiskSettings,
    mark_price: f64,
) -> Result<(), TradingError> {
    if !portfolio.equity.is_finite() || portfolio.equity <= 0.0 {
        return Err(TradingError::RiskLimit("equity <= 0".into()));
    }
    if !req.size.is_finite() || req.size <= 0.0 {
        return Err(TradingError::InvalidOrder(format!(
            "invalid size {}",
            req.size
        )));
    }
    if !mark_price.is_finite() || mark_price <= 0.0 {
        return Err(TradingError::InvalidOrder(format!(
            "invalid mark_price {mark_price}"
        )));
    }

    // Daily loss kill-switch
    let loss_frac = if portfolio.daily_pnl < 0.0 {
        (-portfolio.daily_pnl) / portfolio.equity.max(1e-9)
    } else {
        0.0
    };
    // day start approx: equity - daily_pnl
    let day_start = (portfolio.equity - portfolio.daily_pnl).max(1e-9);
    let dd = if portfolio.daily_pnl < 0.0 {
        (-portfolio.daily_pnl) / day_start
    } else {
        0.0
    };
    if dd >= risk.max_daily_loss || loss_frac >= risk.max_daily_loss * 2.0 {
        return Err(TradingError::RiskLimit(format!(
            "max_daily_loss hit: daily_pnl={:.2} equity={:.2} limit={}",
            portfolio.daily_pnl, portfolio.equity, risk.max_daily_loss
        )));
    }

    if req.reduce_only {
        return Ok(());
    }

    let notional = req.size * mark_price;
    let max_pos = portfolio.equity * risk.max_position_per_instrument;
    if notional > max_pos + 1e-6 {
        return Err(TradingError::RiskLimit(format!(
            "position notional {notional:.2} > max {max_pos:.2}"
        )));
    }

    // Gross exposure estimate
    let mut exposure = 0.0;
    for p in &portfolio.positions {
        exposure += p.size * p.entry_price;
    }
    let same = portfolio.position_for(&req.instrument.id);
    let add = match same {
        Some(p) if p.side == req.side => notional,
        Some(_) => 0.0, // flip/reduce handled elsewhere
        None => notional,
    };
    let new_exposure = exposure + add;
    let max_exp = portfolio.equity * risk.max_portfolio_exposure;
    if new_exposure > max_exp + 1e-6 {
        return Err(TradingError::RiskLimit(format!(
            "portfolio exposure {new_exposure:.2} > max {max_exp:.2}"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use trading_common::{InstrumentRef, MarketType, OrderSide, TimeInForce, VenueKind};

    fn req(size: f64) -> OrderRequest {
        OrderRequest {
            instrument: InstrumentRef {
                venue: VenueKind::Hyperliquid,
                id: "BTC".into(),
                market_type: MarketType::Perp,
            },
            side: OrderSide::Buy,
            price: None,
            size,
            reduce_only: false,
            tif: TimeInForce::Ioc,
            reasoning: "t".into(),
        }
    }

    fn risk() -> RiskSettings {
        RiskSettings {
            max_portfolio_exposure: 0.5,
            max_position_per_instrument: 0.2,
            max_daily_loss: 0.03,
            stop_loss: 0.1,
            use_exchange_balance: true,
            starting_bankroll: 1000.0,
            confidence_decay_lambda: 0.1,
            protections: Default::default(),
        }
    }

    #[test]
    fn blocks_daily_loss() {
        let mut port = PortfolioSnapshot::empty(VenueKind::Hyperliquid, 1000.0);
        port.daily_pnl = -40.0;
        port.equity = 960.0;
        // day_start = 1000, dd = 0.04 > 0.03
        let err = check_order(&req(0.01), &port, &risk(), 100.0).unwrap_err();
        assert!(err.to_string().contains("max_daily_loss"));
    }

    #[test]
    fn rejects_bad_size_and_mark() {
        let port = PortfolioSnapshot::empty(VenueKind::Hyperliquid, 1000.0);
        assert!(check_order(&req(0.0), &port, &risk(), 100.0).is_err());
        assert!(check_order(&req(f64::NAN), &port, &risk(), 100.0).is_err());
        assert!(check_order(&req(0.01), &port, &risk(), 0.0).is_err());
    }
}
