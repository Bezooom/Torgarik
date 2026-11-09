//! Trade protections – the freqtrade-style guard layer that sits *before*
//! the risk gate and answers a different question:
//!
//! * `risk_gate` – "is this order too big for the book right now?"
//! * `protections` – "has this book already proved it should not trade?"
//!
//! Three independent guards, each disabled by setting its threshold to `0`:
//!
//! | Guard | Question |
//! |---|---|
//! | [`StoplossGuard`] | did `coin` stop out `max_stops` times inside the window? |
//! | [`Cooldown`] | was any position on `coin` closed too recently? |
//! | [`MaxDrawdown`] | is equity too far below the observed peak? |
//!
//! State is persisted to [`PROTECTIONS_FILE`] so a shell restart does not
//! forget a stop-out. The file is fail-soft: corrupt or unreadable state
//! degrades to "no history" and the guards fall back to their thresholds,
//! never to a panic and never to an unconditional block.
//!
//! Guards are pure functions of `(state, config, now)`; the store only owns
//! persistence and pruning.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;

/// Persisted path relative to the working directory.
pub const PROTECTIONS_FILE: &str = "runtime/protections.json";

/// Schema version of the persisted state. Bumped only on breaking changes.
pub const PROTECTIONS_VERSION: &str = "1";

/// Tunables live in `common` with the rest of `Settings` so TOML owns them.
pub use trading_common::ProtectionConfig;

/// Why an entry was refused. Serialized into the journal as `reason`.
#[derive(Debug, Clone, PartialEq)]
pub enum ProtectionBlock {
    /// `coin` hit its stop-loss budget for the current window.
    StoplossGuard {
        coin: String,
        stops: usize,
        window_hours: f64,
    },
    /// `coin` was closed recently and is still cooling down.
    Cooldown { coin: String, remaining_secs: i64 },
    /// Portfolio equity is too far below the observed peak.
    MaxDrawdown { drawdown: f64, limit: f64 },
}

impl ProtectionBlock {
    /// Stable machine-readable reason for journal + dashboard.
    pub fn reason(&self) -> &'static str {
        match self {
            ProtectionBlock::StoplossGuard { .. } => "protection_stoploss_guard",
            ProtectionBlock::Cooldown { .. } => "protection_cooldown",
            ProtectionBlock::MaxDrawdown { .. } => "protection_max_drawdown",
        }
    }
}

