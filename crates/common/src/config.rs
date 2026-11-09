//! Application configuration loaded from file, env, and CLI.
//!
//! Layered config: `Settings::load()` merges defaults → file → env.

use std::env;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::types::VenueKind;

// ── OOS kill-switch thresholds ──────────────────────────────────────────
// S1: live auto-alpha locked when avg Sharpe OR avg PnL is red (OR, not AND).
// Holdout red (schema 1.1) also locks.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OosSettings {
    #[serde(default = "default_oos_sharpe_floor")]
    pub sharpe_floor: f64,
    /// Minimum deflated Sharpe (selection-bias corrected). `<= 0` blocks; the
    /// check is skipped when the report predates the field.
    #[serde(default)]
    pub deflated_sharpe_floor: f64,
    /// Maximum tolerated probability of backtest overfitting. `>= 1.0` disables.
    #[serde(default = "default_max_pbo")]
    pub max_pbo: f64,
    #[serde(default = "default_oos_avg_pnl_floor")]
    pub avg_pnl_floor: f64,
    /// Minimum fraction of OOS folds that must be individually profitable.
    /// Below this, even if avg Sharpe/PnL are nominally positive, we stay locked.
    #[serde(default = "default_min_profitable_fold_ratio")]
    pub min_profitable_fold_ratio: f64,
}

fn default_max_pbo() -> f64 {
    0.5
}

impl Default for OosSettings {
    fn default() -> Self {
        Self {
            sharpe_floor: default_oos_sharpe_floor(),
            deflated_sharpe_floor: 0.0,
            max_pbo: default_max_pbo(),
            avg_pnl_floor: default_oos_avg_pnl_floor(),
            min_profitable_fold_ratio: default_min_profitable_fold_ratio(),
        }
    }
}

fn default_oos_sharpe_floor() -> f64 {
    0.0
}
fn default_oos_avg_pnl_floor() -> f64 {
    0.0
}
fn default_min_profitable_fold_ratio() -> f64 {
    0.0
}

/// Result of reading the latest walk-forward OOS report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OosStats {
    pub avg_pnl: f64,
    pub avg_sharpe: f64,
    pub n_folds: usize,
    /// Number of OOS folds with positive PnL.
    pub profitable_folds: usize,
    /// Holdout blocked (schema 1.1): true if holdout pnl/sharpe red.
    pub holdout_blocking: bool,
    /// Worst fold deflated Sharpe from the report, when present.
    pub deflated_sharpe: Option<f64>,
    /// Mean probability of backtest overfitting, when present.
    pub pbo: Option<f64>,
}

impl OosStats {
    /// Construct stats for tests / MCP without touching disk.
    pub fn new(avg_pnl: f64, avg_sharpe: f64, n_folds: usize, profitable_folds: usize) -> Self {
        Self {
            avg_pnl,
            avg_sharpe,
            n_folds,
            profitable_folds,
            holdout_blocking: false,
            deflated_sharpe: None,
            pbo: None,
        }
    }

    /// Default report path (override with env `WF_REPORT_PATH`).
    pub fn default_report_path() -> PathBuf {
        if let Ok(p) = env::var("WF_REPORT_PATH") {
            if !p.is_empty() {
                return PathBuf::from(p);
            }
        }
        PathBuf::from("data/reports/wf_latest.json")
    }

    /// Resolve report path: env → cwd relative → walk parents.
    pub fn resolve_report_path() -> Option<PathBuf> {
        let path = Self::default_report_path();
        if path.exists() {
            return Some(path);
        }
        std::env::current_dir().ok().and_then(|cwd| {
            let mut p = cwd;
            for _ in 0..8 {
                let candidate = p.join("data/reports/wf_latest.json");
                if candidate.exists() {
                    return Some(candidate);
                }
                if !p.pop() {
                    break;
                }
            }
            None
        })
    }

    /// Read default / env report path.
    pub fn load() -> Option<Self> {
        Self::load_from_path(Self::resolve_report_path()?)
    }

    /// Parse OOS metrics from an explicit report file.
    pub fn load_from_path(path: impl AsRef<std::path::Path>) -> Option<Self> {
        let raw = std::fs::read_to_string(path.as_ref()).ok()?;
        let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
        Self::from_json(&parsed)
    }

