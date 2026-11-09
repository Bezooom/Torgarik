//! Pre-registration: pin a research run *before* its result is known.
//!
//! Selection bias does not only come from the grid search inside one run; it
//! also comes from running the same test repeatedly with slightly different
//! settings until something looks green. A pre-registration hash makes that
//! visible: the descriptor below is built from the hypothesis (alpha, grids,
//! window options, cost model) plus a fingerprint of the exact data used, and
//! the hash is written into the report.
//!
//! Workflow:
//!
//! ```bash
//! # 1. before looking at the result: print the hash and commit it
//! cargo run -p backtester -- --walk-forward --alpha trend --preset ethsol --prereg-only
//! # 2. later runs must reproduce it
//! cargo run -p backtester -- --walk-forward --alpha trend --preset ethsol \
//!   --prereg-expect 3f9c... 
//! ```
//!
//! A mismatch means the hypothesis or the data changed after the fact, which is
//! exactly the moment a human should stop and ask why.

use serde::Serialize;
use sha3::{Digest, Sha3_256};

use crate::engine::{Bar, ExecConfig};
use crate::walk_forward::{AlphaKind, FundingGrid, TrendGrid, WfOptions};

/// Everything that defines the hypothesis under test.
#[derive(Debug, Clone, Serialize)]
pub struct PreregDescriptor {
    pub alpha: String,
    pub capital: f64,
    pub n_folds: usize,
    pub train_frac: f64,
    pub expanding: bool,
    pub embargo_bars: usize,
    pub holdout_frac: f64,
    pub funding_grid: Vec<f64>,
    pub funding_exit_ratio: f64,
    pub trend_pairs: Vec<(usize, usize)>,
    pub trend_funding_veto: f64,
    pub trend_min_strength: f64,
    pub trend_vol_lookback: usize,
    pub trend_min_vol_ratio: f64,
    pub exec: ExecConfig,
    /// Per-coin data fingerprint: coin, bar count, first/last ts.
    pub data: Vec<DataFingerprint>,
}

/// Enough of a series to notice that the underlying data moved.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DataFingerprint {
    pub coin: String,
    pub bars: usize,
    pub first_ts: i64,
    pub last_ts: i64,
}

impl DataFingerprint {
    pub fn of(coin: &str, bars: &[Bar]) -> Self {
        Self {
            coin: coin.to_string(),
            bars: bars.len(),
            first_ts: bars.first().map(|b| b.ts).unwrap_or(0),
            last_ts: bars.last().map(|b| b.ts).unwrap_or(0),
        }
    }
}

/// Build the descriptor from the run inputs.
pub fn descriptor(
    series: &[(String, Vec<Bar>)],
    alpha: AlphaKind,
    funding_grid: &FundingGrid,
    trend_grid: &TrendGrid,
    opts: &WfOptions,
) -> PreregDescriptor {
    PreregDescriptor {
        alpha: match alpha {
            AlphaKind::Funding => "funding".into(),
            AlphaKind::Trend => "trend".into(),
        },
        capital: opts.capital,
        n_folds: opts.n_folds,
        train_frac: opts.train_frac,
        expanding: opts.expanding,
        embargo_bars: opts.embargo_bars,
        holdout_frac: opts.holdout_frac,
        funding_grid: funding_grid.enter_grid.clone(),
        funding_exit_ratio: funding_grid.exit_ratio,
        trend_pairs: trend_grid.pairs.clone(),
        trend_funding_veto: trend_grid.funding_veto,
        trend_min_strength: trend_grid.min_trend_strength,
        trend_vol_lookback: trend_grid.vol_lookback,
        trend_min_vol_ratio: trend_grid.min_vol_ratio,
        exec: match alpha {
            AlphaKind::Funding => funding_grid.exec.clone(),
            AlphaKind::Trend => trend_grid.exec.clone(),
        },
        data: series
            .iter()
            .map(|(coin, bars)| DataFingerprint::of(coin, bars))
            .collect(),
    }
}

/// Stable hash of a descriptor. Field order is the struct order, so any change
/// to the hypothesis or the data changes the hash.
pub fn hash(desc: &PreregDescriptor) -> String {
    let canonical = serde_json::to_string(desc).unwrap_or_default();
    let mut hasher = Sha3_256::new();
    hasher.update(canonical.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(7 + digest.len() * 2);
    out.push_str("sha3:");
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// `Ok(())` when `actual` matches `expected`, `Err` with both otherwise.
pub fn verify(expected: &str, actual: &str) -> Result<(), String> {
    let expected = expected.trim();
    if expected == actual {
        return Ok(());
    }
    Err(format!(
        "pre-registration mismatch: expected {expected}, got {actual}. \
         The hypothesis or the data changed after the fact."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bars(n: usize) -> Vec<Bar> {
        (0..n).map(|i| Bar::new(i as i64 * 3600, 100.0 + i as f64, 0.0)).collect()
    }

    fn opts() -> WfOptions {
        WfOptions::default()
    }

    fn desc() -> PreregDescriptor {
        descriptor(
            &[("ETH".to_string(), bars(100))],
            AlphaKind::Trend,
            &FundingGrid::default(),
            &TrendGrid::default(),
            &opts(),
        )
    }

    #[test]
    fn hash_is_stable_for_identical_inputs() {
        assert_eq!(hash(&desc()), hash(&desc()));
        assert!(hash(&desc()).starts_with("sha3:"));
        assert_eq!(hash(&desc()).len(), 5 + 64);
    }

    #[test]
    fn changing_the_hypothesis_changes_the_hash() {
        let base = hash(&desc());
        let mut o = opts();
        o.holdout_frac = 0.25;
        let other = hash(&descriptor(
            &[("ETH".to_string(), bars(100))],
            AlphaKind::Trend,
            &FundingGrid::default(),
            &TrendGrid::default(),
            &o,
        ));
        assert_ne!(base, other, "holdout change must be visible");
    }

    #[test]
    fn changing_the_data_changes_the_hash() {
        let base = hash(&desc());
        let more = hash(&descriptor(
            &[("ETH".to_string(), bars(120))],
            AlphaKind::Trend,
            &FundingGrid::default(),
            &TrendGrid::default(),
            &opts(),
        ));
        assert_ne!(base, more, "bar count change must be visible");

        let mut shifted = bars(100);
        shifted[99].ts += 3600;
        let shifted_hash = hash(&descriptor(
            &[("ETH".to_string(), shifted)],
            AlphaKind::Trend,
            &FundingGrid::default(),
            &TrendGrid::default(),
            &opts(),
        ));
        assert_ne!(base, shifted_hash, "timestamp shift must be visible");
    }

    #[test]
    fn verify_reports_a_mismatch() {
        let h = hash(&desc());
        assert!(verify(&h, &h).is_ok());
        assert!(verify(&format!(" {h} "), &h).is_ok(), "whitespace tolerated");
        let err = verify("sha3:00", &h).expect_err("mismatch");
        assert!(err.contains("mismatch") && err.contains("sha3:00"), "{err}");
    }
}
