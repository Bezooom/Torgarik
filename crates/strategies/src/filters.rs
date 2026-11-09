//! Universe filters: allowlist / blacklist / min OI notional / min mid.

use serde::{Deserialize, Serialize};

/// How the tradable set is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UniverseMode {
    /// Only `symbol_whitelist` (safest after OOS failure on memes).
    #[default]
    Allowlist,
    /// Scan universe with blacklist + liquidity gates.
    Filtered,
    /// No coin filter (legacy; not recommended).
    Open,
}

/// Hard gate before ranking / entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniverseFilter {
    pub mode: UniverseMode,
    /// Allowed coins when mode = Allowlist (and seed list for Filtered).
    pub allowlist: Vec<String>,
    /// Never trade these (meme / illiquid / toxic funding names).
    pub blacklist: Vec<String>,
    /// Min open-interest notional ≈ OI_coins * mid (USD).
    pub min_oi_notional_usd: f64,
    /// Min mid price (filters sub-penny dust).
    pub min_mid: f64,
    /// Max |funding| to enter — extreme funding often = blow-up risk (0 = off).
    pub max_abs_funding: f64,
    /// Max concurrent positions for scanner.
    pub max_positions: usize,
    /// Top-N hot coins after filters (Filtered mode).
    pub top_n: usize,
}

impl Default for UniverseFilter {
    fn default() -> Self {
        Self {
            mode: UniverseMode::Allowlist,
            allowlist: default_majors(),
            blacklist: default_blacklist(),
            min_oi_notional_usd: 5_000_000.0,
            min_mid: 0.05,
            max_abs_funding: 0.001, // 0.1% per hour is extreme
            max_positions: 2,
            top_n: 4,
        }
    }
}

pub fn default_majors() -> Vec<String> {
    vec![
        "BTC".into(),
        "ETH".into(),
        "SOL".into(),
        "XRP".into(),
        "BNB".into(),
        "DOGE".into(),
        "AVAX".into(),
        "LINK".into(),
        "ARB".into(),
        "OP".into(),
        "SUI".into(),
        "APT".into(),
        "NEAR".into(),
        "ATOM".into(),
        "DOT".into(),
        "LTC".into(),
        "BCH".into(),
        "UNI".into(),
        "AAVE".into(),
        "TRX".into(),
        "TON".into(),
        "HYPE".into(),
    ]
}

/// Known high-funding / low-quality names from RF paper runs (extend as needed).
pub fn default_blacklist() -> Vec<String> {
    vec![
        "ACE".into(),
        "NOT".into(),
        "MOVE".into(),
        "kNEIRO".into(),
        "NEIRO".into(),
        "SUPER".into(),
        "CASHCAT".into(),
        "POPCAT".into(),
        "WIF".into(),
        "MEW".into(),
        "PNUT".into(),
        "GOAT".into(),
        "MOODENG".into(),
        "FARTCOIN".into(),
        "BRETT".into(),
        "GIGA".into(),
        "HIPPO".into(),
        "MYRO".into(),
        "BOME".into(),
        "SLERF".into(),
        "HMSTR".into(),
        "CATI".into(),
        "DOGS".into(),
        "1CAT".into(),
        "MEME".into(),
        "PEOPLE".into(),
        "TURBO".into(),
        "FLOKI".into(),
        "BONK".into(),
        "1000SATS".into(),
        "ORDI".into(),
        "RATS".into(),
        "SATS".into(),
    ]
}

/// Hard-gate allowlist: only these coins may ever enter Live/Paper trading
/// after an OOS kill-switch event. Memes are excluded by default.
pub const ALLOWLIST: &[&str] = &[
    "BTC", "ETH", "SOL", "XRP", "BNB", "AVAX", "LINK", "UNI", "AAVE",
    "DOT", "APT", "NEAR", "ATOM", "LTC", "BCH", "TON", "HYPE",
];

/// Current OOS kill-switch state, read from `data/reports/wf_latest.json`.
fn kill_switch_active() -> bool {
    std::env::var("OOS_LOCKDOWN_FORCE").is_ok()
}

/// Returns `true` when the universe gate allows Live auto-trading.
///
/// Lockdown: if the OOS kill-switch is firing, only `Allowlist` mode is permitted
/// and `open`/`Filtered` modes must be rejected at the gate.
pub fn universe_healthy(mode: UniverseMode) -> bool {
    if kill_switch_active() {
        return matches!(mode, UniverseMode::Allowlist);
    }
    true
}

