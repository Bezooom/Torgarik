//! Locally managed bracket orders (SL / TP) for the risk shell.
//!
//! Brackets are **shell-local**: no venue trigger orders are used. After an
//! entry signal with SL/TP parameters passes all gates, a [`Bracket`] is
//! registered in memory and persisted to `runtime/brackets.json` so it
//! survives shell restarts. Each cycle the monitor checks open positions
//! against stored levels and closes via the normal reduce-only IOC path
//! (`check_order` → dry-run or `place_order`).
//!
//! One bracket per coin; re-registration replaces the previous one.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;

use trading_common::OrderSide;

/// Persisted path relative to the working directory.
pub const BRACKETS_FILE: &str = "runtime/brackets.json";

/// Raw optional bracket parameters carried by an external signal line.
#[derive(Debug, Clone, Copy, Default)]
pub struct BracketParams {
    /// Fractional stop-loss distance from entry (0.02 = 2%).
    pub sl_pct: Option<f64>,
    /// Fractional take-profit distance from entry (0.05 = 5%).
    pub tp_pct: Option<f64>,
    /// Absolute stop-loss price.
    pub sl_price: Option<f64>,
    /// Absolute take-profit price.
    pub tp_price: Option<f64>,
}

impl BracketParams {
    /// True when no bracket was requested at all (plain entry).
    pub fn is_empty(&self) -> bool {
        self.sl_pct.is_none()
            && self.tp_pct.is_none()
            && self.sl_price.is_none()
            && self.tp_price.is_none()
    }

    /// Validate raw values: pct must be finite in `(0, 0.5]`, price finite `> 0`.
    pub fn validate(&self) -> Result<(), String> {
        for (name, v) in [("sl_pct", self.sl_pct), ("tp_pct", self.tp_pct)] {
            if let Some(v) = v {
                if !v.is_finite() || v <= 0.0 || v > 0.5 {
                    return Err(format!("{name}={v} not in (0, 0.5]"));
                }
            }
        }
        for (name, v) in [
            ("sl_price", self.sl_price),
            ("tp_price", self.tp_price),
        ] {
            if let Some(v) = v {
                if !v.is_finite() || v <= 0.0 {
                    return Err(format!("{name}={v} must be finite > 0"));
                }
            }
        }
        Ok(())
    }
}

/// Concrete resolved levels (either may be absent → that leg never triggers).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BracketLevels {
    pub sl_price: Option<f64>,
    pub tp_price: Option<f64>,
}

/// Resolve raw parameters into concrete levels for `side` at `entry`.
///
/// - Buy (long):  sl = entry * (1 - sl_pct), tp = entry * (1 + tp_pct)
/// - Sell (short): sl = entry * (1 + sl_pct), tp = entry * (1 - tp_pct)
/// - Absolute prices are taken as-is and take precedence over pcts.
/// - `Ok(None)` when no bracket was requested.
/// - `Err` on invalid values (order must NOT be placed then).
pub fn build_levels(
    side: OrderSide,
    entry: f64,
    params: BracketParams,
) -> Result<Option<BracketLevels>, String> {
    if params.is_empty() {
        return Ok(None);
    }
    params.validate()?;
    if !entry.is_finite() || entry <= 0.0 {
        return Err(format!("entry={entry} must be finite > 0"));
    }

    let leg = |price: Option<f64>, pct: Option<f64>, dir: f64| match price {
        Some(p) => Some(p),
        None => pct.map(|pct| entry * (1.0 + dir * pct)),
    };
    // Long: SL below (+dir = -1), TP above; short mirrored.
    let sl_dir = match side {
        OrderSide::Buy => -1.0,
        OrderSide::Sell => 1.0,
    };
    let sl_price = leg(params.sl_price, params.sl_pct, sl_dir);
    let tp_price = leg(params.tp_price, params.tp_pct, -sl_dir);
    Ok(Some(BracketLevels { sl_price, tp_price }))
}

/// Which bracket leg fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BracketAction {
    StopLoss,
    TakeProfit,
}