    /// Parse from already-loaded JSON (schema 1.0 / 1.1).
    pub fn from_json(parsed: &serde_json::Value) -> Option<Self> {
        let holdout_blocking = parsed.get("holdout").map(holdout_is_red).unwrap_or(false);

        // Prefer top-level aggregates when present (wf_latest / schema 1.1).
        let top_sharpe = parsed.get("test_avg_sharpe").and_then(|v| v.as_f64());
        let top_total_pnl = parsed.get("test_total_pnl").and_then(|v| v.as_f64());
        let top_profitable = parsed
            .get("profitable_test_folds")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        let folds = parsed
            .get("folds")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut pnls = Vec::new();
        let mut sharpes = Vec::new();
        for f in &folds {
            if let Some(test) = f.get("test") {
                if let Some(p) = test.get("total_pnl").and_then(|v| v.as_f64()) {
                    pnls.push(p);
                }
                if let Some(s) = test.get("sharpe_ratio").and_then(|v| v.as_f64()) {
                    sharpes.push(s);
                }
            }
        }

        let (avg_pnl, avg_sharpe, n_folds, profitable_folds) =
            if !pnls.is_empty() && !sharpes.is_empty() {
                let n = pnls.len();
                let fold_avg_pnl = pnls.iter().sum::<f64>() / n as f64;
                let fold_avg_sharpe = if let Some(s) = top_sharpe {
                    s
                } else {
                    sharpes.iter().sum::<f64>() / sharpes.len() as f64
                };
                let profitable =
                    top_profitable.unwrap_or_else(|| pnls.iter().filter(|p| **p > 0.0).count());
                (fold_avg_pnl, fold_avg_sharpe, n, profitable)
            } else if let (Some(s), Some(p)) = (top_sharpe, top_total_pnl) {
                // No per-fold rows – use top-level only.
                let n = parsed.get("n_folds").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
                (p, s, n.max(1), top_profitable.unwrap_or(0))
            } else {
                return None;
            };

        // Require a minimal sample when only folds are used without top-level.
        if top_sharpe.is_none() && top_total_pnl.is_none() && n_folds < 3 {
            return None;
        }

        // Selection-bias verdicts: prefer the worst fold, which is what a
        // "can I trust this" question actually asks.
        let deflated_sharpe = parsed
            .get("deflated_sharpe_min")
            .and_then(|v| v.as_f64())
            .or_else(|| {
                let vals: Vec<f64> = folds
                    .iter()
                    .filter_map(|f| f.get("deflated_sharpe").and_then(|v| v.as_f64()))
                    .filter(|d| d.is_finite())
                    .collect();
                if vals.is_empty() {
                    None
                } else {
                    Some(vals.iter().copied().fold(f64::INFINITY, f64::min))
                }
            });
        let pbo = parsed.get("pbo_avg").and_then(|v| v.as_f64());

        Some(OosStats {
            avg_pnl,
            avg_sharpe,
            n_folds,
            profitable_folds,
            holdout_blocking,
            deflated_sharpe,
            pbo,
        })
    }

    /// True if OOS metrics are below kill-switch thresholds.
    ///
    /// Fires when avg Sharpe < 0 **OR** avg PnL < 0, or holdout is red, or fold-ratio gate.
    pub fn is_blocking(&self) -> bool {
        self.is_blocking_with_settings(&OosSettings::default())
    }

    /// True if OOS metrics are below configured kill-switch thresholds.
    pub fn is_blocking_with_settings(&self, oos: &OosSettings) -> bool {
        if self.holdout_blocking {
            return true;
        }
        // Non-finite metrics are corrupt data → fail closed (block live).
        if metric_below_floor(self.avg_sharpe, oos.sharpe_floor)
            || metric_below_floor(self.avg_pnl, oos.avg_pnl_floor)
        {
            return true;
        }
        if oos.min_profitable_fold_ratio > 0.0 {
            let profitable_ratio = self.profitable_folds as f64 / self.n_folds.max(1) as f64;
            if !profitable_ratio.is_finite() || profitable_ratio < oos.min_profitable_fold_ratio {
                return true;
            }
        }
        // Selection bias: a winner that cannot beat its own luck-adjusted
        // benchmark is not an edge, however good the raw Sharpe looks.
        if let Some(dsr) = self.deflated_sharpe {
            if metric_below_floor(dsr, oos.deflated_sharpe_floor) {
                return true;
            }
        }
        if let Some(pbo) = self.pbo {
            if oos.max_pbo < 1.0 && (!pbo.is_finite() || pbo > oos.max_pbo) {
                return true;
            }
        }
        false
    }

    /// Human-readable reasons the kill-switch is red (dashboard / MCP).
    pub fn block_reasons(&self, oos: &OosSettings) -> Vec<String> {
        let mut out = Vec::new();
        if self.holdout_blocking {
            out.push("holdout_red".to_string());
        }
        if metric_below_floor(self.avg_sharpe, oos.sharpe_floor) {
            out.push(format!("sharpe {} < {}", self.avg_sharpe, oos.sharpe_floor));
        }
        if metric_below_floor(self.avg_pnl, oos.avg_pnl_floor) {
            out.push(format!("pnl {} < {}", self.avg_pnl, oos.avg_pnl_floor));
        }
        if oos.min_profitable_fold_ratio > 0.0 {
            let ratio = self.profitable_folds as f64 / self.n_folds.max(1) as f64;
            if !ratio.is_finite() || ratio < oos.min_profitable_fold_ratio {
                out.push(format!(
                    "profitable_folds {ratio:.2} < {}",
                    oos.min_profitable_fold_ratio
                ));
            }
        }
        if let Some(dsr) = self.deflated_sharpe {
            if metric_below_floor(dsr, oos.deflated_sharpe_floor) {
                out.push(format!(
                    "deflated_sharpe {dsr:.3} < {}",
                    oos.deflated_sharpe_floor
                ));
            }
        }
        if let Some(pbo) = self.pbo {
            if oos.max_pbo < 1.0 && (!pbo.is_finite() || pbo > oos.max_pbo) {
                out.push(format!("pbo {pbo:.2} > {}", oos.max_pbo));
            }
        }
        out
    }
}