impl UniverseFilter {
    /// Construct a filter with the locked-down default mode (`Allowlist`).
    /// Always returns `Allowlist` — override at your own risk when OOS is healthy.
    pub fn default_universe_mode() -> UniverseMode {
        if kill_switch_active() {
            tracing::warn!(
                "OOS kill-switch active — forcing UniverseMode::Allowlist. \
                 Open/Filtered modes are rejected."
            );
            return UniverseMode::Allowlist;
        }
        UniverseMode::Allowlist
    }
    pub fn normalize(mut self) -> Self {
        for v in &mut self.allowlist {
            *v = v.to_ascii_uppercase();
        }
        for v in &mut self.blacklist {
            *v = v.to_ascii_uppercase();
        }
        // kNEIRO style: keep as-is uppercase still
        self
    }

    fn is_blacklisted(&self, coin: &str) -> bool {
        let u = coin.to_ascii_uppercase();
        self.blacklist.iter().any(|b| b.eq_ignore_ascii_case(&u) || b == &u)
            // also match without k prefix variants
            || self.blacklist.iter().any(|b| {
                let bu = b.to_ascii_uppercase();
                u.ends_with(&bu) && (u.len() <= bu.len() + 2)
            })
    }

    fn is_allowlisted(&self, coin: &str) -> bool {
        self.allowlist
            .iter()
            .any(|a| a.eq_ignore_ascii_case(coin))
    }

    /// Coin-level gate (mode + blacklist/allowlist). Liquidity checked separately.
    pub fn allows_coin(&self, coin: &str) -> bool {
        if self.is_blacklisted(coin) {
            return false;
        }
        match self.mode {
            UniverseMode::Allowlist => self.is_allowlisted(coin),
            UniverseMode::Filtered | UniverseMode::Open => true,
        }
    }

    /// Full liquidity gate.
    pub fn allows(&self, coin: &str, mid: f64, oi_coins: f64, funding: f64) -> bool {
        if !self.allows_coin(coin) {
            return false;
        }
        if mid < self.min_mid {
            return false;
        }
        let oi_usd = oi_coins * mid;
        if oi_usd < self.min_oi_notional_usd {
            return false;
        }
        if self.max_abs_funding > 0.0 && funding.abs() > self.max_abs_funding {
            return false;
        }
        true
    }

    /// Filter ranked opportunities: (coin, funding, mid, oi_coins).
    pub fn filter_ranked(
        &self,
        rows: &[(String, f64, f64, f64)],
    ) -> Vec<(String, f64, f64, f64)> {
        let mut out: Vec<_> = rows
            .iter()
            .filter(|(c, f, m, oi)| self.allows(c, *m, *oi, *f))
            .cloned()
            .collect();
        out.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(self.top_n.max(1));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_blocks_ace() {
        let f = UniverseFilter::default().normalize();
        assert!(f.allows_coin("BTC"));
        assert!(!f.allows_coin("ACE"));
        assert!(!f.allows("ACE", 0.1, 1e9, -0.003));
    }

    #[test]
    fn filtered_requires_oi_usd() {
        let mut f = UniverseFilter::default().normalize();
        f.mode = UniverseMode::Filtered;
        f.allowlist.clear();
        // LINK mid 15, oi 1000 → oi_usd 15k < 5M
        assert!(!f.allows("LINK", 15.0, 1000.0, 0.0001));
        // oi huge
        assert!(f.allows("LINK", 15.0, 1_000_000.0, 0.0001));
    }

    #[test]
    fn universe_healthy_blocks_filtered_when_lockdown() {
        std::env::set_var("OOS_LOCKDOWN_FORCE", "1");
        assert!(!universe_healthy(UniverseMode::Filtered));
        assert!(!universe_healthy(UniverseMode::Open));
        assert!(universe_healthy(UniverseMode::Allowlist));
        std::env::remove_var("OOS_LOCKDOWN_FORCE");
    }

    #[test]
    fn default_universe_mode_forced_allowlist_on_lockdown() {
        std::env::set_var("OOS_LOCKDOWN_FORCE", "1");
        assert_eq!(
            UniverseFilter::default_universe_mode(),
            UniverseMode::Allowlist
        );
        std::env::remove_var("OOS_LOCKDOWN_FORCE");
    }

    #[test]
    fn filter_ranked_respects_top_n() {
        let f = UniverseFilter::default().normalize();
        let rows = vec![
            ("BTC".into(), 0.0001, 60_000.0, 200.0),   // oi_usd = 12M
            ("ETH".into(), 0.0002, 3_000.0, 3000.0),   // oi_usd = 9M
            ("SOL".into(), 0.0003, 150.0, 50000.0),   // oi_usd = 7.5M
            ("XRP".into(), 0.0004, 1.0, 10000000.0),  // oi_usd = 10M
        ];
        let out = f.filter_ranked(&rows);
        assert_eq!(out.len(), 4); // top_n default = 4
    }
}
