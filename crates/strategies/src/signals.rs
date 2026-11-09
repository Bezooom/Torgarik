//! Trend signal generation – the single implementation used by both the
//! backtester and the shell.
//!
//! Backtest/live parity fails silently when research and execution each keep
//! their own copy of an indicator: the strategy that passed walk-forward is not
//! the one that runs. This module is deliberately pure (`&[f64]` in, `Vec<f64>`
//! out, no I/O, no clock) so the same call can serve a historical replay, a
//! live bar close, or a golden test.
//!
//! The plan is **causal**: signal `i` depends only on observations `0..=i`.
//! `backtester::BacktestEngine::audit_causality` enforces that by replaying
//! truncated prefixes.

/// Trend parameters shared by research and execution.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrendParams {
    pub ema_fast: usize,
    pub ema_slow: usize,
    /// Minimum |EMA fast - EMA slow| / mid to call it a trend.
    pub min_trend_strength: f64,
    /// Skip longs when funding >= this, shorts when funding <= -this. `0` = off.
    pub funding_veto: f64,
    /// Lookback for the realised-volatility regime filter.
    pub vol_lookback: usize,
    /// Minimum mean |bar return| to allow entries. `0` = off.
    pub min_vol_ratio: f64,
}

impl Default for TrendParams {
    fn default() -> Self {
        Self {
            ema_fast: 12,
            ema_slow: 21,
            min_trend_strength: 0.0008,
            funding_veto: 0.00015,
            vol_lookback: 24,
            min_vol_ratio: 0.0025,
        }
    }
}

/// Signal series plus the regime gate that produced it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SignalPlan {
    /// Desired side at the close of each bar: `1.0` long, `-1.0` short, `0.0` flat.
    pub signal: Vec<f64>,
    /// Per-bar volatility-regime gate (`true` = entries allowed).
    pub vol_ok: Vec<bool>,
}

/// EMA trend + funding veto + volatility regime, computed causally.
///
/// `mids` and `funding` must be the same length; a short `funding` slice is
/// treated as zero funding from that point on rather than panicking.
pub fn trend_plan(mids: &[f64], funding: &[f64], p: &TrendParams) -> SignalPlan {
    let n = mids.len();
    let mut signal = vec![0.0f64; n];
    let mut vol_ok = vec![false; n];
    if n == 0 {
        return SignalPlan { signal, vol_ok };
    }
    let a_f = 2.0 / (p.ema_fast.max(1) as f64 + 1.0);
    let a_s = 2.0 / (p.ema_slow.max(1) as f64 + 1.0);
    let mut ema_f = mids[0];
    let mut ema_s = mids[0];
    let lb = p.vol_lookback.max(2);

    for i in 0..n {
        let mid = mids[i];
        if !mid.is_finite() || mid <= 0.0 {
            continue;
        }
        if i > 0 {
            ema_f = a_f * mid + (1.0 - a_f) * ema_f;
            ema_s = a_s * mid + (1.0 - a_s) * ema_s;
        }

        if i >= lb {
            let mut sum = 0.0;
            let mut count = 0usize;
            for k in (i - lb + 1)..=i {
                let p0 = mids[k - 1];
                let p1 = mids[k];
                if p0.is_finite() && p0 > 0.0 {
                    sum += ((p1 - p0) / p0).abs();
                    count += 1;
                }
            }
            let m = if count > 0 { sum / count as f64 } else { 0.0 };
            vol_ok[i] = p.min_vol_ratio <= 0.0 || m >= p.min_vol_ratio;
        } else {
            // Not enough history to judge the regime: stay flat.
            vol_ok[i] = false;
        }

        if i + 1 < p.ema_slow {
            signal[i] = 0.0;
            continue;
        }

        let strength = (ema_f - ema_s).abs() / mid.max(1e-12);
        let mut want = if strength < p.min_trend_strength {
            0.0
        } else if ema_f > ema_s {
            1.0
        } else {
            -1.0
        };

        let f = funding.get(i).copied().unwrap_or(0.0);
        if want > 0.0 && p.funding_veto > 0.0 && f >= p.funding_veto {
            want = 0.0;
        }
        if want < 0.0 && p.funding_veto > 0.0 && f <= -p.funding_veto {
            want = 0.0;
        }
        if !vol_ok[i] && want != 0.0 {
            want = 0.0;
        }
        signal[i] = want;
    }

    SignalPlan { signal, vol_ok }
}

/// Funding-carry signal series: short rich funding, long cheap funding.
///
/// `enter` is the absolute funding threshold, `exit` the level that flattens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FundingParams {
    pub enter: f64,
    pub exit: f64,
}