/// Metric is red when non-finite (NaN/Inf) or strictly below the configured floor.
fn metric_below_floor(value: f64, floor: f64) -> bool {
    !value.is_finite() || value < floor
}

fn holdout_is_red(h: &serde_json::Value) -> bool {
    let pnl = h.get("total_pnl").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let sh = h.get("avg_sharpe").and_then(|v| v.as_f64()).unwrap_or(0.0);
    // NaN/Inf must not pass as "green" (`NaN < 0.0` is false in IEEE-754).
    metric_below_floor(pnl, 0.0) || metric_below_floor(sh, 0.0)
}

/// Network mode: live trading, dry-run (paper), or backtest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    /// Real money – trades hit the venue.
    #[serde(alias = "Live")]
    Live,
    /// Paper / dry-run – real quotes, simulated fills.
    #[default]
    #[serde(alias = "DryRun", alias = "dryrun", alias = "Dryrun")]
    DryRun,
    /// Paper trading alias
    #[serde(alias = "Paper")]
    Paper,
    /// Backtest – historical replay, no network calls.
    #[serde(alias = "Backtest")]
    Backtest,
}

/// Top-level settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub mode: RunMode,
    pub log_level: String,
    pub metrics_port: u16,
    pub exchange: ExchangeSettings,
    pub risk: RiskSettings,
    pub strategy: StrategySettings,
    pub ml: MlSettings,
    #[serde(default)]
    pub oos: OosSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangeSettings {
    /// Primary venue for the money path.
    #[serde(default = "default_primary_venue")]
    pub primary_venue: String,

    /// Hyperliquid info/exchange REST base.
    #[serde(default = "default_hl_url")]
    pub hyperliquid_base_url: String,

    /// Optional Hyperliquid WS.
    #[serde(default = "default_hl_ws")]
    pub hyperliquid_ws_url: String,

    /// Symbols to trade (perp coins on HL).
    #[serde(default = "default_symbols")]
    pub symbol_whitelist: Vec<String>,

    /// Generic EVM RPC (signing / on-chain helpers).
    #[serde(default = "default_rpc")]
    pub rpc_url: String,

    #[serde(default)]
    pub explorer_api_url: String,

    #[serde(default = "default_chain_id")]
    pub chain_id: u64,

    #[serde(default = "default_gas")]
    pub max_gas_price_gwei: u64,

    #[serde(default = "default_slippage")]
    pub max_slippage: f64,

    /// HTTP/SOCKS5 proxy – **opt-in only**. Empty / unset = direct. No auto-WARP scan.
    #[serde(default)]
    pub proxy_url: Option<String>,

    /// Public wallet address for portfolio queries (HL user state).
    #[serde(default)]
    pub wallet_address: String,

    /// Allow synthetic market fallbacks (tests only; never for live).
    #[serde(default)]
    pub allow_synthetic: bool,
}

fn default_primary_venue() -> String {
    "hyperliquid".into()
}
fn default_hl_url() -> String {
    "https://api.hyperliquid.xyz".into()
}
fn default_hl_ws() -> String {
    "wss://api.hyperliquid.xyz/ws".into()
}
fn default_symbols() -> Vec<String> {
    // ETH+SOL focus after 60d OOS (BTC weaker for this alpha)
    vec!["ETH".into(), "SOL".into()]
}
fn default_rpc() -> String {
    "https://rpc.hyperliquid.xyz/evm".into()
}
fn default_chain_id() -> u64 {
    42161 // Arbitrum; HL uses own L1 – kept for EVM helpers
}
fn default_gas() -> u64 {
    50
}
fn default_slippage() -> f64 {
    0.01
}

impl ExchangeSettings {
    pub fn venue_kind(&self) -> VenueKind {
        VenueKind::parse(&self.primary_venue)
    }
}

/// Trade protections: the "has this book proved it should not trade?" layer.
///
/// Evaluated before the risk gate on every entry. Each guard is disabled by
/// setting its threshold to `0`, so old configs keep their behaviour when the
/// whole block is absent.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProtectionConfig {
    /// Stop-outs on one coin inside the window before entries on it are blocked.
    #[serde(default = "default_guard_stops")]
    pub stoploss_guard_max_stops: usize,
    /// Window for [`Self::stoploss_guard_max_stops`], hours.
    #[serde(default = "default_guard_window")]
    pub stoploss_guard_window_hours: f64,
    /// Pause on a coin after any close, hours.
    #[serde(default = "default_cooldown_hours")]
    pub cooldown_hours: f64,
    /// Block all entries when equity is this far below peak (0.10 = 10%).
    #[serde(default = "default_max_drawdown")]
    pub max_drawdown_frac: f64,
}

