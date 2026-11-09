//! Bayesian probability updating for market probabilities.
//!
//! Models a binary outcome as a Beta distribution `Beta(alpha, beta)`.
//! New evidence (observations) updates `alpha` / `beta` via conjugate update.
//! The posterior mean is `(alpha) / (alpha + beta)`.

use serde::{Deserialize, Serialize};

/// A Bayesian belief over a binary outcome probability.
///
/// Internally stores the two concentration parameters of a Beta distribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BetaBelief {
    pub alpha: f64,
    pub beta: f64,
}

impl BetaBelief {
    /// Create a uniform prior `Beta(1, 1)`.
    pub fn uniform() -> Self {
        Self {
            alpha: 1.0,
            beta: 1.0,
        }
    }

    /// Create a prior with pseudo-counts (equivalent sample size).
    pub fn with_strength(mean: f64, strength: f64) -> Self {
        let mean = mean.clamp(0.001, 0.999);
        let alpha = mean * strength;
        let beta = (1.0 - mean) * strength;
        Self { alpha, beta }
    }

    /// Posterior mean: `alpha / (alpha + beta)`.
    pub fn mean(&self) -> f64 {
        self.alpha / (self.alpha + self.beta)
    }

    /// Posterior variance: `ab / ((a+b)^2 (a+b+1))`.
    pub fn variance(&self) -> f64 {
        let sum = self.alpha + self.beta;
        (self.alpha * self.beta) / (sum * sum * (sum + 1.0))
    }

    /// 95% credible interval (approximate, normal).
    pub fn credible_interval_95(&self) -> (f64, f64) {
        let mean = self.mean();
        let var = self.variance();
        let std = var.sqrt();
        let (lower, upper) = (mean - 1.96 * std, mean + 1.96 * std);
        (
            lower.clamp(0.0, 1.0),
            upper.clamp(0.0, 1.0),
        )
    }

    /// Update belief with a single Bernoulli observation.
    ///
    /// `success = true` means the event occurred.
    /// `weight` scales the observation strength (1.0 = standard update).
    pub fn update(&mut self, success: bool, weight: f64) {
        let w = weight.max(0.0);
        if success {
            self.alpha += w;
        } else {
            self.beta += w;
        }
    }

    /// Update belief with a continuous outcome in the range [0.0, 1.0].
    ///
    /// 1.0 represents a complete success, 0.0 represents a complete failure,
    /// and 0.5 represents no informational update.
    pub fn update_continuous(&mut self, value: f64, weight: f64) {
        // NaN clamp stays NaN in IEEE-754 — reject non-finite inputs.
        if !value.is_finite() || !weight.is_finite() {
            return;
        }
        let val = value.clamp(0.0, 1.0);
        let w = weight.max(0.0);
        self.alpha += val * w;
        self.beta += (1.0 - val) * w;
    }

    /// Batch update with observed successes and failures.
    pub fn update_batch(&mut self, successes: usize, failures: usize, weight: f64) {
        let w = weight.max(0.0);
        self.alpha += successes as f64 * w;
        self.beta += failures as f64 * w;
    }

    /// Equivalent sample size (alpha + beta).
    pub fn n(&self) -> f64 {
        self.alpha + self.beta
    }

    /// Exponentially decay belief strength toward a uniform `Beta(1, 1)` prior.
    ///
    /// `lambda` is the decay rate per day; `days` is the elapsed time to apply.
    /// Only excess pseudo-counts above the uniform floor are decayed:
    /// `(α-1, β-1) *= exp(-λ · days)`.
    ///
    /// Safe to apply in successive slices: `decay(λ, t1)` then `decay(λ, t2)`
    /// equals `decay(λ, t1 + t2)` (no double-counting of absolute inactivity).
    pub fn decay(&mut self, lambda: f64, days: f64) {
        if !lambda.is_finite() || !days.is_finite() || days <= 0.0 || lambda <= 0.0 {
            return;
        }
        let factor = (-lambda * days).exp();
        if !factor.is_finite() {
            return;
        }
        let a_ex = (self.alpha - 1.0).max(0.0) * factor;
        let b_ex = (self.beta - 1.0).max(0.0) * factor;
        self.alpha = 1.0 + a_ex;
        self.beta = 1.0 + b_ex;
    }
}

impl Default for BetaBelief {
    fn default() -> Self {
        Self::uniform()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posterior_mean_basic() {
        let mut b = BetaBelief::uniform();
        assert!((b.mean() - 0.5).abs() < 1e-9);
        b.update(true, 1.0); // Beta(2,1) -> mean = 2/3
        assert!((b.mean() - 2.0 / 3.0).abs() < 1e-9);
        b.update(true, 1.0); // Beta(3,1) -> mean = 3/4
        assert!((b.mean() - 3.0 / 4.0).abs() < 1e-9);
    }

    #[test]
    fn credible_interval_contains_mean() {
        let mut b = BetaBelief::uniform();
        b.update_batch(3, 1, 5.0); // strong prior toward true
        let (lo, hi) = b.credible_interval_95();
        let m = b.mean();
        assert!(lo < m && m < hi);
    }

    #[test]
    fn decay_reduces_strength_and_shifts_toward_uniform() {
        let mut b = BetaBelief::with_strength(0.8, 10.0);
        let original_mean = b.mean();
        let original_n = b.n();
        // 10 days with lambda 0.1 → excess *= exp(-1) ≈ 0.368
        b.decay(0.1, 10.0);

        let new_mean = b.mean();
        assert!(b.n() < original_n, "n should shrink, got {} vs {}", b.n(), original_n);
        // Mean should be closer to 0.5 than before
        assert!((new_mean - 0.5).abs() < (original_mean - 0.5).abs());
    }

    #[test]
    fn decay_zero_days_no_change() {
        let mut b = BetaBelief::with_strength(0.7, 5.0);
        let pre_mean = b.mean();
        let pre_n = b.n();
        b.decay(0.1, 0.0);
        assert!((b.mean() - pre_mean).abs() < 1e-9);
        assert!((b.n() - pre_n).abs() < 1e-9);
    }

    #[test]
    fn decay_is_additive_in_time() {
        let mut once = BetaBelief::with_strength(0.8, 20.0);
        let mut twice = BetaBelief::with_strength(0.8, 20.0);
        once.decay(0.1, 10.0);
        twice.decay(0.1, 4.0);
        twice.decay(0.1, 6.0);
        assert!((once.alpha - twice.alpha).abs() < 1e-9);
        assert!((once.beta - twice.beta).abs() < 1e-9);
    }

    #[test]
    fn update_continuous_basic() {
        let mut b = BetaBelief::uniform();
        b.update_continuous(0.5, 2.0); // Beta(1 + 1, 1 + 1) = Beta(2, 2)
        assert!((b.mean() - 0.5).abs() < 1e-9);
        assert!((b.n() - 4.0).abs() < 1e-9);

        b.update_continuous(0.75, 4.0); // Beta(2 + 3, 2 + 1) = Beta(5, 3)
        assert!((b.mean() - 5.0 / 8.0).abs() < 1e-9);
    }

    #[test]
    fn update_continuous_rejects_nan() {
        let mut b = BetaBelief::uniform();
        let a = b.alpha;
        let beta = b.beta;
        b.update_continuous(f64::NAN, 1.0);
        b.update_continuous(0.5, f64::NAN);
        assert_eq!(b.alpha, a);
        assert_eq!(b.beta, beta);
    }
}
