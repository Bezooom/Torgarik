//! Bank of Russia daily FX rates (`cbr-xml-daily.ru`, no key required).
//!
//! Two responsibilities only: fetch the daily JSON and keep a one-file cache so
//! a tax calculation never depends on the network being up at that moment.
//! All arithmetic lives in `trading_common::fx`; this module just produces
//! [`FxRates`].
//!
//! The cache is keyed by the published date: a cached file from another day is
//! used only when the fetch fails, and the caller can see the date it got.

use std::path::{Path, PathBuf};

use serde_json::Value;
use tracing::{info, warn};
use trading_common::fx::FxRates;

/// Public mirror of the CBR daily rates.
pub const DEFAULT_CBR_URL: &str = "https://www.cbr-xml-daily.ru/daily_json.js";

/// Default cache location, relative to the working directory.
pub const FX_CACHE_FILE: &str = "runtime/fx_cbr.json";

/// Fetch/caching client for official rates.
pub struct CbrClient {
    url: String,
    cache: PathBuf,
    client: reqwest::Client,
}

impl Default for CbrClient {
    fn default() -> Self {
        Self::new(DEFAULT_CBR_URL, FX_CACHE_FILE)
    }
}

impl CbrClient {
    pub fn new(url: impl Into<String>, cache: impl Into<PathBuf>) -> Self {
        Self {
            url: url.into(),
            cache: cache.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Rates for today: cache first (same date), otherwise fetch and refresh.
    pub async fn daily(&self) -> Result<FxRates, String> {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        if let Some(cached) = Self::read_cache(&self.cache) {
            if cached.date.starts_with(&today) {
                return Ok(cached);
            }
        }
        match self.fetch().await {
            Ok(rates) => {
                self.write_cache(&rates);
                Ok(rates)
            }
            Err(e) => match Self::read_cache(&self.cache) {
                Some(stale) => {
                    warn!(
                        "cbr: fetch failed ({e}) – using cached rates from {}",
                        stale.date
                    );
                    Ok(stale)
                }
                None => Err(e),
            },
        }
    }

    async fn fetch(&self) -> Result<FxRates, String> {
        let resp = self
            .client
            .get(&self.url)
            .send()
            .await
            .map_err(|e| format!("cbr: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("cbr HTTP {status}"));
        }
        let body = resp.text().await.map_err(|e| format!("cbr body: {e}"))?;
        Self::parse(&body)
    }

    /// Parse the `daily_json.js` payload. Public for fixture tests.
    pub fn parse(body: &str) -> Result<FxRates, String> {
        let root: Value = serde_json::from_str(body).map_err(|e| format!("cbr json: {e}"))?;
        // `Date` looks like `2026-10-31T11:30:00+03:00`; keep the date part.
        let date = root
            .get("Date")
            .and_then(|v| v.as_str())
            .map(|s| s.split('T').next().unwrap_or(s).to_string())
            .ok_or_else(|| "cbr: no Date".to_string())?;
        let valute = root
            .get("Valute")
            .and_then(|v| v.as_object())
            .ok_or_else(|| "cbr: no Valute".to_string())?;

        // Value is per `Nominal` units, not per 1 unit: divide before using.
        let per_unit = |code: &str| -> Result<f64, String> {
            let node = valute.get(code).ok_or_else(|| format!("cbr: no {code}"))?;
            let value = node
                .get("Value")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| format!("cbr: {code} has no Value"))?;
            let nominal = node
                .get("Nominal")
                .and_then(|v| v.as_f64())
                .filter(|n| *n > 0.0)
                .unwrap_or(1.0);
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("cbr: {code} Value {value} invalid"));
            }
            Ok(value / nominal)
        };

        FxRates::new(date, per_unit("USD")?, per_unit("EUR")?, per_unit("CNY")?)
    }

    fn read_cache(path: &Path) -> Option<FxRates> {
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str::<FxRates>(&raw).ok()
    }

    fn write_cache(&self, rates: &FxRates) {
        if let Some(parent) = self.cache.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(rates) {
            Ok(body) => {
                if let Err(e) = std::fs::write(&self.cache, body) {
                    warn!("cbr cache write {}: {e}", self.cache.display());
                }
            }
            Err(e) => warn!("cbr cache serialize: {e}"),
        }
    }

