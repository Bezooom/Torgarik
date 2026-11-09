//! Load mid+funding series from collector CSV/JSON.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::engine::Bar;

#[derive(Debug, Deserialize)]
struct JsonSeries {
    coin: Option<String>,
    bars: Vec<JsonBar>,
}

#[derive(Debug, Deserialize)]
struct JsonBar {
    ts: i64,
    mid: f64,
    funding: f64,
    #[serde(default)]
    #[allow(dead_code)]
    premium: f64,
    /// Optional USD notional volume for impact model.
    #[serde(default)]
    volume: f64,
    #[serde(default)]
    volume_usd: f64,
}

#[derive(Debug, Clone)]
pub struct CoinSeries {
    pub coin: String,
    pub bars: Vec<Bar>,
}

/// Load from `COIN_1h.json` or `COIN_1h.csv`.
pub fn load_series(path: &Path) -> anyhow::Result<CoinSeries> {
    let coin = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("UNK")
        .trim_end_matches("_1h")
        .to_string();

    if path.extension().and_then(|e| e.to_str()) == Some("json") {
        let text = fs::read_to_string(path)?;
        let series: JsonSeries = serde_json::from_str(&text)?;
        let bars = series
            .bars
            .into_iter()
            .filter(|b| b.mid > 0.0)
            .map(|b| Bar {
                ts: b.ts,
                mid: b.mid,
                funding: b.funding,
                volume: if b.volume > 0.0 {
                    b.volume
                } else {
                    b.volume_usd
                },
            })
            .collect();
        return Ok(CoinSeries {
            coin: series.coin.unwrap_or(coin),
            bars,
        });
    }

    // CSV: ts,ts_ms,mid,funding,premium
    let text = fs::read_to_string(path)?;
    let mut bars = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 && line.starts_with("ts") {
            continue;
        }
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 4 {
            continue;
        }
        let ts: i64 = parts[0].parse()?;
        let mid: f64 = parts[2].parse()?;
        let funding: f64 = parts[3].parse()?;
        if mid > 0.0 {
            // Optional 6th column: volume_usd
            let volume = parts.get(5).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            bars.push(Bar {
                ts,
                mid,
                funding,
                volume,
            });
        }
    }
    Ok(CoinSeries { coin, bars })
}

/// Drop every bar the world had not produced yet at `as_of` (unix seconds).
///
/// Point-in-time replay: the same history file, cut at a past instant, must
/// reproduce the run that was made on that instant. Bars are not assumed to be
/// sorted, so the cut is a filter, not a truncation.
pub fn truncate_as_of(series: &mut CoinSeries, as_of: i64) -> usize {
    let before = series.bars.len();
    series.bars.retain(|b| b.ts <= as_of);
    before - series.bars.len()
}

/// [`load_series`] with an optional point-in-time cut.
pub fn load_series_as_of(path: &Path, as_of: Option<i64>) -> anyhow::Result<CoinSeries> {
    let mut series = load_series(path)?;
    if let Some(cut) = as_of {
        let dropped = truncate_as_of(&mut series, cut);
        if dropped > 0 {
            tracing::info!(
                "{}: point-in-time cut as_of={cut} dropped {dropped} bars ({} left)",
                series.coin,
                series.bars.len()
            );
        }
    }
    Ok(series)
}

/// Discover `*_1h.json` (prefer) or `*_1h.csv` under dir.
pub fn load_history_dir(dir: &Path) -> anyhow::Result<Vec<CoinSeries>> {
    load_history_dir_as_of(dir, None)
}

/// [`load_history_dir`] with a point-in-time cut applied to every series.
pub fn load_history_dir_as_of(
    dir: &Path,
    as_of: Option<i64>,
) -> anyhow::Result<Vec<CoinSeries>> {
    if !dir.is_dir() {
        anyhow::bail!("data dir not found: {}", dir.display());
    }
    let mut jsons: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with("_1h.json"))
                .unwrap_or(false)
        })
        .collect();
    if jsons.is_empty() {
        jsons = fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with("_1h.csv"))
                    .unwrap_or(false)
            })
            .collect();
    }
    jsons.sort();
    let mut out = Vec::new();
    for p in jsons {
        match load_series_as_of(&p, as_of) {
            Ok(s) if s.bars.len() >= 10 => out.push(s),
            Ok(s) => tracing::warn!("{}: only {} bars, skip", s.coin, s.bars.len()),
            Err(e) => tracing::warn!("load {}: {e}", p.display()),
        }
    }
    if out.is_empty() {
        anyhow::bail!(
            "no series in {} – run: python3 scripts/collect_hl_history.py --days 14",
            dir.display()
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series() -> CoinSeries {
        CoinSeries {
            coin: "TEST".into(),
            bars: vec![
                Bar::new(1_700_000_300, 101.0, 0.0),
                Bar::new(1_700_000_000, 100.0, 0.0),
                Bar::new(1_700_000_600, 102.0, 0.0),
            ],
        }
    }

    #[test]
    fn as_of_cut_keeps_only_bars_known_at_that_instant() {
        let mut s = series();
        let dropped = truncate_as_of(&mut s, 1_700_000_300);
        assert_eq!(dropped, 1);
        assert_eq!(s.bars.len(), 2);
        assert!(s.bars.iter().all(|b| b.ts <= 1_700_000_300));
    }

    #[test]
    fn as_of_cut_is_inclusive_and_unsorted_safe() {
        let mut s = series();
        // Cut exactly at the middle bar's ts: that bar is still known.
        let dropped = truncate_as_of(&mut s, 1_700_000_000);
        assert_eq!(dropped, 2);
        assert_eq!(s.bars.len(), 1);
        assert_eq!(s.bars[0].mid, 100.0);

        let mut untouched = series();
        assert_eq!(truncate_as_of(&mut untouched, i64::MAX), 0);
        assert_eq!(untouched.bars.len(), 3);
    }
}
