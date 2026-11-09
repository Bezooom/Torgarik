//! Money types for trading.

use serde::{Deserialize, Serialize};

/// Represents a monetary amount in the smallest unit of a currency.
/// For USDC, 1 USDC = 1_000_000 smallest units.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Money {
    /// Amount in smallest currency unit (e.g. micro-USDC)
    pub amount: i64,
    /// Decimal places of the currency (6 for USDC)
    pub decimals: u8,
}

impl Money {
    /// Create a new Money instance from a floating-point value.
    pub fn from_f64(value: f64, decimals: u8) -> Self {
        let multiplier = 10_i64.pow(decimals as u32);
        let amount = (value * multiplier as f64) as i64;
        Self { amount, decimals }
    }

    /// Convert back to a floating-point value.
    pub fn to_f64(&self) -> f64 {
        let multiplier = 10_f64.powf(self.decimals as f64);
        self.amount as f64 / multiplier
    }

    /// Add two Money instances (must have the same decimals).
    pub fn add(&self, other: &Self) -> Self {
        assert_eq!(self.decimals, other.decimals, "decimals mismatch");
        Self {
            amount: self.amount + other.amount,
            decimals: self.decimals,
        }
    }

    /// Subtract two Money instances.
    pub fn sub(&self, other: &Self) -> Self {
        assert_eq!(self.decimals, other.decimals, "decimals mismatch");
        Self {
            amount: self.amount - other.amount,
            decimals: self.decimals,
        }
    }

    /// Multiply by a scalar.
    pub fn mul_f64(&self, factor: f64) -> Self {
        Self {
            amount: (self.amount as f64 * factor) as i64,
            decimals: self.decimals,
        }
    }
}

impl std::fmt::Display for Money {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_f64_roundtrip() {
        let m = Money::from_f64(123.456, 6);
        assert!((m.to_f64() - 123.456).abs() < 1e-6);
    }
}
