//! Conviction penalty: penalize stale signal strength.
//!
//! After a funding or trend signal goes quiet (no new bar has triggered a
//! position change), the original confidence evaporates.  This module provides
//! a lightweight, dependency-free decay model that maps "bars since last
//! meaningful signal" → `[0, 1]` penalty multiplier.
//!
//! Design notes
//! * Pure functions — no floating timestamp math, only bar-count based
//!   decay so it is deterministic in backtests and replay-safe.
//! * `decayed_strength` is the *effective* edge used by sizing.
//! * `penalty_for_flat` returns the fraction lost when flat (used by the
//!   advisory copilot to dim inactive signals).

/// Exponential decay of confidence after the last active signal bar.
///
/// `decayed_strength = strength * exp(-lambda * bars_since_signal)`
#[derive(Debug, Clone, Copy)]
pub struct ConvictionPenalty {
    /// Per-bar exponential decay lambda (e.g. 0.1 → ~90% after 1 bar,
    /// ~37% after 10, ~0.006 after 50).
    pub lambda: f64,
    /// Strength assigned when a brand-new signal fires (typically 1.0).
    pub full_strength: f64,
    /// Bars elapsed since the last meaningful signal bar.
    pub bars_since_signal: u64,
    /// Maximum bars to keep the signal alive at all (floor to zero after).
    pub stale_after: u64,
}

impl Default for ConvictionPenalty {
    fn default() -> Self {
        Self {
            lambda: 0.15,
            full_strength: 1.0,
            bars_since_signal: 0,
            stale_after: 48,
        }
    }
}

impl ConvictionPenalty {
    /// Create a fresh signal (bars_since_signal = 0).
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance one bar of inactivity.
    pub fn tick(mut self) -> Self {
        if self.bars_since_signal < self.stale_after {
            self.bars_since_signal += 1;
        }
        self
    }

    /// Recency-reset: a new signal fired on this bar.
    pub fn refresh(mut self) -> Self {
        self.bars_since_signal = 0;
        self
    }

    /// Current strength after decay. Returns 0.0 if stale.
    pub fn decayed_strength(&self) -> f64 {
        if self.bars_since_signal >= self.stale_after {
            return 0.0;
        }
        let d = self.lambda * self.bars_since_signal as f64;
        let decay = (-d).exp();
        self.full_strength * decay
    }

    /// Penalty fraction for being flat (1.0 - decayed strength).
    /// Used by advisory copilot to dim inactive signals.
    pub fn penalty_for_flat(&self) -> f64 {
        1.0 - self.decayed_strength() / self.full_strength.max(1e-9)
    }

    /// True when the signal has exceeded `stale_after` bars.
    pub fn is_stale(&self) -> bool {
        self.bars_since_signal >= self.stale_after
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_signal_full_strength() {
        let cp = ConvictionPenalty::new();
        assert!((cp.decayed_strength() - 1.0).abs() < 1e-9);
        assert!(!cp.is_stale());
    }

    #[test]
    fn decays_exponentially() {
        let cp = ConvictionPenalty {
            lambda: 0.1,
            full_strength: 1.0,
            bars_since_signal: 10,
            stale_after: 100,
        };
        let expected = (-0.1 * 10.0_f64).exp();
        assert!((cp.decayed_strength() - expected).abs() < 1e-9);
    }

    #[test]
    fn stale_after_zero() {
        let cp = ConvictionPenalty {
            lambda: 0.1,
            full_strength: 1.0,
            bars_since_signal: 50,
            stale_after: 48,
        };
        assert_eq!(cp.decayed_strength(), 0.0);
        assert!(cp.is_stale());
    }

    #[test]
    fn tick_increments() {
        let cp = ConvictionPenalty::new().tick();
        assert_eq!(cp.bars_since_signal, 1);
    }

    #[test]
    fn refresh_resets() {
        let cp = ConvictionPenalty::new().tick().tick().refresh();
        assert_eq!(cp.bars_since_signal, 0);
    }

    #[test]
    fn penalty_for_flat_after_ticks() {
        // lambda=0.1, bars=10 → decayed = e^-1 ≈ 0.3679, penalty ≈ 0.6321
        let cp = ConvictionPenalty {
            lambda: 0.1,
            full_strength: 1.0,
            bars_since_signal: 10,
            stale_after: 100,
        };
        let penalty = cp.penalty_for_flat();
        let expected = 1.0 - (-0.1 * 10.0_f64).exp();
        assert!((penalty - expected).abs() < 1e-6);
        assert!(penalty > 0.5);
    }

    #[test]
    fn tick_does_not_exceed_stale_after() {
        let mut cp = ConvictionPenalty {
            lambda: 0.1,
            full_strength: 1.0,
            bars_since_signal: 47,
            stale_after: 48,
        };
        cp = cp.tick();
        assert_eq!(cp.bars_since_signal, 48);
        assert!(cp.is_stale());
        // One more tick should not go beyond
        cp = cp.tick();
        assert_eq!(cp.bars_since_signal, 48);
    }
}
