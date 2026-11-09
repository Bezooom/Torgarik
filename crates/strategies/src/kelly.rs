//! Kelly criterion and fractional Kelly position sizing.
//!
//! The Kelly criterion gives the optimal fraction of capital to wager
//! when the bet has known win probability `p` and even-money payout.
//! For prediction markets the payout is `(1 / price) - 1` on a win.
//!
//! Reference: Kelly, J. L. (1956) "A New Interpretation of Information
//! Rate."

/// Result of a Kelly calculation.
#[derive(Debug, Clone, Copy)]
pub struct KellyResult {
    /// Fraction of bankroll to risk (0.0 – 1.0).
    pub fraction: f64,
    /// Target dollar position at full Kelly.
    pub target_position: f64,
    /// Whether the bet is favourable.
    pub is_favourable: bool,
}

/// Kelly calculator for binary prediction market outcomes.
pub struct KellyCriterion {
    /// Fractional Kelly multiplier (default 0.25 for safety).
    pub fract: f64,
    /// Minimum edge (probability - break-even) to enter.
    pub min_edge: f64,
}

impl Default for KellyCriterion {
    fn default() -> Self {
        Self {
            fract: 0.25,
            min_edge: 0.02,
        }
    }
}

impl KellyCriterion {
    pub fn new(fract: f64, min_edge: f64) -> Self {
        Self {
            fract: fract.clamp(0.0, 1.0),
            min_edge: min_edge.max(0.0),
        }
    }

    /// Compute the Kelly fraction for a binary market bet.
    ///
    /// * `win_prob` — estimated probability of the outcome resolving YES.
    /// * `price`    — current market price (0.01 – 0.99).
    /// * `bankroll` — total capital available.
    pub fn compute(&self, win_prob: f64, price: f64, bankroll: f64) -> KellyResult {
        let p = win_prob.clamp(0.001, 0.999);
        let price = price.clamp(0.01, 0.99);

        // Decimal odds on a binary outcome at price `p`:
        //   payout per unit staked = 1 / price - 1  (if you win)
        //   net = (1 - price) / price
        let b = (1.0 - price) / price; // odds
        let q = 1.0 - p;

        // Kelly fraction: f = (bp - q) / b
        let numerator = b * p - q;
        let kelly_full = if numerator > 0.0 { numerator / b } else { 0.0 };

        let is_favourable = numerator > self.min_edge;

        let fraction = if is_favourable {
            (kelly_full * self.fract).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let target_position = bankroll * fraction;

        KellyResult {
            fraction,
            target_position,
            is_favourable,
        }
    }
}

/// Full Kelly (aggressive).
pub fn full_kelly(win_prob: f64, price: f64, bankroll: f64) -> KellyResult {
    KellyCriterion::new(1.0, 0.0).compute(win_prob, price, bankroll)
}

/// Half-Kelly (moderate, recommended for most retail traders).
pub fn half_kelly(win_prob: f64, price: f64, bankroll: f64) -> KellyResult {
    KellyCriterion::new(0.5, 0.0).compute(win_prob, price, bankroll)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favourable_bet() {
        // Edge case: win_prob 0.7, price 0.4, bankroll 1000
        let result = half_kelly(0.7, 0.4, 1000.0);
        assert!(result.is_favourable);
        assert!(result.fraction > 0.0);
        assert!(result.target_position > 0.0);
    }

    #[test]
    fn unfavourable_bet() {
        // Edge case: win_prob 0.3, price 0.4 -> negative edge
        let result = half_kelly(0.3, 0.4, 1000.0);
        assert!(!result.is_favourable);
        assert!(result.fraction == 0.0);
    }
}
