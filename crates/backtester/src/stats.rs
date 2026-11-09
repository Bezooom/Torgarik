//! Backtest statistics that fight selection bias.
//!
//! The kill-switch used to trust the raw Sharpe of the *selected* configuration.
//! After a grid search over `N` trials that number is the maximum of `N` noisy
//! estimates, so it is biased upward by construction. This module implements the
//! Bailey / Lopez de Prado corrections:
//!
//! | Function | Question it answers |
//! |---|---|
//! | [`probabilistic_sharpe`] | is this Sharpe distinguishable from zero at all? |
//! | [`expected_max_sharpe`] | how high does the best of `N` trials look by luck alone? |
//! | [`deflated_sharpe`] | is the winner still significant after `N` trials? |
//! | [`cscv_pbo`] | probability that the in-sample winner underperforms out of sample |
//!
//! No external crates: the normal CDF/quantile are implemented here (Acklam's
//! rational approximation for the quantile, Abramowitz-Stegun 7.1.26 for the CDF)
//! and pinned by tests against known values.

/// Euler-Mascheroni constant, used by the expected-maximum-Sharpe formula.
pub const EULER_GAMMA: f64 = 0.577_215_664_901_532_9;

/// Standard normal CDF.
pub fn norm_cdf(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    // Abramowitz & Stegun 7.1.26, |error| < 1.5e-7.
    let z = x.abs() / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.327_591_1 * z);
    let poly = t
        * (0.254_829_592
            + t * (-0.284_496_736
                + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let erf = 1.0 - poly * (-z * z).exp();
    let cdf = 0.5 * (1.0 + if x < 0.0 { -erf } else { erf });
    cdf.clamp(0.0, 1.0)
}

/// Inverse standard normal CDF (Acklam's algorithm, relative error ~1e-9).
pub fn norm_ppf(p: f64) -> f64 {
    if !(0.0..=1.0).contains(&p) || p.is_nan() {
        return f64::NAN;
    }
    if p == 0.0 {
        return f64::NEG_INFINITY;
    }
    if p == 1.0 {
        return f64::INFINITY;
    }
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    const P_LOW: f64 = 0.024_25;
    const P_HIGH: f64 = 1.0 - P_LOW;

    if p < P_LOW {
        let q = (-2.0 * p.ln()).sqrt();
        return (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0);
    }
    if p <= P_HIGH {
        let q = p - 0.5;
        let r = q * q;
        return (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0);
    }
    let q = (-2.0 * (1.0 - p).ln()).sqrt();
    -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
        / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
}

/// Arithmetic mean, `0.0` for an empty slice.
pub fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// Sample standard deviation (n-1), `0.0` when undefined.
pub fn sample_std(xs: &[f64]) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let m = mean(xs);
    let var = xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (xs.len() - 1) as f64;
    var.max(0.0).sqrt()
}

/// Population skewness (0 for < 3 observations).
pub fn skewness(xs: &[f64]) -> f64 {
    if xs.len() < 3 {
        return 0.0;
    }
    let m = mean(xs);
    let sd = sample_std(xs);
    if sd <= 0.0 {
        return 0.0;
    }
    let n = xs.len() as f64;
    xs.iter().map(|x| ((x - m) / sd).powi(3)).sum::<f64>() / n
}

/// Population kurtosis, **not** excess (normal = 3.0; 0 for < 4 observations).
pub fn kurtosis(xs: &[f64]) -> f64 {
    if xs.len() < 4 {
        return 3.0;
    }
    let m = mean(xs);
    let sd = sample_std(xs);
    if sd <= 0.0 {
        return 3.0;
    }
    let n = xs.len() as f64;
    xs.iter().map(|x| ((x - m) / sd).powi(4)).sum::<f64>() / n
}

/// Sharpe ratio per observation (not annualised), `0.0` when undefined.
pub fn sharpe_per_obs(returns: &[f64]) -> f64 {
    let sd = sample_std(returns);
    if sd <= 0.0 {
        return 0.0;
    }
    mean(returns) / sd
}

/// Probabilistic Sharpe Ratio: probability that the true Sharpe exceeds
/// `benchmark` given the sample moments (Bailey & Lopez de Prado, 2012).
pub fn probabilistic_sharpe(sr: f64, n: usize, skew: f64, kurt: f64, benchmark: f64) -> f64 {
    if n < 2 || !sr.is_finite() || !skew.is_finite() || !kurt.is_finite() {
        return f64::NAN;
    }
    let denom = 1.0 - skew * sr + (kurt - 1.0) / 4.0 * sr * sr;
    if denom <= 0.0 {
        return f64::NAN;
    }
    let z = (sr - benchmark) * ((n - 1) as f64).sqrt() / denom.sqrt();
    norm_cdf(z)
}