impl std::fmt::Display for ProtectionBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtectionBlock::StoplossGuard {
                coin,
                stops,
                window_hours,
            } => write!(
                f,
                "stoploss_guard: {coin} stopped {stops}x within {window_hours}h"
            ),
            ProtectionBlock::Cooldown {
                coin,
                remaining_secs,
            } => write!(
                f,
                "cooldown: {coin} closed recently, {remaining_secs}s left"
            ),
            ProtectionBlock::MaxDrawdown { drawdown, limit } => write!(
                f,
                "max_drawdown: {:.2}% below peak, limit {:.2}%",
                drawdown * 100.0,
                limit * 100.0
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StopEvent {
    pub coin: String,
    pub ts: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CloseEvent {
    pub coin: String,
    pub ts: DateTime<Utc>,
}

/// Persisted protection history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProtectionState {
    #[serde(default = "default_version")]
    pub version: String,
    /// Highest equity observed; `0.0` means "not observed yet".
    #[serde(default)]
    pub peak_equity: f64,
    #[serde(default)]
    pub stops: Vec<StopEvent>,
    #[serde(default)]
    pub closes: Vec<CloseEvent>,
}

fn default_version() -> String {
    PROTECTIONS_VERSION.to_string()
}

impl Default for ProtectionState {
    fn default() -> Self {
        Self {
            version: default_version(),
            peak_equity: 0.0,
            stops: Vec::new(),
            closes: Vec::new(),
        }
    }
}

impl ProtectionState {
    /// Number of stops on `coin` at or after `since`.
    fn stops_since(&self, coin: &str, since: DateTime<Utc>) -> usize {
        self.stops
            .iter()
            .filter(|e| e.coin == coin && e.ts >= since)
            .count()
    }

    /// Most recent close on `coin`, if any.
    fn last_close(&self, coin: &str) -> Option<DateTime<Utc>> {
        self.closes
            .iter()
            .filter(|e| e.coin == coin)
            .map(|e| e.ts)
            .max()
    }

    /// Evaluate every guard for an entry on `coin` at `now`.
    pub fn check(
        &self,
        coin: &str,
        now: DateTime<Utc>,
        cfg: &ProtectionConfig,
    ) -> Result<(), ProtectionBlock> {
        if let Some(window) = cfg.stoploss_window() {
            let since = now - window;
            let stops = self.stops_since(coin, since);
            if stops >= cfg.stoploss_guard_max_stops {
                return Err(ProtectionBlock::StoplossGuard {
                    coin: coin.to_string(),
                    stops,
                    window_hours: cfg.stoploss_guard_window_hours,
                });
            }
        }

        if let Some(cooldown) = cfg.cooldown() {
            if let Some(last) = self.last_close(coin) {
                let until = last + cooldown;
                if now < until {
                    return Err(ProtectionBlock::Cooldown {
                        coin: coin.to_string(),
                        remaining_secs: (until - now).num_seconds().max(0),
                    });
                }
            }
        }

        // The drawdown guard needs current equity, so it lives in
        // `check_equity` and is composed by `ProtectionStore::check_entry`.
        Ok(())
    }

    /// Drawdown guard, evaluated separately because it needs current equity.
    pub fn check_equity(&self, equity: f64, cfg: &ProtectionConfig) -> Result<(), ProtectionBlock> {
        if cfg.max_drawdown_frac <= 0.0 || self.peak_equity <= 0.0 {
            return Ok(());
        }
        if !equity.is_finite() || equity <= 0.0 {
            // Bad equity is the risk gate's job, not a drawdown verdict.
            return Ok(());
        }
        let drawdown = (self.peak_equity - equity) / self.peak_equity;
        if drawdown >= cfg.max_drawdown_frac {
            return Err(ProtectionBlock::MaxDrawdown {
                drawdown,
                limit: cfg.max_drawdown_frac,
            });
        }
        Ok(())
    }

    fn prune(&mut self, now: DateTime<Utc>, cfg: &ProtectionConfig) {
        let stop_cut = cfg
            .stoploss_window()
            .map(|w| now - w)
            .unwrap_or(now - Duration::try_days(30).unwrap_or_else(Duration::zero));
        let close_cut = cfg
            .cooldown()
            .map(|c| now - c * 4)
            .unwrap_or(now - Duration::try_days(30).unwrap_or_else(Duration::zero));
        self.stops.retain(|e| e.ts >= stop_cut);
        self.closes.retain(|e| e.ts >= close_cut);
    }
}

/// Persistent protection history bound to a file.
#[derive(Debug, Clone)]
pub struct ProtectionStore {
    path: PathBuf,
    state: ProtectionState,
}

impl ProtectionStore {
    /// Load from `path`; missing or corrupt file yields empty state.
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let state = match std::fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<ProtectionState>(&raw) {
                Ok(st) if st.version == PROTECTIONS_VERSION => st,
                Ok(st) => {
                    warn!(
                        "protections: version {} != {} – starting empty",
                        st.version, PROTECTIONS_VERSION
                    );
                    ProtectionState::default()
                }
                Err(e) => {
                    warn!(
                        "protections: corrupt {}: {e} – starting empty",
                        path.display()
                    );
                    ProtectionState::default()
                }
            },
            Err(_) => ProtectionState::default(),
        };
        Self { path, state }
    }

    /// In-memory store with no file (tests, dry tooling).
    pub fn in_memory() -> Self {
        Self {
            path: PathBuf::new(),
            state: ProtectionState::default(),
        }
    }

    pub fn state(&self) -> &ProtectionState {
        &self.state
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Record a stop-loss trigger on `coin`.
    pub fn record_stop(&mut self, coin: &str, ts: DateTime<Utc>) {
        self.state.stops.push(StopEvent {
            coin: coin.to_string(),
            ts,
        });
        self.persist();
    }

    /// Record any position close on `coin` (SL, TP, manual flat).
    pub fn record_close(&mut self, coin: &str, ts: DateTime<Utc>) {
        self.state.closes.push(CloseEvent {
            coin: coin.to_string(),
            ts,
        });
        self.persist();
    }

    /// Drop events older than the configured windows. Call once per shell
    /// cycle: recording never prunes, so a guard can never lose data it
    /// still needs because a newer event happened to arrive first.
    pub fn prune_at(&mut self, now: DateTime<Utc>, cfg: &ProtectionConfig) {
        let before = self.state.stops.len() + self.state.closes.len();
        self.state.prune(now, cfg);
        if self.state.stops.len() + self.state.closes.len() != before {
            self.persist();
        }
    }

    /// Feed current equity; raises the peak, never lowers it.
    pub fn observe_equity(&mut self, equity: f64) {
        if !equity.is_finite() || equity <= 0.0 {
            return;
        }
        if equity > self.state.peak_equity {
            self.state.peak_equity = equity;
            self.persist();
        }
    }

    /// Entry gate: per-coin guards, then the drawdown guard for `equity`.
    pub fn check_entry(
        &self,
        coin: &str,
        now: DateTime<Utc>,
        equity: f64,
        cfg: &ProtectionConfig,
    ) -> Result<(), ProtectionBlock> {
        if cfg.is_empty() {
            return Ok(());
        }
        self.state.check(coin, now, cfg)?;
        self.state.check_equity(equity, cfg)
    }

    /// True when the drawdown guard currently blocks every entry.
    pub fn drawdown_blocked(&self, equity: f64, cfg: &ProtectionConfig) -> bool {
        self.state.check_equity(equity, cfg).is_err()
    }

    fn persist(&self) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&self.state) {
            Ok(body) => {
                if let Err(e) = std::fs::write(&self.path, body) {
                    warn!("protections: write {}: {e}", self.path.display());
                }
            }
            Err(e) => warn!("protections: serialize: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(minutes: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).expect("valid ts") + Duration::minutes(minutes)
    }

    fn cfg() -> ProtectionConfig {
        ProtectionConfig {
            stoploss_guard_max_stops: 2,
            stoploss_guard_window_hours: 24.0,
            cooldown_hours: 4.0,
            max_drawdown_frac: 0.10,
        }
    }

    #[test]
    fn stoploss_guard_blocks_after_threshold_and_expires_with_window() {
        let mut store = ProtectionStore::in_memory();
        let now = ts(0);
        store.record_stop("ETH", now - Duration::hours(1));
        assert!(store.check_entry("ETH", now, 10_000.0, &cfg()).is_ok());

        store.record_stop("ETH", now - Duration::minutes(10));
        let err = store.check_entry("ETH", now, 10_000.0, &cfg()).unwrap_err();
        assert!(matches!(
            err,
            ProtectionBlock::StoplossGuard { stops: 2, .. }
        ));

        // 25h later the stops fall out of the 24h window.
        let later = now + Duration::hours(25);
        assert!(store.check_entry("ETH", later, 10_000.0, &cfg()).is_ok());
    }

    #[test]
    fn stoploss_guard_is_per_coin() {
        let mut store = ProtectionStore::in_memory();
        let now = ts(0);
        store.record_stop("ETH", now);
        store.record_stop("ETH", now);
        assert!(store.check_entry("ETH", now, 10_000.0, &cfg()).is_err());
        assert!(store.check_entry("BTC", now, 10_000.0, &cfg()).is_ok());
    }

    #[test]
    fn cooldown_blocks_then_releases() {
        let mut store = ProtectionStore::in_memory();
        let now = ts(0);
        store.record_close("BTC", now);
        let err = store.check_entry("BTC", now, 10_000.0, &cfg()).unwrap_err();
        match err {
            ProtectionBlock::Cooldown { remaining_secs, .. } => {
                assert_eq!(remaining_secs, 4 * 3600);
            }
            other => panic!("expected cooldown, got {other:?}"),
        }
        assert!(store
            .check_entry("BTC", now + Duration::hours(4), 10_000.0, &cfg())
            .is_ok());
    }

    #[test]
    fn drawdown_guard_blocks_all_entries_below_peak() {
        let mut store = ProtectionStore::in_memory();
        let now = ts(0);
        store.observe_equity(10_000.0);
        assert!(store.check_entry("ETH", now, 9_500.0, &cfg()).is_ok());

        let err = store.check_entry("ETH", now, 8_900.0, &cfg()).unwrap_err();
        match err {
            ProtectionBlock::MaxDrawdown { drawdown, limit } => {
                assert!((drawdown - 0.11).abs() < 1e-9, "{drawdown}");
                assert!((limit - 0.10).abs() < 1e-9);
            }
            other => panic!("expected drawdown, got {other:?}"),
        }
        // Peak is not lowered by a lower reading.
        store.observe_equity(8_000.0);
        assert_eq!(store.state().peak_equity, 10_000.0);
    }

    #[test]
    fn disabled_config_never_blocks() {
        let mut store = ProtectionStore::in_memory();
        let now = ts(0);
        store.observe_equity(10_000.0);
        store.record_stop("ETH", now);
        store.record_stop("ETH", now);
        store.record_close("ETH", now);
        let off = ProtectionConfig {
            stoploss_guard_max_stops: 0,
            stoploss_guard_window_hours: 0.0,
            cooldown_hours: 0.0,
            max_drawdown_frac: 0.0,
        };
        assert!(off.is_empty());
        assert!(store.check_entry("ETH", now, 1.0, &off).is_ok());
    }

    #[test]
    fn persisted_state_survives_reload() {
        let dir = std::env::temp_dir().join(format!("prot-{}", uuid_like()));
        let path = dir.join("protections.json");
        let now = ts(0);
        {
            let mut store = ProtectionStore::load(&path);
            store.observe_equity(12_000.0);
            store.record_stop("ETH", now);
            store.record_stop("ETH", now);
        }
        let reloaded = ProtectionStore::load(&path);
        assert_eq!(reloaded.state().peak_equity, 12_000.0);
        assert!(matches!(
            reloaded.check_entry("ETH", now, 12_000.0, &cfg()),
            Err(ProtectionBlock::StoplossGuard { .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_fails_soft_to_empty_state() {
        let dir = std::env::temp_dir().join(format!("prot-bad-{}", uuid_like()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("protections.json");
        std::fs::write(&path, b"{not json").expect("write");
        let store = ProtectionStore::load(&path);
        assert!(store.state().stops.is_empty());
        assert!(store.check_entry("ETH", ts(0), 10_000.0, &cfg()).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_at_drops_events_outside_configured_windows() {
        let dir = std::env::temp_dir().join(format!("prot-prune-{}", uuid_like()));
        let path = dir.join("protections.json");
        let now = ts(0);
        let mut store = ProtectionStore::load(&path);
        store.record_stop("ETH", now - Duration::days(40));
        store.record_close("BTC", now - Duration::days(40));
        store.record_stop("ETH", now - Duration::minutes(5));
        assert_eq!(store.state().stops.len(), 2, "recording never prunes");

        store.prune_at(now, &cfg());
        assert_eq!(store.state().stops.len(), 1, "40d-old stop pruned");
        assert!(store.state().closes.is_empty(), "40d-old close pruned");

        let reloaded = ProtectionStore::load(&path);
        assert_eq!(reloaded.state().stops.len(), 1, "prune persisted");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Cheap unique suffix without pulling `uuid` into this crate.
    fn uuid_like() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{nanos}-{}", std::process::id())
    }
}