impl BracketAction {
    pub fn as_str(self) -> &'static str {
        match self {
            BracketAction::StopLoss => "trigger_sl",
            BracketAction::TakeProfit => "trigger_tp",
        }
    }
}

/// Pure trigger decision at current `mid`. Stop-loss is checked first
/// (conservative priority); absent legs never trigger.
pub fn trigger_for(
    side: OrderSide,
    mid: f64,
    sl_price: Option<f64>,
    tp_price: Option<f64>,
) -> Option<BracketAction> {
    if !mid.is_finite() || mid <= 0.0 {
        return None;
    }
    let hit = |level: Option<f64>, when_below: bool| match level {
        Some(lv) if lv.is_finite() => {
            if when_below {
                mid <= lv
            } else {
                mid >= lv
            }
        }
        _ => false,
    };
    // Long SL fires when price falls to the level; long TP when it rises.
    // Short is mirrored.
    let sl_when_below = matches!(side, OrderSide::Buy);
    if hit(sl_price, sl_when_below) {
        Some(BracketAction::StopLoss)
    } else if hit(tp_price, !sl_when_below) {
        Some(BracketAction::TakeProfit)
    } else {
        None
    }
}

/// A registered bracket for an open entry position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bracket {
    pub coin: String,
    /// Entry direction (Buy = long bracket, Sell = short bracket).
    pub side: OrderSide,
    /// Requested entry size in coin units (informational; closes use live pos size).
    pub size: f64,
    /// quote.mid at registration; updated to ack.avg_price on a known live fill.
    pub entry_price: f64,
    #[serde(default)]
    pub sl_price: Option<f64>,
    #[serde(default)]
    pub tp_price: Option<f64>,
    pub created_ts: DateTime<Utc>,
}

impl Bracket {
    /// Representative level for journal events without a specific trigger.
    pub fn repr_level(&self) -> f64 {
        self.sl_price
            .or(self.tp_price)
            .unwrap_or(self.entry_price)
    }
}

/// In-memory bracket storage persisted to a single JSON file.
///
/// Fail-soft: missing file → empty store; broken file → warn + empty store.
/// Writes are atomic (tmp file + rename).
pub struct BracketStore {
    path: PathBuf,
    brackets: Vec<Bracket>,
}

impl BracketStore {
    /// Load brackets from `path`, tolerating missing/corrupt files.
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut brackets: Vec<Bracket> = Vec::new();
        match std::fs::read_to_string(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!("brackets read failed ({}): {e}", path.display()),
            Ok(body) if body.trim().is_empty() => {}
            Ok(body) => match serde_json::from_str::<Vec<Bracket>>(&body) {
                Ok(list) => {
                    // Dedupe by coin keeping the last entry (one bracket per coin).
                    for b in list {
                        if b.coin.is_empty() {
                            continue;
                        }
                        brackets.retain(|x| x.coin != b.coin);
                        brackets.push(b);
                    }
                }
                Err(e) => warn!(
                    "brackets parse failed ({}) – starting empty: {e}",
                    path.display()
                ),
            },
        }
        Self { path, brackets }
    }

    pub fn list(&self) -> &[Bracket] {
        &self.brackets
    }

    pub fn get(&self, coin: &str) -> Option<&Bracket> {
        self.brackets.iter().find(|b| b.coin == coin)
    }

    /// Insert or replace (one bracket per coin), then persist atomically.
    pub fn upsert(&mut self, bracket: Bracket) {
        let coin = bracket.coin.clone();
        self.brackets.retain(|b| b.coin != coin);
        self.brackets.push(bracket);
        self.save();
    }

    /// Remove by coin. Returns true when something was removed; persists on change.
    pub fn remove(&mut self, coin: &str) -> bool {
        let before = self.brackets.len();
        self.brackets.retain(|b| b.coin != coin);
        let removed = self.brackets.len() != before;
        if removed {
            self.save();
        }
        removed
    }

    /// Drop brackets that no longer have a matching open position (same coin,
    /// same direction, non-zero size). Call once on the first portfolio
    /// snapshot after a restart: without it a bracket can outlive the position
    /// it was protecting and later "close" a position that was opened by hand.
    ///
    /// Returns the coins whose brackets were dropped, in file order.
    pub fn reconcile(&mut self, positions: &[trading_common::Position]) -> Vec<String> {
        let mut removed = Vec::new();
        self.brackets.retain(|b| {
            let keep = positions
                .iter()
                .any(|p| p.instrument_id == b.coin && p.side == b.side && p.size > 0.0);
            if !keep {
                removed.push(b.coin.clone());
            }
            keep
        });
        if !removed.is_empty() {
            self.save();
        }
        removed
    }

    /// Atomic write: serialize → tmp file → rename. Fail-soft (warn only):
    /// the journal keeps an audit trail even if persistence breaks.
    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                warn!("brackets mkdir {}: {e}", parent.display());
                return;
            }
        }
        let body = match serde_json::to_string_pretty(&self.brackets) {
            Ok(b) => b,
            Err(e) => {
                warn!("brackets serialize: {e}");
                return;
            }
        };
        let tmp = self.path.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, body.as_bytes()) {
            warn!("brackets write {}: {e}", tmp.display());
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &self.path) {
            warn!("brackets rename {}→{}: {e}", tmp.display(), self.path.display());
        }
    }
}