    /// Cached rates without touching the network (offline tools, tests).
    pub fn cached(&self) -> Option<FxRates> {
        Self::read_cache(&self.cache)
    }

    /// Log the rate that a caller is about to use, so a tax number can always
    /// be traced back to a date.
    pub fn log_used(rates: &FxRates) {
        info!(
            "fx {}: USD={:.4} EUR={:.4} CNY={:.4} RUB",
            rates.date, rates.usd_rub, rates.eur_rub, rates.cny_rub
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "Date": "2026-10-31T11:30:00+03:00",
      "PreviousDate": "2026-10-30T11:30:00+03:00",
      "Valute": {
        "USD": {"ID": "R01235", "NumCode": "840", "CharCode": "USD", "Nominal": 1, "Name": "Доллар США", "Value": 80.1234, "Previous": 79.9},
        "EUR": {"ID": "R01239", "NumCode": "978", "CharCode": "EUR", "Nominal": 1, "Name": "Евро", "Value": 87.4321, "Previous": 87.0},
        "CNY": {"ID": "R01375", "NumCode": "156", "CharCode": "CNY", "Nominal": 10, "Name": "Юань", "Value": 112.5, "Previous": 112.0}
      }
    }"#;

    #[test]
    fn parses_daily_payload_and_normalises_nominal() {
        let r = CbrClient::parse(FIXTURE).expect("parse");
        assert_eq!(r.date, "2026-10-31");
        assert!((r.usd_rub - 80.1234).abs() < 1e-9);
        assert!((r.eur_rub - 87.4321).abs() < 1e-9);
        // CNY is quoted per 10 units → 11.25 RUB per yuan.
        assert!((r.cny_rub - 11.25).abs() < 1e-9, "{}", r.cny_rub);
    }

    #[test]
    fn missing_currency_is_an_error_not_a_zero_rate() {
        let body =
            r#"{"Date":"2026-10-31T11:30:00+03:00","Valute":{"USD":{"Nominal":1,"Value":80.0}}}"#;
        let err = CbrClient::parse(body).expect_err("must fail");
        assert!(err.contains("EUR"), "{err}");
    }

    #[test]
    fn zero_or_negative_value_is_rejected() {
        let body = r#"{"Date":"2026-10-31","Valute":{
            "USD":{"Nominal":1,"Value":0.0},
            "EUR":{"Nominal":1,"Value":87.0},
            "CNY":{"Nominal":10,"Value":112.0}}}"#;
        assert!(CbrClient::parse(body).is_err());
    }

    #[test]
    fn missing_date_is_an_error() {
        let body = r#"{"Valute":{"USD":{"Nominal":1,"Value":80.0},"EUR":{"Nominal":1,"Value":87.0},"CNY":{"Nominal":10,"Value":112.0}}}"#;
        assert!(CbrClient::parse(body).is_err());
    }

    #[test]
    fn cache_roundtrip_is_offline() {
        let dir = std::env::temp_dir().join(format!(
            "cbr-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let path = dir.join("fx_cbr.json");
        let client = CbrClient::new("http://127.0.0.1:1/never", &path);
        assert!(client.cached().is_none(), "no cache yet");

        let rates = CbrClient::parse(FIXTURE).expect("parse");
        client.write_cache(&rates);
        let cached = client.cached().expect("cache hit");
        assert_eq!(cached, rates);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Live CBR smoke: `cargo test -p trading-exchange --lib cbr_live -- --ignored`
    #[tokio::test]
    #[ignore = "network: hits cbr-xml-daily.ru"]
    async fn cbr_live_rates() {
        let dir = std::env::temp_dir().join(format!("cbr-live-{}", std::process::id()));
        let client = CbrClient::new(DEFAULT_CBR_URL, dir.join("fx.json"));
        let rates = client.daily().await.expect("live rates");
        CbrClient::log_used(&rates);
        assert!(rates.usd_rub > 10.0 && rates.usd_rub < 500.0);
        assert!(rates.eur_rub > rates.usd_rub);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