/// Sharpe ratio the best of `n_trials` independent trials would reach by luck
/// alone, given the variance of the trial Sharpes.
pub fn expected_max_sharpe(var_sr: f64, n_trials: usize) -> f64 {
    if n_trials < 2 || !var_sr.is_finite() || var_sr <= 0.0 {
        return 0.0;
    }
    let n = n_trials as f64;
    let a = norm_ppf(1.0 - 1.0 / n);
    let b = norm_ppf(1.0 - 1.0 / (n * std::f64::consts::E));
    if !a.is_finite() || !b.is_finite() {
        return 0.0;
    }
    var_sr.sqrt() * ((1.0 - EULER_GAMMA) * a + EULER_GAMMA * b)
}

/// Deflated Sharpe Ratio: PSR of the winner evaluated against the luck-adjusted
/// benchmark. `<= 0.95` means "not significant after `n_trials` attempts".
pub fn deflated_sharpe(
    sr: f64,
    n: usize,
    skew: f64,
    kurt: f64,
    var_sr: f64,
    n_trials: usize,
) -> f64 {
    let sr0 = expected_max_sharpe(var_sr, n_trials);
    probabilistic_sharpe(sr, n, skew, kurt, sr0)
}

/// Sample variance of the per-trial Sharpe ratios.
pub fn sharpe_variance(trial_sharpes: &[f64]) -> f64 {
    if trial_sharpes.len() < 2 {
        return 0.0;
    }
    let m = mean(trial_sharpes);
    trial_sharpes.iter().map(|s| (s - m).powi(2)).sum::<f64>() / (trial_sharpes.len() - 1) as f64
}

