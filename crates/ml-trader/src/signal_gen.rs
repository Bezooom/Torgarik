//! `--mode signals`: turn research output into shell input with the *same* code.
//!
//! The parity rule of this workspace: a strategy that passed walk-forward must
//! be the strategy that runs. Both sides call
//! [`trading_strategies::signals::trend_plan`]; this module only reads history,
//! takes the last closed bar's side, and appends a normal external signal line.
//! Every gate of the shell (whitelist, protections, risk, OOS) still applies
//! downstream, so this mode cannot place an order on its own.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use trading_strategies::signals::{trend_plan, TrendParams};

/// History file shape written by `scripts/collect_hl_history.py`.
#[derive(Debug, Deserialize)]
struct HistoryFile {
    #[serde(default)]
    coin: Option<String>,
    bars: Vec<HistoryBar>,
}

#[derive(Debug, Deserialize)]
struct HistoryBar {
    #[serde(default)]
    ts: i64,
    mid: f64,
    #[serde(default)]
    funding: f64,
}

/// Load mids + funding from `COIN_1h.json`, keeping only bars with a usable mid.
pub fn load_series(path: &Path) -> Result<(String, Vec<f64>, Vec<f64>)> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    let file: HistoryFile =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    let coin = file
        .coin
        .or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.trim_end_matches("_1h").to_string())
        })
        .unwrap_or_else(|| "UNKNOWN".into());
    let mut mids = Vec::with_capacity(file.bars.len());
    let mut funding = Vec::with_capacity(file.bars.len());
    for b in file.bars {
        if b.mid.is_finite() && b.mid > 0.0 {
            mids.push(b.mid);
            funding.push(b.funding);
        }
    }
    Ok((coin, mids, funding))
}

/// Latest side for one series, or `None` when the plan says flat.
pub fn latest_side(mids: &[f64], funding: &[f64], params: &TrendParams) -> Option<f64> {
    let plan = trend_plan(mids, funding, params);
    plan.signal.last().copied().filter(|s| s.abs() > 0.5)
}

/// Signal line appended to `runtime/signals.jsonl`.
#[derive(Debug, serde::Serialize)]
pub struct SignalLine {
    pub coin: String,
    pub side: String,
    pub note: String,
    pub source: String,
}

/// Build a signal line for `coin` given its plan side.
pub fn signal_line(coin: &str, side: f64, params: &TrendParams, bars: usize) -> SignalLine {
    SignalLine {
        coin: coin.to_string(),
        side: if side > 0.0 { "buy" } else { "sell" }.to_string(),
        note: format!(
            "trend_plan ema={}/{} bars={} (same code as backtest)",
            params.ema_fast, params.ema_slow, bars
        ),
        source: "trend_plan".into(),
    }
}

/// Append one line to the signals file (created if missing).
pub fn append_signal(path: &Path, line: &SignalLine) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    writeln!(f, "{}", serde_json::to_string(line)?)?;
    Ok(())
}

/// Default history path for a coin under `data/history`.
pub fn history_path(coin: &str) -> PathBuf {
    PathBuf::from("data/history").join(format!("{coin}_1h.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir()
            .join(format!("sig-{tag}-{nanos}-{}", std::process::id()))
    }

    #[test]
    fn loads_only_usable_bars() {
        let dir = tmp("load");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("ETH_1h.json");
        std::fs::write(
            &path,
            r#"{"coin":"ETH","bars":[
                {"ts":1,"mid":100.0,"funding":0.0},
                {"ts":2,"mid":0.0,"funding":0.0},
                {"ts":3,"mid":101.5,"funding":0.0001}
            ]}"#,
        )
        .expect("write");
        let (coin, mids, funding) = load_series(&path).expect("load");
        assert_eq!(coin, "ETH");
        assert_eq!(mids, vec![100.0, 101.5]);
        assert_eq!(funding.len(), mids.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn coin_falls_back_to_file_stem() {
        let dir = tmp("stem");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("SOL_1h.json");
        std::fs::write(&path, r#"{"bars":[{"ts":1,"mid":10.0}]}"#).expect("write");
        let (coin, _, _) = load_series(&path).expect("load");
        assert_eq!(coin, "SOL");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uptrend_yields_buy_and_flat_yields_none() {
        let p = TrendParams {
            min_vol_ratio: 0.0,
            funding_veto: 0.0,
            min_trend_strength: 0.0001,
            ..TrendParams::default()
        };
        let up: Vec<f64> = (0..80).map(|i| 100.0 * 1.004f64.powi(i)).collect();
        assert_eq!(latest_side(&up, &[], &p), Some(1.0));

        let flat = vec![100.0; 80];
        assert_eq!(latest_side(&flat, &[], &p), None, "no trend, no signal");
    }

    #[test]
    fn appended_line_is_valid_jsonl_and_readable_by_the_shell() {
        let dir = tmp("append");
        let path = dir.join("signals.jsonl");
        let p = TrendParams::default();
        let line = signal_line("ETH", 1.0, &p, 892);
        append_signal(&path, &line).expect("append");
        append_signal(&path, &signal_line("SOL", -1.0, &p, 892)).expect("append");

        let body = std::fs::read_to_string(&path).expect("read");
        let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).expect("json");
        assert_eq!(first["coin"], "ETH");
        assert_eq!(first["side"], "buy");
        assert_eq!(first["source"], "trend_plan");
        let second: serde_json::Value = serde_json::from_str(lines[1]).expect("json");
        assert_eq!(second["side"], "sell");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
