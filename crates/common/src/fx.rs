//! RUB conversion rates.
//!
//! Domain-only: no HTTP, no clock. The fetcher lives in
//! `trading_exchange::cbr`, this module owns the arithmetic that the tax layer
//! needs, so a cost basis can never be silently mixed across currencies.
//!
//! RF rule of thumb the code encodes: a lot bought in USD keeps its **RUB cost
//! at the acquisition date**, not at the sale date. Converting later, with a
//! newer rate, is exactly the bug this type exists to prevent.

use serde::{Deserialize, Serialize};

use crate::tax::Currency;

/// Official rates published by the Bank of Russia for one date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FxRates {
    /// Date the rates are quoted for (`YYYY-MM-DD`, Moscow time).
    pub date: String,
    /// RUB per 1 USD.
    pub usd_rub: f64,
    /// RUB per 1 EUR.
    pub eur_rub: f64,
    /// RUB per 1 CNY.
    pub cny_rub: f64,
}

impl FxRates {
    /// Construct and validate. Non-positive or non-finite rates are rejected:
    /// a zero rate would silently zero a tax basis.
    pub fn new(
        date: impl Into<String>,
        usd_rub: f64,
        eur_rub: f64,
        cny_rub: f64,
    ) -> Result<Self, String> {
        for (name, v) in [("usd_rub", usd_rub), ("eur_rub", eur_rub), ("cny_rub", cny_rub)] {
            if !v.is_finite() || v <= 0.0 {
                return Err(format!("{name}={v} must be finite and > 0"));
            }
        }
        Ok(Self {
            date: date.into(),
            usd_rub,
            eur_rub,
            cny_rub,
        })
    }

    /// RUB per one unit of `currency`. RUB is 1.0 by definition.
    pub fn rub_per_unit(&self, currency: Currency) -> f64 {
        match currency {
            Currency::Rub => 1.0,
            Currency::Usd | Currency::Usdc => self.usd_rub,
        }
    }

    /// Convert `amount` of `currency` into RUB at these rates.
    pub fn to_rub(&self, currency: Currency, amount: f64) -> f64 {
        amount * self.rub_per_unit(currency)
    }

    /// RUB cost basis of a lot: `qty * unit_price` in `currency`, converted now.
    pub fn cost_basis_rub(&self, currency: Currency, qty: f64, unit_price: f64) -> f64 {
        qty * self.to_rub(currency, unit_price)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rates() -> FxRates {
        FxRates::new("2026-10-31", 80.0, 87.0, 11.2).expect("valid")
    }

    #[test]
    fn rub_is_identity() {
        let r = rates();
        assert_eq!(r.rub_per_unit(Currency::Rub), 1.0);
        assert_eq!(r.to_rub(Currency::Rub, 1000.0), 1000.0);
    }

    #[test]
    fn usd_and_usdc_use_the_same_official_rate() {
        let r = rates();
        assert_eq!(r.to_rub(Currency::Usd, 10.0), 800.0);
        assert_eq!(r.to_rub(Currency::Usdc, 10.0), 800.0);
    }

    #[test]
    fn cost_basis_scales_with_qty_and_price() {
        let r = rates();
        // 2.5 ETH at 2000 USD = 5000 USD = 400 000 RUB
        assert!((r.cost_basis_rub(Currency::Usd, 2.5, 2000.0) - 400_000.0).abs() < 1e-9);
    }

    #[test]
    fn rejects_zero_or_non_finite_rates() {
        assert!(FxRates::new("2026-10-31", 0.0, 87.0, 11.2).is_err());
        assert!(FxRates::new("2026-10-31", f64::NAN, 87.0, 11.2).is_err());
        assert!(FxRates::new("2026-10-31", 80.0, -1.0, 11.2).is_err());
    }

    #[test]
    fn rates_roundtrip_through_json() {
        let r = rates();
        let json = serde_json::to_string(&r).expect("ser");
        let back: FxRates = serde_json::from_str(&json).expect("de");
        assert_eq!(r, back);
    }
}