/// Combinatorially Symmetric Cross-Validation: probability of backtest
/// overfitting.
///
/// `matrix[row][col]` = return of trial `col` at observation `row`; all trials
/// must cover the same observations. Rows are cut into `s` contiguous blocks
/// (even `s`, `>= 4`), every half-split is used as in-sample / out-of-sample,
/// and PBO is the share of splits where the in-sample winner lands in the
/// bottom half out of sample.
///
/// `None` when the matrix is too small to say anything.
pub fn cscv_pbo(matrix: &[Vec<f64>], s: usize) -> Option<f64> {
    let t = matrix.len();
    let n = matrix.first().map(|r| r.len()).unwrap_or(0);
    if n < 2 || s < 4 || s % 2 != 0 || t < s * 2 {
        return None;
    }
    if matrix.iter().any(|r| r.len() != n) {
        return None;
    }
    let block = t / s;
    if block < 2 {
        return None;
    }

    // All combinations of s/2 block indices.
    let half = s / 2;
    let mut combos: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::with_capacity(half);
    fn walk(
        start: usize,
        s: usize,
        half: usize,
        current: &mut Vec<usize>,
        out: &mut Vec<Vec<usize>>,
    ) {
        if current.len() == half {
            out.push(current.clone());
            return;
        }
        for i in start..s {
            current.push(i);
            walk(i + 1, s, half, current, out);
            current.pop();
        }
    }
    walk(0, s, half, &mut current, &mut combos);

    let mut overfit = 0usize;
    let mut total = 0usize;
    for combo in combos {
        let mut is_ret = vec![Vec::<f64>::new(); n];
        let mut oos_ret = vec![Vec::<f64>::new(); n];
        for (b, _) in (0..s).enumerate() {
            let in_is = combo.contains(&b);
            let lo = b * block;
            let hi = if b + 1 == s { t } else { (b + 1) * block };
            for row in lo..hi {
                for (col, val) in matrix[row].iter().enumerate() {
                    if in_is {
                        is_ret[col].push(*val);
                    } else {
                        oos_ret[col].push(*val);
                    }
                }
            }
        }
        let is_sr: Vec<f64> = is_ret.iter().map(|r| sharpe_per_obs(r)).collect();
        let oos_sr: Vec<f64> = oos_ret.iter().map(|r| sharpe_per_obs(r)).collect();
        let Some((best, _)) = is_sr.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) else {
            continue;
        };
        // Rank of the in-sample winner among out-of-sample Sharpes, 1 = worst.
        let winner_oos = oos_sr[best];
        let worse = oos_sr.iter().filter(|s| **s < winner_oos).count();
        let rank = (worse + 1) as f64;
        let omega = rank / (n as f64 + 1.0);
        total += 1;
        if omega <= 0.5 {
            overfit += 1;
        }
    }
    if total == 0 {
        return None;
    }
    Some(overfit as f64 / total as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-6;

    #[test]
    fn normal_cdf_and_quantile_roundtrip() {
        assert!((norm_cdf(0.0) - 0.5).abs() < EPS);
        assert!((norm_cdf(1.959_964) - 0.975).abs() < 1e-5);
        assert!((norm_ppf(0.5)).abs() < 1e-9);
        assert!((norm_ppf(0.975) - 1.959_964).abs() < 1e-5);
        assert!((norm_ppf(0.025) + 1.959_964).abs() < 1e-5);
        for p in [0.001, 0.01, 0.3, 0.7, 0.99, 0.999] {
            assert!((norm_cdf(norm_ppf(p)) - p).abs() < 1e-6, "p={p}");
        }
        assert!(norm_ppf(0.0).is_infinite() && norm_ppf(0.0) < 0.0);
        assert!(norm_ppf(1.5).is_nan());
    }

    #[test]
    fn moments_match_hand_computation() {
        let xs = [1.0, 2.0, 3.0, 4.0];
        assert!((mean(&xs) - 2.5).abs() < 1e-12);
        // sample std of 1,2,3,4 = sqrt(5/3)
        assert!((sample_std(&xs) - (5.0f64 / 3.0).sqrt()).abs() < 1e-12);
        assert!(skewness(&xs).abs() < 1e-12, "symmetric");
        assert!(kurtosis(&xs) < 3.0, "platykurtic");
        assert_eq!(sample_std(&[1.0]), 0.0);
        assert_eq!(mean(&[]), 0.0);
    }

    #[test]
    fn psr_of_zero_sharpe_is_half() {
        let p = probabilistic_sharpe(0.0, 100, 0.0, 3.0, 0.0);
        assert!((p - 0.5).abs() < EPS, "{p}");
    }

    #[test]
    fn psr_grows_with_sample_and_sharpe() {
        let small = probabilistic_sharpe(0.1, 50, 0.0, 3.0, 0.0);
        let big = probabilistic_sharpe(0.1, 500, 0.0, 3.0, 0.0);
        assert!(big > small);
        let weak = probabilistic_sharpe(0.01, 500, 0.0, 3.0, 0.0);
        assert!(weak < big);
    }

    #[test]
    fn expected_max_sharpe_grows_with_trials() {
        let one = expected_max_sharpe(0.01, 1);
        let ten = expected_max_sharpe(0.01, 10);
        let hundred = expected_max_sharpe(0.01, 100);
        assert_eq!(one, 0.0, "a single trial has no selection bias");
        assert!(ten > 0.0 && hundred > ten, "{ten} {hundred}");
        assert_eq!(expected_max_sharpe(0.0, 10), 0.0, "no variance, no bias");
    }

    #[test]
    fn deflated_sharpe_punishes_more_trials() {
        let sr = 0.15;
        let one = deflated_sharpe(sr, 500, 0.0, 3.0, 0.01, 1);
        let ten = deflated_sharpe(sr, 500, 0.0, 3.0, 0.01, 10);
        let hundred = deflated_sharpe(sr, 500, 0.0, 3.0, 0.01, 100);
        assert!((one - probabilistic_sharpe(sr, 500, 0.0, 3.0, 0.0)).abs() < 1e-9);
        assert!(ten < one && hundred < ten, "{one} {ten} {hundred}");
    }

    #[test]
    fn deflated_sharpe_rejects_a_lucky_winner() {
        // Best of 50 trials at SR 0.1 over 200 observations is not significant.
        let dsr = deflated_sharpe(0.1, 200, 0.0, 3.0, 0.02, 50);
        assert!(dsr < 0.95, "dsr={dsr}");
        // The same Sharpe with a single trial is.
        let clean = deflated_sharpe(0.1, 200, 0.0, 3.0, 0.0, 1);
        assert!(clean > 0.9, "clean={clean}");
    }

    #[test]
    fn sharpe_variance_is_sample_variance() {
        let v = sharpe_variance(&[0.1, 0.2, 0.3]);
        assert!((v - 0.01).abs() < 1e-12, "{v}");
        assert_eq!(sharpe_variance(&[0.1]), 0.0);
    }

    #[test]
    fn cscv_detects_a_dominant_trial() {
        // Trial 1 beats trial 0 in every block: the winner persists, PBO = 0.
        let mut matrix = Vec::new();
        for i in 0..200 {
            let noise: f64 = if i % 2 == 0 { 0.001 } else { -0.0009 };
            matrix.push(vec![noise, noise.abs() + 0.001]);
        }
        let pbo = cscv_pbo(&matrix, 10).expect("pbo");
        assert!(pbo < 0.05, "persistent winner should not be flagged: {pbo}");
    }

    #[test]
    fn cscv_flags_a_noise_winner() {
        // Two trials whose relative order flips per block: the in-sample winner
        // is a coin flip out of sample.
        let mut matrix = Vec::new();
        for i in 0..400 {
            let block = i / 40;
            let a = if block % 2 == 0 { 0.002 } else { -0.0018 };
            let b = if block % 2 == 0 { -0.0018 } else { 0.002 };
            matrix.push(vec![a, b]);
        }
        let pbo = cscv_pbo(&matrix, 10).expect("pbo");
        assert!(pbo > 0.3, "flipping winner must look overfit: {pbo}");
    }

    #[test]
    fn cscv_refuses_to_answer_on_tiny_input() {
        assert!(cscv_pbo(&[], 10).is_none());
        let few = vec![vec![0.1, 0.2]; 5];
        assert!(cscv_pbo(&few, 10).is_none(), "too few rows");
        let one_trial = vec![vec![0.1]; 100];
        assert!(cscv_pbo(&one_trial, 10).is_none(), "one trial");
        let many = vec![vec![0.1, 0.2]; 100];
        assert!(cscv_pbo(&many, 3).is_none(), "odd s");
        assert!(
            cscv_pbo(&[vec![0.1, 0.2], vec![0.1]], 10).is_none(),
            "ragged"
        );
    }
}