/// Convenience: default store location under the repo working directory.
pub fn default_store() -> BracketStore {
    BracketStore::load(Path::new(BRACKETS_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-9;

    fn params(sl_pct: Option<f64>, tp_pct: Option<f64>) -> BracketParams {
        BracketParams { sl_pct, tp_pct, ..Default::default() }
    }

    fn bracket(coin: &str, side: OrderSide) -> Bracket {
        Bracket {
            coin: coin.to_string(),
            side,
            size: 1.0,
            entry_price: 100.0,
            sl_price: Some(98.0),
            tp_price: Some(105.0),
            created_ts: Utc::now(),
        }
    }

    fn position(coin: &str, side: OrderSide, size: f64) -> trading_common::Position {
        trading_common::Position {
            instrument_id: coin.to_string(),
            side,
            size,
            entry_price: 100.0,
            unrealized_pnl: 0.0,
            timestamp: Utc::now(),
            market_id: None,
            outcome_id: None,
        }
    }

    #[test]
    fn reconcile_keeps_only_matching_positions() {
        let dir = std::env::temp_dir().join(format!(
            "bracket-reconcile-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let path = dir.join("brackets.json");
        let mut store = BracketStore::load(&path);
        store.upsert(bracket("ETH", OrderSide::Buy));
        store.upsert(bracket("BTC", OrderSide::Buy));
        store.upsert(bracket("SOL", OrderSide::Sell));

        let removed = store.reconcile(&[
            // ETH long still open → keep.
            position("ETH", OrderSide::Buy, 1.0),
            // BTC long closed → drop.
            position("BTC", OrderSide::Buy, 0.0),
            // SOL flipped short → long bracket no longer matches → drop.
            position("SOL", OrderSide::Buy, 1.0),
        ]);

        assert_eq!(removed, vec!["BTC".to_string(), "SOL".to_string()]);
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].coin, "ETH");
        // Persisted: a reload sees the reconciled set, not the stale one.
        let reloaded = BracketStore::load(&path);
        assert_eq!(reloaded.list().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_levels_from_pcts() {
        let lv = build_levels(OrderSide::Buy, 100.0, params(Some(0.02), Some(0.05)))
            .unwrap()
            .unwrap();
        assert!((lv.sl_price.unwrap() - 98.0).abs() < EPS);
        assert!((lv.tp_price.unwrap() - 105.0).abs() < EPS);
    }

    #[test]
    fn short_levels_mirrored() {
        let lv = build_levels(OrderSide::Sell, 100.0, params(Some(0.02), Some(0.05)))
            .unwrap()
            .unwrap();
        assert!((lv.sl_price.unwrap() - 102.0).abs() < EPS);
        assert!((lv.tp_price.unwrap() - 95.0).abs() < EPS);
    }

    #[test]
    fn absolute_prices_taken_as_is_and_override_pcts() {
        let p = BracketParams {
            sl_pct: Some(0.01),
            tp_pct: Some(0.01),
            sl_price: Some(90.0),
            tp_price: Some(120.0),
        };
        let lv = build_levels(OrderSide::Buy, 100.0, p).unwrap().unwrap();
        assert_eq!(lv.sl_price, Some(90.0));
        assert_eq!(lv.tp_price, Some(120.0));
    }

    #[test]
    fn partial_bracket_single_leg() {
        let lv = build_levels(OrderSide::Buy, 50.0, params(None, Some(0.10)))
            .unwrap()
            .unwrap();
        assert!(lv.sl_price.is_none());
        assert!((lv.tp_price.unwrap() - 55.0).abs() < EPS);
    }

    #[test]
    fn empty_params_means_no_bracket() {
        assert!(build_levels(OrderSide::Buy, 100.0, Default::default())
            .unwrap()
            .is_none());
    }

    #[test]
    fn validation_rejects_bad_values() {
        assert!(build_levels(OrderSide::Buy, 100.0, params(Some(0.0), None)).is_err());
        assert!(build_levels(OrderSide::Buy, 100.0, params(Some(-0.02), None)).is_err());
        assert!(build_levels(OrderSide::Buy, 100.0, params(Some(0.51), None)).is_err());
        assert!(build_levels(OrderSide::Buy, 100.0, params(Some(f64::NAN), None)).is_err());
        // Boundary 0.5 is allowed.
        assert!(build_levels(OrderSide::Buy, 100.0, params(Some(0.5), Some(0.5))).is_ok());

        let bad_price = BracketParams { sl_price: Some(0.0), ..Default::default() };
        assert!(build_levels(OrderSide::Sell, 100.0, bad_price).is_err());
        let neg = BracketParams { tp_price: Some(-3.0), ..Default::default() };
        assert!(build_levels(OrderSide::Sell, 100.0, neg).is_err());
        let nan = BracketParams { tp_price: Some(f64::NAN), ..Default::default() };
        assert!(build_levels(OrderSide::Sell, 100.0, nan).is_err());
        let inf = BracketParams { sl_price: Some(f64::INFINITY), ..Default::default() };
        assert!(build_levels(OrderSide::Buy, 100.0, inf).is_err());

        // Bad entry rejected too.
        assert!(build_levels(OrderSide::Buy, 0.0, params(Some(0.02), None)).is_err());
    }

    #[test]
    fn triggers_long_side() {
        let (sl, tp) = (Some(98.0_f64), Some(105.0_f64));
        assert_eq!(trigger_for(OrderSide::Buy, 97.99, sl, tp), Some(BracketAction::StopLoss));
        assert_eq!(trigger_for(OrderSide::Buy, 98.0, sl, tp), Some(BracketAction::StopLoss));
        assert_eq!(trigger_for(OrderSide::Buy, 100.0, sl, tp), None);
        assert_eq!(trigger_for(OrderSide::Buy, 105.0, sl, tp), Some(BracketAction::TakeProfit));
        assert_eq!(trigger_for(OrderSide::Buy, 200.0, sl, tp), Some(BracketAction::TakeProfit));
        assert_eq!(trigger_for(OrderSide::Buy, 0.0, sl, tp), None);
        assert_eq!(trigger_for(OrderSide::Buy, f64::NAN, sl, tp), None);
        // Missing legs never fire.
        assert_eq!(trigger_for(OrderSide::Buy, 1.0, None, tp), None);
        assert_eq!(trigger_for(OrderSide::Buy, 999.0, sl, None), None);
    }

    #[test]
    fn triggers_short_side() {
        let (sl, tp) = (Some(102.0_f64), Some(95.0_f64));
        assert_eq!(trigger_for(OrderSide::Sell, 102.0, sl, tp), Some(BracketAction::StopLoss));
        assert_eq!(trigger_for(OrderSide::Sell, 150.0, sl, tp), Some(BracketAction::StopLoss));
        assert_eq!(trigger_for(OrderSide::Sell, 96.0, sl, tp), None);
        assert_eq!(trigger_for(OrderSide::Sell, 95.0, sl, tp), Some(BracketAction::TakeProfit));
        assert_eq!(trigger_for(OrderSide::Sell, 10.0, sl, tp), Some(BracketAction::TakeProfit));
    }

    #[test]
    fn stop_loss_wins_when_both_hit() {
        assert_eq!(
            trigger_for(OrderSide::Buy, 90.0, Some(98.0), Some(105.0)),
            Some(BracketAction::StopLoss)
        );
    }

    fn sample_bracket(coin: &str, side: OrderSide) -> Bracket {
        Bracket {
            coin: coin.into(),
            side,
            size: 0.5,
            entry_price: 100.0,
            sl_price: Some(98.0),
            tp_price: Some(105.0),
            created_ts: Utc::now(),
        }
    }

    #[test]
    fn store_roundtrip_survives_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("brackets.json");

        let created;
        {
            let mut store = BracketStore::load(&path);
            store.upsert(sample_bracket("ETH", OrderSide::Buy));
            store.upsert(sample_bracket("SOL", OrderSide::Sell));
            created = store.get("ETH").cloned().unwrap();
        }
        let store = BracketStore::load(&path);
        assert_eq!(store.list().len(), 2);
        let eth = store.get("ETH").unwrap();
        assert_eq!(eth.side, OrderSide::Buy);
        assert_eq!(eth.entry_price, created.entry_price);
        assert_eq!(eth.sl_price, Some(98.0));

        // Atomic write leaves no stray tmp file behind.
        assert!(!dir.path().join("brackets.json.tmp").exists());
    }

    #[test]
    fn store_one_bracket_per_coin() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("brackets.json");
        let mut store = BracketStore::load(&path);
        store.upsert(sample_bracket("ETH", OrderSide::Buy));
        let mut replaced = sample_bracket("ETH", OrderSide::Buy);
        replaced.entry_price = 123.4;
        replaced.tp_price = None;
        store.upsert(replaced);

        assert_eq!(store.list().len(), 1);
        assert!((store.get("ETH").unwrap().entry_price - 123.4).abs() < EPS);
        assert!(store.get("ETH").unwrap().tp_price.is_none());
    }

    #[test]
    fn store_remove_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("brackets.json");
        let mut store = BracketStore::load(&path);
        store.upsert(sample_bracket("BTC", OrderSide::Buy));
        assert!(store.remove("BTC"));
        assert!(!store.remove("BTC"));
        assert!(store.list().is_empty());
        // Reload sees removal too.
        let store = BracketStore::load(&path);
        assert!(store.list().is_empty());
    }

    #[test]
    fn store_missing_file_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = BracketStore::load(dir.path().join("nope.json"));
        assert!(store.list().is_empty());
    }

    #[test]
    fn store_broken_file_warns_and_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("brackets.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        let store = BracketStore::load(&path);
        assert!(store.list().is_empty());
        // Store still usable afterwards (overwrites broken file on save).
        let mut store = BracketStore::load(&path);
        store.upsert(sample_bracket("ETH", OrderSide::Buy));
        let reloaded = BracketStore::load(&path);
        assert_eq!(reloaded.list().len(), 1);
    }

    #[test]
    fn store_dedupes_duplicate_coins_in_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("brackets.json");
        let mut first = sample_bracket("ETH", OrderSide::Buy);
        first.entry_price = 1.0;
        let second = sample_bracket("ETH", OrderSide::Buy);
        let body = serde_json::to_string(&vec![first, second]).unwrap();
        std::fs::write(&path, body).unwrap();
        let store = BracketStore::load(&path);
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.get("ETH").unwrap().entry_price, 100.0);
    }

    #[test]
    fn bracket_serde_omits_optional_legs_backward_compat() {
        // A bracket written without optional legs reads back as None.
        let raw = r#"{"coin":"ETH","side":"Buy","size":0.5,"entry_price":100.0,"created_ts":"2026-01-01T00:00:00Z"}"#;
        let b: Bracket = serde_json::from_str(raw).unwrap();
        assert!(b.sl_price.is_none() && b.tp_price.is_none());
    }
}