fn default_guard_stops() -> usize {
    2
}
fn default_guard_window() -> f64 {
    48.0
}
fn default_cooldown_hours() -> f64 {
    4.0
}
fn default_max_drawdown() -> f64 {
    0.10
}

impl Default for ProtectionConfig {
    fn default() -> Self {
        Self {
            stoploss_guard_max_stops: default_guard_stops(),
            stoploss_guard_window_hours: default_guard_window(),
            cooldown_hours: default_cooldown_hours(),
            max_drawdown_frac: default_max_drawdown(),
        }
    }
}

impl ProtectionConfig {
    /// True when every guard is disabled.
    pub fn is_empty(&self) -> bool {
        self.stoploss_guard_max_stops == 0
            && self.cooldown_hours <= 0.0
            && self.max_drawdown_frac <= 0.0
    }

    /// Stop-loss guard window, or `None` when the guard is off.
    pub fn stoploss_window(&self) -> Option<chrono::Duration> {
        if self.stoploss_guard_max_stops == 0 || self.stoploss_guard_window_hours <= 0.0 {
            return None;
        }
        chrono::Duration::try_seconds((self.stoploss_guard_window_hours * 3600.0) as i64)
    }

    /// Per-coin cooldown, or `None` when the guard is off.
    pub fn cooldown(&self) -> Option<chrono::Duration> {
        if self.cooldown_hours <= 0.0 {
            return None;
        }
        chrono::Duration::try_seconds((self.cooldown_hours * 3600.0) as i64)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskSettings {
    /// Max portfolio exposure as fraction of equity (e.g. 0.4 = 40%).
    pub max_portfolio_exposure: f64,
    /// Max position notional per instrument (fraction of equity).
    pub max_position_per_instrument: f64,
    /// Daily loss limit (fraction of starting equity) → kill new orders.
    pub max_daily_loss: f64,
    /// Stop-loss per position (fraction).
    pub stop_loss: f64,
    /// If true, bankroll from exchange portfolio; else starting_bankroll.
    #[serde(default = "default_true")]
    pub use_exchange_balance: bool,
    /// Paper / fallback bankroll in USD.
    #[serde(default = "default_bankroll")]
    pub starting_bankroll: f64,
    /// Exponential decay rate (per day) for wallet confidence after inactivity.
    /// Applied via `decayed_strength = original_strength * exp(-lambda * days_inactive)`.
    #[serde(default = "default_decay_lambda")]
    pub confidence_decay_lambda: f64,
    /// Trade protections (stop-loss guard, cooldown, drawdown circuit breaker).
    #[serde(default)]
    pub protections: ProtectionConfig,
}

fn default_decay_lambda() -> f64 {
    0.1
}

fn default_true() -> bool {
    true
}
fn default_bankroll() -> f64 {
    1_000.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySettings {
    /// Strategy id: `funding_carry` | `trend_mid` | `majors_alpha` | …
    #[serde(default = "default_strategy_name")]
    pub name: String,
    /// Enter funding carry when |funding| >= this (venue raw units).
    #[serde(default = "default_funding_enter")]
    pub funding_enter: f64,
    /// Exit when |funding| <= this.
    #[serde(default = "default_funding_exit")]
    pub funding_exit: f64,
    /// Fractional Kelly multiplier.
    #[serde(default = "default_kelly")]
    pub kelly_fraction: f64,
    /// Minimum edge to trade (strategy-specific).
    #[serde(default)]
    pub min_edge: f64,
    /// Loop interval seconds.
    #[serde(default = "default_loop_secs")]
    pub loop_interval_secs: u64,

    // ── Universe filters (B) ──────────────────────────────────────────
    /// `allowlist` | `filtered` | `open`
    #[serde(default = "default_universe_mode")]
    pub universe_mode: String,
    /// Blacklisted coins (meme / toxic funding).
    #[serde(default = "default_blacklist_cfg")]
    pub blacklist: Vec<String>,
    /// Min OI notional USD ≈ open_interest_coins * mid.
    #[serde(default = "default_min_oi_usd")]
    pub min_oi_notional_usd: f64,
    /// Min mid price.
    #[serde(default = "default_min_mid")]
    pub min_mid: f64,
    /// Cap extreme funding (0 = disabled).
    #[serde(default = "default_max_abs_funding")]
    pub max_abs_funding: f64,
    /// Scanner max concurrent positions.
    #[serde(default = "default_max_pos")]
    pub max_positions: usize,
    /// Top-N after filters.
    #[serde(default = "default_top_n")]
    pub top_n: usize,

    // ── Majors alpha (C): EMA trend + funding veto ────────────────────
    #[serde(default = "default_ema_fast")]
    pub ema_fast: usize,
    #[serde(default = "default_ema_slow")]
    pub ema_slow: usize,
    /// Do not enter long if funding >= this; short if funding <= -this.
    #[serde(default = "default_funding_veto")]
    pub funding_veto: f64,
    /// Min |ema_fast - ema_slow| / mid to treat as a trend.
    #[serde(default = "default_min_trend")]
    pub min_trend_strength: f64,
    /// Bars for realized vol (mean |ret|).
    #[serde(default = "default_vol_lookback")]
    pub vol_lookback: usize,
    /// Min mean |hourly ret| to allow new entries (0 = off).
    #[serde(default = "default_min_vol_ratio")]
    pub min_vol_ratio: f64,
}

fn default_strategy_name() -> String {
    "funding_carry".into()
}
fn default_funding_enter() -> f64 {
    0.0001
}
fn default_funding_exit() -> f64 {
    0.00003
}
fn default_kelly() -> f64 {
    0.25
}
fn default_loop_secs() -> u64 {
    15
}
fn default_universe_mode() -> String {
    "allowlist".into()
}
fn default_blacklist_cfg() -> Vec<String> {
    // Keep in sync with strategies::filters::default_blacklist
    vec![
        "ACE", "NOT", "MOVE", "kNEIRO", "NEIRO", "SUPER", "CASHCAT", "POPCAT", "WIF", "MEW",
        "PNUT", "GOAT", "MOODENG", "FARTCOIN", "BRETT", "GIGA", "BONK", "FLOKI", "BOME", "DOGS",
        "HMSTR", "MEME", "TURBO", "PEOPLE", "RATS", "ORDI", "1000SATS",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}
fn default_min_oi_usd() -> f64 {
    5_000_000.0
}
fn default_min_mid() -> f64 {
    0.05
}
fn default_max_abs_funding() -> f64 {
    0.001
}
fn default_max_pos() -> usize {
    2
}
fn default_top_n() -> usize {
    4
}
fn default_ema_fast() -> usize {
    8
}
fn default_ema_slow() -> usize {
    21
}
fn default_funding_veto() -> f64 {
    0.00015
}
fn default_min_trend() -> f64 {
    0.0008
}
fn default_vol_lookback() -> usize {
    24
}
fn default_min_vol_ratio() -> f64 {
    0.0025
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlSettings {
    /// Master switch – default off for money path.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_model_path")]
    pub model_path: PathBuf,
    #[serde(default = "default_consensus")]
    pub consensus_threshold: f64,
    #[serde(default = "default_n_est")]
    pub n_estimators: usize,
    #[serde(default = "default_llm_endpoint")]
    pub llm_endpoint: String,
    #[serde(default)]
    pub llm_api_key: String,
    #[serde(default = "default_sglang")]
    pub sglang_endpoint: String,
    #[serde(default = "default_sglang_model")]
    pub sglang_model: String,
    #[serde(default)]
    pub use_local_sglang: bool,
}

fn default_model_path() -> PathBuf {
    "models/market_predictor.onnx".into()
}
fn default_consensus() -> f64 {
    0.65
}
fn default_n_est() -> usize {
    200
}
fn default_llm_endpoint() -> String {
    "https://openrouter.ai/api/v1".into()
}
fn default_sglang() -> String {
    "http://localhost:30000/v1".into()
}
fn default_sglang_model() -> String {
    "default".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: RunMode::DryRun,
            log_level: "info".to_string(),
            metrics_port: 9090,
            exchange: ExchangeSettings {
                primary_venue: default_primary_venue(),
                hyperliquid_base_url: default_hl_url(),
                hyperliquid_ws_url: default_hl_ws(),
                symbol_whitelist: default_symbols(),
                rpc_url: default_rpc(),
                explorer_api_url: String::new(),
                chain_id: default_chain_id(),
                max_gas_price_gwei: default_gas(),
                max_slippage: default_slippage(),
                // Opt-in only: do NOT auto-fill from env proxies that point at CF/WARP.
                proxy_url: None,
                wallet_address: String::new(),
                allow_synthetic: false,
            },
            risk: RiskSettings {
                max_portfolio_exposure: 0.4,
                max_position_per_instrument: 0.1,
                max_daily_loss: 0.03,
                stop_loss: 0.10,
                use_exchange_balance: true,
                starting_bankroll: default_bankroll(),
                confidence_decay_lambda: default_decay_lambda(),
                protections: ProtectionConfig::default(),
            },
            strategy: StrategySettings {
                name: default_strategy_name(),
                funding_enter: default_funding_enter(),
                funding_exit: default_funding_exit(),
                kelly_fraction: default_kelly(),
                min_edge: 0.0,
                loop_interval_secs: default_loop_secs(),
                universe_mode: default_universe_mode(),
                blacklist: default_blacklist_cfg(),
                min_oi_notional_usd: default_min_oi_usd(),
                min_mid: default_min_mid(),
                max_abs_funding: default_max_abs_funding(),
                max_positions: default_max_pos(),
                top_n: default_top_n(),
                ema_fast: default_ema_fast(),
                ema_slow: default_ema_slow(),
                funding_veto: default_funding_veto(),
                min_trend_strength: default_min_trend(),
                vol_lookback: default_vol_lookback(),
                min_vol_ratio: default_min_vol_ratio(),
            },
            ml: MlSettings {
                enabled: false,
                model_path: default_model_path(),
                consensus_threshold: default_consensus(),
                n_estimators: default_n_est(),
                llm_endpoint: default_llm_endpoint(),
                llm_api_key: env::var("OPENROUTER_API_KEY").unwrap_or_default(),
                sglang_endpoint: default_sglang(),
                sglang_model: default_sglang_model(),
                use_local_sglang: false,
            },
            oos: OosSettings::default(),
        }
    }
}

impl Settings {
    /// Load settings from `config/<mode>.toml` if present, then override with env vars.
    pub fn load() -> anyhow::Result<Self> {
        let mode_str = env::var("RUN_MODE").unwrap_or_else(|_| "dryrun".to_string());
        let config_path = PathBuf::from("config").join(format!("{}.toml", mode_str));

        let mut settings = Settings::default();

        if config_path.exists() {
            let bytes = std::fs::read_to_string(&config_path)?;
            let file_settings: Settings = toml::from_str(&bytes)?;
            settings = file_settings;
        }

        if let Ok(v) = env::var("LOG_LEVEL") {
            settings.log_level = v;
        }
        if let Ok(v) = env::var("METRICS_PORT") {
            settings.metrics_port = v.parse()?;
        }
        if let Ok(v) = env::var("RPC_URL") {
            settings.exchange.rpc_url = v;
        }
        if let Ok(v) = env::var("PRIMARY_VENUE") {
            settings.exchange.primary_venue = v;
        }
        if let Ok(v) = env::var("HL_BASE_URL") {
            settings.exchange.hyperliquid_base_url = v;
        }
        if let Ok(v) = env::var("WALLET_ADDRESS") {
            settings.exchange.wallet_address = v;
        }
        // Explicit proxy only – HTTPS_PROXY respected if set by operator.
        if let Ok(v) = env::var("HTTPS_PROXY")
            .or_else(|_| env::var("ALL_PROXY"))
            .or_else(|_| env::var("HTTP_PROXY"))
        {
            if !v.is_empty() {
                settings.exchange.proxy_url = Some(v);
            }
        }
        if let Ok(v) = env::var("LLM_API_KEY") {
            settings.ml.llm_api_key = v;
        }
        if let Ok(v) = env::var("RUN_MODE") {
            settings.mode = match v.to_ascii_lowercase().as_str() {
                "live" => RunMode::Live,
                "backtest" => RunMode::Backtest,
                "paper" => RunMode::Paper,
                _ => RunMode::DryRun,
            };
        }

        Ok(settings)
    }

    /// Safety checks before starting the bot (loads OOS from disk / env).
    pub fn validate(&self) -> anyhow::Result<()> {
        self.validate_with_oos(OosStats::load().as_ref())
    }

    /// Same as [`validate`] but inject OOS stats (for e2e / unit tests without disk races).
    pub fn validate_with_oos(&self, oos: Option<&OosStats>) -> anyhow::Result<()> {
        match self.mode {
            // Live: hard block if OOS kill-switch fires.
            RunMode::Live => {
                if let Some(stats) = oos {
                    if stats.is_blocking_with_settings(&self.oos) {
                        anyhow::bail!(
                            "OOS kill-switch active: avg_sharpe={:.2} avg_pnl={:.2} \
                             folds={}/{} holdout_blocking={} – Live auto-trading locked.",
                            stats.avg_sharpe,
                            stats.avg_pnl,
                            stats.profitable_folds,
                            stats.n_folds,
                            stats.holdout_blocking
                        );
                    }
                }
                let confirm = env::var("CONFIRM_LIVE").unwrap_or_default();
                if confirm != "1" {
                    anyhow::bail!("RUN_MODE=live requires CONFIRM_LIVE=1");
                }
                let has_key = env::var("WALLET_PRIVATE_KEY")
                    .or_else(|_| env::var("HL_API_PRIVATE_KEY"))
                    .map(|k| !k.is_empty())
                    .unwrap_or(false);
                if !has_key {
                    anyhow::bail!(
                        "RUN_MODE=live requires WALLET_PRIVATE_KEY or HL_API_PRIVATE_KEY"
                    );
                }
                if self.exchange.allow_synthetic {
                    anyhow::bail!("allow_synthetic must be false in live mode");
                }
            }
            // Paper: advisory – kill-switch emits a warning but does NOT hard-block.
            RunMode::Paper => {
                if let Some(stats) = oos {
                    if stats.is_blocking_with_settings(&self.oos) {
                        tracing::warn!(
                            "OOS kill-switch is active (sharpe={:.2} pnl={:.2} folds={} holdout={}). \
                             Paper mode allowed read-only – live execution remains locked.",
                            stats.avg_sharpe,
                            stats.avg_pnl,
                            stats.n_folds,
                            stats.holdout_blocking
                        );
                    }
                }
            }
            RunMode::Backtest | RunMode::DryRun => {}
        }

        if self.risk.max_position_per_instrument > self.risk.max_portfolio_exposure {
            anyhow::bail!(
                "max_position_per_instrument ({}) cannot exceed max_portfolio_exposure ({})",
                self.risk.max_position_per_instrument,
                self.risk.max_portfolio_exposure
            );
        }

        if self.exchange.symbol_whitelist.is_empty() {
            anyhow::bail!("exchange.symbol_whitelist must not be empty");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deflated_sharpe_gate_blocks_a_lucky_winner() {
        let mut st = OosStats::new(100.0, 1.5, 6, 5);
        // No verdict in the report → old behaviour, not blocked.
        assert!(!st.is_blocking());
        st.deflated_sharpe = Some(-0.12);
        assert!(st.is_blocking(), "negative DSR must block");
        let reasons = st.block_reasons(&OosSettings::default());
        assert!(
            reasons.iter().any(|r| r.contains("deflated_sharpe")),
            "{reasons:?}"
        );
        // NaN fails closed.
        st.deflated_sharpe = Some(f64::NAN);
        assert!(st.is_blocking());
        st.deflated_sharpe = Some(0.97);
        assert!(!st.is_blocking());
    }

    #[test]
    fn pbo_gate_blocks_above_half_and_can_be_disabled() {
        let mut st = OosStats::new(100.0, 1.5, 6, 5);
        st.pbo = Some(0.72);
        assert!(st.is_blocking());
        let reasons = st.block_reasons(&OosSettings::default());
        assert!(reasons.iter().any(|r| r.contains("pbo")), "{reasons:?}");

        let relaxed = OosSettings {
            max_pbo: 1.0,
            ..OosSettings::default()
        };
        assert!(!st.is_blocking_with_settings(&relaxed), "1.0 disables");

        st.pbo = Some(0.3);
        assert!(!st.is_blocking());
    }

    #[test]
    fn report_json_carries_selection_bias_verdicts() {
        let report = json!({
            "test_avg_sharpe": 0.4,
            "test_total_pnl": 120.0,
            "profitable_test_folds": 4,
            "n_folds": 6,
            "deflated_sharpe_min": -0.05,
            "pbo_avg": 0.61,
            "folds": []
        });
        let st = OosStats::from_json(&report).expect("parsed");
        assert_eq!(st.deflated_sharpe, Some(-0.05));
        assert_eq!(st.pbo, Some(0.61));
        assert!(st.is_blocking(), "both verdicts red");
    }

    #[test]
    fn old_report_without_verdicts_still_parses() {
        let report = json!({
            "test_avg_sharpe": 0.4,
            "test_total_pnl": 120.0,
            "profitable_test_folds": 4,
            "n_folds": 6,
            "folds": []
        });
        let st = OosStats::from_json(&report).expect("parsed");
        assert!(st.deflated_sharpe.is_none());
        assert!(st.pbo.is_none());
        assert!(!st.is_blocking(), "schema 1.1 reports keep old behaviour");
    }

    #[test]
    fn default_is_hyperliquid_dryrun() {
        let s = Settings::default();
        assert_eq!(s.mode, RunMode::DryRun);
        assert_eq!(s.exchange.venue_kind(), VenueKind::Hyperliquid);
        assert!(!s.ml.enabled);
        assert!(s.exchange.proxy_url.is_none());
    }

    #[test]
    fn validate_dryrun_ok() {
        let s = Settings::default();
        s.validate_with_oos(None).unwrap();
    }

    #[test]
    fn validate_live_needs_confirm_when_oos_green() {
        let mut s = Settings::default();
        s.mode = RunMode::Live;
        let green = OosStats::new(10.0, 0.5, 6, 4);
        assert!(!green.is_blocking());
        let err = s.validate_with_oos(Some(&green)).unwrap_err().to_string();
        assert!(
            err.contains("CONFIRM_LIVE")
                || err.contains("WALLET_PRIVATE_KEY")
                || err.contains("HL_API"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn validate_live_blocked_by_red_oos() {
        let mut s = Settings::default();
        s.mode = RunMode::Live;
        let red = OosStats::new(-25.0, -1.4, 6, 1);
        assert!(red.is_blocking());
        let err = s.validate_with_oos(Some(&red)).unwrap_err().to_string();
        assert!(
            err.contains("kill-switch") || err.contains("locked"),
            "expected kill-switch, got: {err}"
        );
    }

    #[test]
    fn validate_paper_allows_read_only_when_oos_red() {
        let mut s = Settings::default();
        s.mode = RunMode::Paper;
        let red = OosStats::new(-10.0, -1.0, 5, 0);
        s.validate_with_oos(Some(&red))
            .expect("paper mode must remain available under OOS kill-switch");
    }

    #[test]
    fn validate_dryrun_allows_when_oos_red() {
        let s = Settings::default();
        assert_eq!(s.mode, RunMode::DryRun);
        let red = OosStats::new(-1.0, -0.5, 3, 0);
        s.validate_with_oos(Some(&red)).unwrap();
    }

    #[test]
    fn oos_from_json_top_level_and_holdout() {
        let red = json!({
            "test_avg_sharpe": -1.2,
            "test_total_pnl": -100.0,
            "n_folds": 3,
            "profitable_test_folds": 0,
            "folds": [
                {"test": {"total_pnl": -40.0, "sharpe_ratio": -1.0}},
                {"test": {"total_pnl": -30.0, "sharpe_ratio": -1.1}},
                {"test": {"total_pnl": -30.0, "sharpe_ratio": -1.5}},
            ],
            "holdout": {"total_pnl": -5.0, "avg_sharpe": -2.0}
        });
        let s = OosStats::from_json(&red).expect("parse");
        assert!(s.is_blocking());
        assert!(s.holdout_blocking);

        let green = json!({
            "test_avg_sharpe": 0.8,
            "test_total_pnl": 50.0,
            "profitable_test_folds": 5,
            "folds": [
                {"test": {"total_pnl": 10.0, "sharpe_ratio": 0.5}},
                {"test": {"total_pnl": 10.0, "sharpe_ratio": 0.6}},
                {"test": {"total_pnl": 10.0, "sharpe_ratio": 0.7}},
                {"test": {"total_pnl": 10.0, "sharpe_ratio": 0.8}},
                {"test": {"total_pnl": 10.0, "sharpe_ratio": 0.9}},
            ],
            "holdout": {"total_pnl": 3.0, "avg_sharpe": 0.4}
        });
        let g = OosStats::from_json(&green).expect("parse green");
        assert!(!g.is_blocking());
        assert!(!g.holdout_blocking);
    }

    #[test]
    fn oos_holdout_red_blocks_even_if_wf_green() {
        let mut s = OosStats::new(10.0, 0.5, 6, 5);
        assert!(!s.is_blocking());
        s.holdout_blocking = true;
        assert!(s.is_blocking());
    }

    #[test]
    fn oos_stats_load_from_real_report() {
        let stats = OosStats::load();
        assert!(stats.is_some(), "wf_latest.json should exist and parse");
        let s = stats.unwrap();
        assert!(s.n_folds >= 1);
        // Current money-path reports are red – kill-switch must fire.
        assert!(
            s.is_blocking(),
            "OOS should be red currently: avg_pnl={:.2} avg_sharpe={:.2} holdout={}",
            s.avg_pnl,
            s.avg_sharpe,
            s.holdout_blocking
        );
    }

    #[test]
    fn oos_load_from_path_roundtrip_tempfile() {
        let dir = std::env::temp_dir().join(format!("trading_oos_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("wf.json");
        let body = json!({
            "test_avg_sharpe": -0.5,
            "test_total_pnl": -12.0,
            "folds": [
                {"test": {"total_pnl": -4.0, "sharpe_ratio": -0.4}},
                {"test": {"total_pnl": -4.0, "sharpe_ratio": -0.5}},
                {"test": {"total_pnl": -4.0, "sharpe_ratio": -0.6}},
            ],
            "profitable_test_folds": 0
        });
        std::fs::write(&path, body.to_string()).unwrap();
        let s = OosStats::load_from_path(&path).expect("load temp");
        assert!(s.is_blocking());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn metric_below_floor_rejects_non_finite_and_red() {
        // JSON cannot encode NaN; kill-switch still sees NaN/Inf on in-memory stats.
        assert!(metric_below_floor(f64::NAN, 0.0));
        assert!(metric_below_floor(f64::INFINITY, 0.0));
        assert!(metric_below_floor(f64::NEG_INFINITY, 0.0));
        assert!(metric_below_floor(-0.01, 0.0));
        assert!(!metric_below_floor(0.0, 0.0));
        assert!(!metric_below_floor(0.5, 0.0));
    }

    #[test]
    fn holdout_is_red_on_negative_or_non_finite() {
        assert!(holdout_is_red(
            &json!({"total_pnl": -1.0, "avg_sharpe": 0.5})
        ));
        assert!(holdout_is_red(
            &json!({"total_pnl": 1.0, "avg_sharpe": -0.1})
        ));
        assert!(!holdout_is_red(
            &json!({"total_pnl": 1.0, "avg_sharpe": 0.1})
        ));
        // Missing fields → 0.0 floor (not strictly red); null → same.
        assert!(!holdout_is_red(
            &json!({"total_pnl": 0.0, "avg_sharpe": 0.0})
        ));
    }

    #[test]
    fn oos_nan_metrics_block_even_without_holdout() {
        let mut s = OosStats::new(10.0, 0.5, 3, 3);
        assert!(!s.is_blocking());
        s.avg_sharpe = f64::NAN;
        assert!(s.is_blocking(), "NaN sharpe must fail closed");
        s.avg_sharpe = 0.5;
        s.avg_pnl = f64::NAN;
        assert!(s.is_blocking(), "NaN pnl must fail closed");
        s.avg_pnl = f64::NEG_INFINITY;
        assert!(s.is_blocking(), "-Inf pnl must fail closed");
        s.avg_pnl = 10.0;
        s.holdout_blocking = true;
        assert!(s.is_blocking());
    }
}