impl Default for FundingParams {
    fn default() -> Self {
        Self {
            enter: 0.00003,
            exit: 0.0000105,
        }
    }
}

/// Causal funding-carry plan.
pub fn funding_plan(funding: &[f64], p: &FundingParams) -> Vec<f64> {
    let mut signal = vec![0.0f64; funding.len()];
    let mut pos_sign = 0.0f64;
    for (i, f) in funding.iter().enumerate() {
        let f = *f;
        if !f.is_finite() {
            signal[i] = pos_sign;
            continue;
        }
        if f >= p.enter {
            pos_sign = -1.0;
        } else if f <= -p.enter {
            pos_sign = 1.0;
        } else if pos_sign > 0.0 && f >= -p.exit {
            pos_sign = 0.0;
        } else if pos_sign < 0.0 && f <= p.exit {
            pos_sign = 0.0;
        }
        signal[i] = pos_sign;
    }
    signal
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave(n: usize, period: usize, amp: f64) -> Vec<f64> {
        (0..n)
            .map(|i| 100.0 * (1.0 + amp * ((i % period) as f64 / period as f64 - 0.5)))
            .collect()
    }

    #[test]
    fn plan_length_matches_input_and_starts_flat() {
        let mids = wave(60, 20, 0.05);
        let p = TrendParams::default();
        let plan = trend_plan(&mids, &[], &p);
        assert_eq!(plan.signal.len(), mids.len());
        assert_eq!(plan.vol_ok.len(), mids.len());
        for s in plan.signal.iter().take(p.ema_slow - 1) {
            assert_eq!(*s, 0.0, "no signal before the slow EMA warms up");
        }
        assert!(plan.vol_ok.iter().take(p.vol_lookback).all(|v| !*v));
    }

    #[test]
    fn uptrend_gives_long_after_warmup() {
        let mids: Vec<f64> = (0..80).map(|i| 100.0 * 1.004f64.powi(i)).collect();
        let p = TrendParams {
            min_vol_ratio: 0.0,
            funding_veto: 0.0,
            min_trend_strength: 0.0001,
            ..TrendParams::default()
        };
        let plan = trend_plan(&mids, &[], &p);
        assert_eq!(*plan.signal.last().expect("last"), 1.0);
    }

    #[test]
    fn funding_veto_flattens_a_long() {
        let mids: Vec<f64> = (0..80).map(|i| 100.0 * 1.004f64.powi(i)).collect();
        let funding = vec![0.001; mids.len()];
        let p = TrendParams {
            min_vol_ratio: 0.0,
            funding_veto: 0.00015,
            min_trend_strength: 0.0001,
            ..TrendParams::default()
        };
        let plan = trend_plan(&mids, &funding, &p);
        assert_eq!(*plan.signal.last().expect("last"), 0.0);
    }

    #[test]
    fn short_funding_slice_is_zero_not_a_panic() {
        let mids: Vec<f64> = (0..60).map(|i| 100.0 + i as f64).collect();
        let plan = trend_plan(&mids, &[0.0, 0.0], &TrendParams::default());
        assert_eq!(plan.signal.len(), mids.len());
    }

    #[test]
    fn plan_is_causal_by_construction() {
        // Truncating the input must not change the shared prefix.
        let mids = wave(120, 17, 0.08);
        let funding: Vec<f64> = (0..120).map(|i| (i as f64 % 7.0) * 0.00005).collect();
        let p = TrendParams {
            min_vol_ratio: 0.0001,
            ..TrendParams::default()
        };
        let full = trend_plan(&mids, &funding, &p);
        for k in [10usize, 40, 80] {
            let prefix = trend_plan(&mids[..k], &funding[..k], &p);
            assert_eq!(
                &prefix.signal[..k],
                &full.signal[..k],
                "prefix {k} diverged"
            );
            assert_eq!(&prefix.vol_ok[..k], &full.vol_ok[..k], "vol_ok {k} diverged");
        }
    }

    #[test]
    fn funding_plan_shorts_rich_funding_and_exits() {
        let funding = [0.0005, 0.0005, 0.0000, -0.0005, 0.0];
        let p = FundingParams {
            enter: 0.0001,
            exit: 0.00002,
        };
        let s = funding_plan(&funding, &p);
        assert_eq!(s[0], -1.0, "rich funding → short");
        assert_eq!(s[2], 0.0, "funding back to 0 → flat");
        assert_eq!(s[3], 1.0, "negative funding → long");
    }

    #[test]
    fn funding_plan_ignores_non_finite_bars() {
        let s = funding_plan(&[0.0005, f64::NAN, 0.0005], &FundingParams { enter: 0.0001, exit: 0.00001 });
        assert_eq!(s, vec![-1.0, -1.0, -1.0]);
    }
}
