//! Structural anomaly logger — market facts, not trade signals.
//!
//! Hypotheses (see docs/ANOMALIES.md):
//! H1: Extreme |funding| predicts *adverse* 1–4h mid move (funding trap).
//! H2: Quiet vol + then spike — optional (logged as vol_regime).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use tokio::time::{interval, Duration};
use tracing::info;

use trading_common::ConfigStore;
use trading_exchange::{Exchange, HyperliquidClient};

use crate::journal::{Journal, JournalEvent};

/// Majors rarely hit 2e-4; H1 deep-dive: useful signal zone ~1e-5..3e-5 (p90–p99).
const FUNDING_EXTREME: f64 = 0.000025;
const LOOKBACK: usize = 12; // hours of mids for short ret

pub struct AnomalyLogger {
    hl: Arc<HyperliquidClient>,
    journal: Journal,
    symbols: Vec<String>,
    mids: HashMap<String, VecDeque<(i64, f64)>>,
    last_funding: HashMap<String, f64>,
}

impl AnomalyLogger {
    pub fn new(hl: Arc<HyperliquidClient>, journal: Journal, symbols: Vec<String>) -> Self {
        Self {
            hl,
            journal,
            symbols,
            mids: HashMap::new(),
            last_funding: HashMap::new(),
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        info!(
            "ANOMALY LOGGER symbols={:?} journal={}",
            self.symbols,
            self.journal.path().display()
        );
        self.journal.write(&JournalEvent::Note {
            ts: Utc::now(),
            text: "anomaly_logger start H1=funding_trap H2=vol_spike".into(),
        });

        let mut ticker = interval(Duration::from_secs(60));
        loop {
            ticker.tick().await;
            if let Err(e) = self.hl.refresh_market_data().await {
                tracing::warn!("refresh: {e}");
                continue;
            }
            let now = Utc::now().timestamp();
            for coin in &self.symbols.clone() {
                let Ok(q) = self.hl.get_quote(coin).await else {
                    continue;
                };
                let mid = q.mid;
                let funding = q.funding_rate.unwrap_or(0.0);

                let hist = self.mids.entry(coin.clone()).or_default();
                hist.push_back((now, mid));
                while hist.len() > LOOKBACK + 2 {
                    hist.pop_front();
                }

                // H1: funding extreme
                if funding.abs() >= FUNDING_EXTREME {
                    let ret_1h = ret_from(hist, 3600);
                    let ctx = format!(
                        "funding={funding:.6} mid={mid:.4} ret_~1h={ret_1h:?} (H1: extreme funding)"
                    );
                    info!("ANOMALY funding_extreme {coin} {ctx}");
                    self.journal.write(&JournalEvent::Anomaly {
                        ts: Utc::now(),
                        coin: coin.clone(),
                        name: "funding_extreme".into(),
                        value: funding,
                        context: ctx,
                    });
                }

                // Funding flip
                if let Some(prev) = self.last_funding.get(coin) {
                    if prev.signum() != funding.signum()
                        && prev.abs() > 1e-6
                        && funding.abs() > 1e-6
                    {
                        let ctx = format!("funding flip {prev:.6} → {funding:.6}");
                        info!("ANOMALY funding_flip {coin} {ctx}");
                        self.journal.write(&JournalEvent::Anomaly {
                            ts: Utc::now(),
                            coin: coin.clone(),
                            name: "funding_flip".into(),
                            value: funding,
                            context: ctx,
                        });
                    }
                }
                self.last_funding.insert(coin.clone(), funding);

                // H2: short-horizon vol
                if hist.len() >= LOOKBACK {
                    let mut s = 0.0;
                    let mut c = 0usize;
                    let v: Vec<_> = hist.iter().cloned().collect();
                    for i in 1..v.len() {
                        let p0 = v[i - 1].1;
                        let p1 = v[i].1;
                        if p0 > 0.0 {
                            s += ((p1 - p0) / p0).abs();
                            c += 1;
                        }
                    }
                    let mean_abs = if c > 0 { s / c as f64 } else { 0.0 };
                    if mean_abs >= 0.008 {
                        // ~0.8% mean step — hot
                        let ctx = format!("mean_abs_step={mean_abs:.5} n={c}");
                        info!("ANOMALY vol_hot {coin} {ctx}");
                        self.journal.write(&JournalEvent::Anomaly {
                            ts: Utc::now(),
                            coin: coin.clone(),
                            name: "vol_hot".into(),
                            value: mean_abs,
                            context: ctx,
                        });
                    }
                }
            }
        }
    }
}

fn ret_from(hist: &VecDeque<(i64, f64)>, lookback_sec: i64) -> Option<f64> {
    if hist.len() < 2 {
        return None;
    }
    let (t1, p1) = *hist.back()?;
    let target = t1 - lookback_sec;
    let mut best: Option<(i64, f64)> = None;
    for &(t, p) in hist.iter() {
        if t <= target {
            best = Some((t, p));
        }
    }
    let (_, p0) = best?;
    if p0 <= 0.0 {
        return None;
    }
    Some((p1 - p0) / p0)
}

pub async fn run_anomaly() -> Result<()> {
    let settings = trading_common::Settings::load()?;
    let symbols = if settings.exchange.symbol_whitelist.is_empty() {
        vec!["BTC".into(), "ETH".into(), "SOL".into()]
    } else {
        // watch a bit wider for anomalies even if trading is ethsol
        let mut s = settings.exchange.symbol_whitelist.clone();
        for c in ["BTC", "ETH", "SOL"] {
            if !s.iter().any(|x| x == c) {
                s.push(c.into());
            }
        }
        s
    };
    let hl = Arc::new(HyperliquidClient::with_signer(
        &settings.exchange.hyperliquid_base_url,
        &settings.exchange.wallet_address,
        None,
        settings.exchange.proxy_url.as_deref(),
    )?);
    let journal = Journal::open("runtime/journal")?;
    let mut logger = AnomalyLogger::new(hl, journal, symbols);
    logger.run().await
}

// silence unused ConfigStore import path if any
#[allow(dead_code)]
fn _cfg() -> ConfigStore {
    ConfigStore::from_settings(trading_common::Settings::default())
}
