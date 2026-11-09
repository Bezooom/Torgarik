//! Moscow Exchange market data over the public ISS JSON API.
//!
//! Read-only by design: MOEX is not on this workspace's money path, so the
//! client implements [`MarketData`] and nothing else. No key, no auth, no
//! order placement, no session.
//!
//! Covered endpoints (all `iss.meta=off`, JSON):
//!
//! | Method | ISS path |
//! |---|---|
//! | [`MoexClient::list_instruments`] | `/iss/engines/stock/markets/shares/boards/{board}/securities.json` |
//! | [`MoexClient::get_quote`] | `.../securities/{SECID}.json` |
//! | [`MoexClient::get_orderbook`] | `.../securities/{SECID}/orderbook.json` |
//! | [`MoexClient::candles`] | `.../securities/{SECID}/candles.json` |
//!
//! Prices are quoted in **RUB**; `Quote` has no currency field, so every
//! consumer of this client must treat prices as RUB. That is the whole reason
//! the tax layer works in RUB cost basis.
//!
//! ISS answers with `{"<block>": {"columns": [...], "data": [[...], ...]}}`.
//! [`Table`] flattens that into name-indexed rows so column order can never
//! silently shift a field into the wrong slot.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use tracing::debug;
use trading_common::{
    InstrumentId, InstrumentRef, MarketType, Quote, TradingError, TradingResult, VenueKind,
};

use crate::traits::{Book, BookLevel, MarketData};

/// Default ISS root. Override for a mirror or a recorded proxy.
pub const DEFAULT_ISS_BASE: &str = "https://iss.moex.com";

/// Main equity board (T+ mode, RUB settlement).
pub const DEFAULT_BOARD: &str = "TQBR";

/// One declared dividend: registry close date and cash per share.
///
/// Corporate actions are what make an equity backtest honest: without the cash
/// payment the price series drops on the ex-date and looks like a loss.
#[derive(Debug, Clone, PartialEq)]
pub struct MoexDividend {
    pub secid: String,
    /// Date the holder list closes (`YYYY-MM-DD`).
    pub registry_close: String,
    /// Cash per one share, in RUB.
    pub value_rub: f64,
    pub currency: String,
}

/// One MOEX candle as returned by ISS, times in UTC.
#[derive(Debug, Clone, PartialEq)]
pub struct MoexCandle {
    pub begin: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub open: f64,
    pub close: f64,
    pub high: f64,
    pub low: f64,
    /// RUB turnover of the candle.
    pub value_rub: f64,
    /// Number of securities traded.
    pub volume: f64,
}

/// ISS block flattened to named columns.
#[derive(Debug, Clone, Default)]
pub struct Table {
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
}

impl Table {
    /// Parse `root[block]` into a table. Missing block = empty table, so a
    /// venue answering `{"marketdata":{"columns":[],"data":[]}}` is a "no data"
    /// case, not a parse failure.
    pub fn from_block(root: &Value, block: &str) -> TradingResult<Self> {
        let Some(node) = root.get(block) else {
            return Ok(Self::default());
        };
        let columns = node
            .get("columns")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|v| v.as_str().unwrap_or_default().to_string())
                    .collect::<Vec<_>>()
            })
            .ok_or_else(|| {
                TradingError::Serialization(format!("ISS block '{block}': no columns array"))
            })?;
        let rows = node
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|row| row.as_array().cloned().unwrap_or_default())
                    .collect::<Vec<_>>()
            })
            .ok_or_else(|| {
                TradingError::Serialization(format!("ISS block '{block}': no data array"))
            })?;
        Ok(Self { columns, rows })
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// First row as a name -> value map.
    pub fn first_map(&self) -> Option<HashMap<&str, &Value>> {
        self.rows.first().map(|row| self.row_map(row))
    }

    pub fn row_map<'a>(&'a self, row: &'a [Value]) -> HashMap<&'a str, &'a Value> {
        self.columns
            .iter()
            .zip(row.iter())
            .map(|(c, v)| (c.as_str(), v))
            .collect()
    }

    /// Rows as name -> value maps.
    pub fn maps(&self) -> Vec<HashMap<&str, &Value>> {
        self.rows.iter().map(|r| self.row_map(r)).collect()
    }
}

/// Read a numeric field, accepting ISS's mixed int/float/string encoding.
pub fn as_f64(v: Option<&&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// Read a string field, empty strings become `None`.
pub fn as_str<'a>(v: Option<&'a &'a Value>) -> Option<&'a str> {
    match v {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.as_str()),
        _ => None,
    }
}

/// MOEX ISS market data client.
pub struct MoexClient {
    base: String,
    board: String,
    client: reqwest::Client,
}

impl Default for MoexClient {
    fn default() -> Self {
        Self::new(DEFAULT_ISS_BASE)
    }
}

impl MoexClient {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            board: DEFAULT_BOARD.to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Use a different board (`TQBR` equities, `TQTF` funds, `TQCB` bonds).
    pub fn with_board(mut self, board: impl Into<String>) -> Self {
        self.board = board.into();
        self
    }

    fn shares_path(&self, tail: &str) -> String {
        format!(
            "{}/iss/engines/stock/markets/shares/boards/{}/{}",
            self.base, self.board, tail
        )
    }

    async fn fetch(&self, url: &str) -> TradingResult<Value> {
        debug!("moex GET {url}");
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| TradingError::Network(format!("moex: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(TradingError::ExchangeApi(format!("moex HTTP {status}")));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| TradingError::Network(format!("moex body: {e}")))?;
        // ISS answers 200 + an HTML passport page for endpoints that need a
        // subscription (e.g. orderbook). Say so instead of "json parse error".
        let head = body.trim_start();
        if !head.starts_with('{') && !head.starts_with('[') {
            return Err(TradingError::Auth(format!(
                "moex: {url} returned non-JSON (endpoint requires ISS authorization)"
            )));
        }
        serde_json::from_str::<Value>(&body)
            .map_err(|e| TradingError::Serialization(format!("moex json: {e}")))
    }

    /// Turn an ISS `marketdata` row into a [`Quote`].
    ///
    /// Mid priority: `LAST` (last trade) → bid/ask midpoint → `MARKETPRICE`.
    /// A row with none of those is "no quote right now", not a zero price.
    pub fn quote_from_marketdata(
        secid: &str,
        map: &HashMap<&str, &Value>,
    ) -> TradingResult<Quote> {
        let bid = as_f64(map.get("BID")).unwrap_or(0.0);
        let ask = as_f64(map.get("OFFER")).unwrap_or(0.0);
        let last = as_f64(map.get("LAST"));
        let market = as_f64(map.get("MARKETPRICE"));

        // A crossed or one-sided touch means the session is closed or the row
        // is stale: never publish bid >= ask as a tradeable quote.
        let touch = if bid > 0.0 && ask > 0.0 && bid < ask {
            Some((bid, ask))
        } else {
            None
        };

        let mid = last
            .or_else(|| touch.map(|(b, a)| (b + a) / 2.0))
            .or(market)
            .filter(|m| m.is_finite() && *m > 0.0)
            .ok_or(TradingError::InsufficientData)?;

        let (bid, ask) = match touch {
            Some((b, a)) => (b, a),
            None => (mid, mid),
        };

        Ok(Quote {
            instrument: InstrumentRef {
                venue: VenueKind::Moex,
                id: secid.to_string(),
                market_type: MarketType::Spot,
            },
            bid,
            ask,
            mid,
            mark: Some(mid),
            // MOEX has no funding rate; the field stays None on purpose.
            funding_rate: None,
            ts: Utc::now(),
        })
    }

    /// Daily or intraday candles for research/backtest ingestion.
    ///
    /// `interval`: 1, 10, 60 (minutes), 24 (day), 7 (week), 31 (month).
    pub async fn candles(
        &self,
        secid: &str,
        from: &str,
        interval: u32,
    ) -> TradingResult<Vec<MoexCandle>> {
        let url = self.shares_path(&format!(
            "securities/{secid}/candles.json?iss.meta=off&from={from}&interval={interval}"
        ));
        let root = self.fetch(&url).await?;
        Self::candles_from_block(&root)
    }

    /// Parse a candles response. Public for fixture tests.
    pub fn candles_from_block(root: &Value) -> TradingResult<Vec<MoexCandle>> {
        let table = Table::from_block(root, "candles")?;
        let mut out = Vec::with_capacity(table.len());
        for map in table.maps() {
            let begin = as_str(map.get("begin")).and_then(parse_iss_ts);
            let end = as_str(map.get("end")).and_then(parse_iss_ts);
            let open = as_f64(map.get("open"));
            let close = as_f64(map.get("close"));
            let (Some(begin), Some(end), Some(open), Some(close)) = (begin, end, open, close)
            else {
                // A malformed row must not poison the whole series.
                continue;
            };
            out.push(MoexCandle {
                begin,
                end,
                open,
                close,
                high: as_f64(map.get("high")).unwrap_or(close),
                low: as_f64(map.get("low")).unwrap_or(close),
                value_rub: as_f64(map.get("value")).unwrap_or(0.0),
                volume: as_f64(map.get("volume")).unwrap_or(0.0),
            });
        }
        Ok(out)
    }

    /// Declared dividends for `secid`.
    ///
    /// The ISS dividends endpoint is not always open: from some networks it
    /// answers with an HTML passport page instead of JSON, in which case this
    /// returns `TradingError::Auth` rather than an empty list, so a caller never
    /// mistakes "no access" for "no dividends declared".
    pub async fn dividends(&self, secid: &str) -> TradingResult<Vec<MoexDividend>> {
        let url = format!(
            "{}/iss/securities/{secid}/dividends.json?iss.meta=off",
            self.base
        );
        let root = self.fetch(&url).await?;
        Self::dividends_from_block(&root)
    }

    /// Parse a dividends response. Public for fixture tests.
    ///
    /// A missing `dividends` block is an error: the open ISS payload for a
    /// security without an authorised dividends feed simply omits the block, and
    /// reporting "0 dividends" there would be a lie that a tax calculation
    /// would happily believe.
    pub fn dividends_from_block(root: &Value) -> TradingResult<Vec<MoexDividend>> {
        if root.get("dividends").is_none() {
            return Err(TradingError::ExchangeApi(
                "moex: dividends block absent (endpoint requires ISS authorization)".into(),
            ));
        }
        let table = Table::from_block(root, "dividends")?;
        let mut out = Vec::new();
        for map in table.maps() {
            let Some(secid) = as_str(map.get("secid")) else {
                continue;
            };
            let Some(close) = as_str(map.get("registryclosedate")) else {
                continue;
            };
            let Some(value) = as_f64(map.get("value")).filter(|v| v.is_finite() && *v > 0.0)
            else {
                continue;
            };
            out.push(MoexDividend {
                secid: secid.to_string(),
                registry_close: close.to_string(),
                value_rub: value,
                currency: as_str(map.get("currencyid")).unwrap_or("RUB").to_string(),
            });
        }
        Ok(out)
    }

    /// Parse an orderbook response into a [`Book`]. Public for fixture tests.
    pub fn book_from_block(secid: &str, root: &Value, depth: usize) -> TradingResult<Book> {
        let table = Table::from_block(root, "orderbook")?;
        let mut bids: Vec<BookLevel> = Vec::new();
        let mut asks: Vec<BookLevel> = Vec::new();
        for map in table.maps() {
            let side = as_str(map.get("BUYSELL")).unwrap_or_default();
            let Some(price) = as_f64(map.get("PRICE")).filter(|p| *p > 0.0) else {
                continue;
            };
            let size = as_f64(map.get("QUANTITY")).unwrap_or(0.0);
            let level = BookLevel { price, size };
            match side {
                "B" | "BID" | "buy" => bids.push(level),
                "S" | "ASK" | "sell" => asks.push(level),
                _ => {}
            }
        }
        // Best prices first, then keep `depth` levels per side.
        bids.sort_by(|a, b| b.price.total_cmp(&a.price));
        asks.sort_by(|a, b| a.price.total_cmp(&b.price));
        bids.truncate(depth);
        asks.truncate(depth);
        Ok(Book {
            instrument_id: secid.to_string(),
            bids,
            asks,
        })
    }
}

#[async_trait]
impl MarketData for MoexClient {
    fn venue(&self) -> VenueKind {
        VenueKind::Moex
    }

    async fn list_instruments(&self) -> TradingResult<Vec<InstrumentRef>> {
        let url = self.shares_path("securities.json?iss.meta=off");
        let root = self.fetch(&url).await?;
        let table = Table::from_block(&root, "securities")?;
        let mut out = Vec::new();
        for map in table.maps() {
            let Some(secid) = as_str(map.get("SECID")) else {
                continue;
            };
            out.push(InstrumentRef {
                venue: VenueKind::Moex,
                id: secid.to_string(),
                market_type: MarketType::Spot,
            });
        }
        if out.is_empty() {
            return Err(TradingError::InsufficientData);
        }
        Ok(out)
    }

    async fn get_quote(&self, id: &InstrumentId) -> TradingResult<Quote> {
        let url = self.shares_path(&format!("securities/{id}.json?iss.meta=off"));
        let root = self.fetch(&url).await?;
        let table = Table::from_block(&root, "marketdata")?;
        let map = table
            .first_map()
            .ok_or(TradingError::InstrumentNotFound(id.clone()))?;
        Self::quote_from_marketdata(id, &map)
    }

    async fn get_orderbook(&self, id: &InstrumentId, depth: usize) -> TradingResult<Book> {
        let url = self.shares_path(&format!("securities/{id}/orderbook.json?iss.meta=off"));
        let root = self.fetch(&url).await?;
        Self::book_from_block(id, &root, depth)
    }
}

/// ISS timestamps are `YYYY-MM-DD HH:MM:SS` in UTC (no zone suffix).
fn parse_iss_ts(raw: &str) -> Option<DateTime<Utc>> {
    chrono::NaiveDateTime::parse_from_str(raw.trim(), "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|dt| dt.and_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECURITIES: &str = r#"{
      "securities": {
        "columns": ["SECID", "SHORTNAME", "PREVPRICE", "LOTSIZE"],
        "data": [
          ["SBER", "Сбербанк", 310.5, 10],
          ["GAZP", "Газпром", 128.2, 10],
          ["", "broken row", 1.0, 1]
        ]
      }
    }"#;

    const MARKETDATA: &str = r#"{
      "marketdata": {
        "columns": ["SECID", "LAST", "BID", "OFFER", "MARKETPRICE"],
        "data": [["SBER", 311.4, 311.3, 311.5, 311.4]]
      }
    }"#;

    const MARKETDATA_NO_TRADES: &str = r#"{
      "marketdata": {
        "columns": ["SECID", "LAST", "BID", "OFFER", "MARKETPRICE"],
        "data": [["SBER", null, null, null, null]]
      }
    }"#;

    const ORDERBOOK: &str = r#"{
      "orderbook": {
        "columns": ["BOARDID", "SECID", "BUYSELL", "PRICE", "QUANTITY"],
        "data": [
          ["TQBR", "SBER", "B", 311.3, 100],
          ["TQBR", "SBER", "S", 311.5, 50],
          ["TQBR", "SBER", "B", 311.2, 200],
          ["TQBR", "SBER", "S", 311.6, 70]
        ]
      }
    }"#;

    const DIVIDENDS: &str = r#"{
      "dividends": {
        "columns": ["secid", "isin", "registryclosedate", "value", "currencyid"],
        "data": [
          ["SBER", "RU0009029540", "2026-07-18", 37.64, "RUB"],
          ["SBER", "RU0009029540", "2025-07-19", 33.3, "RUB"],
          ["SBER", "RU0009029540", "2024-07-20", null, "RUB"],
          ["", "RU0009029540", "2023-07-21", 25.0, "RUB"]
        ]
      }
    }"#;

    const CANDLES: &str = r#"{
      "candles": {
        "columns": ["open", "close", "high", "low", "value", "volume", "begin", "end"],
        "data": [
          [310.0, 311.4, 312.0, 309.5, 1234567.0, 4000, "2026-10-30 07:00:00", "2026-10-30 08:00:00"],
          [311.4, 310.2, 311.9, 310.0, 987654.0, 3200, "2026-10-30 08:00:00", "2026-10-30 09:00:00"],
          ["bad", 1.0, 1.0, 1.0, 0.0, 0, "2026-10-30 09:00:00", "2026-10-30 10:00:00"]
        ]
      }
    }"#;

    fn parse(raw: &str) -> Value {
        serde_json::from_str(raw).expect("fixture json")
    }

    #[test]
    fn securities_list_skips_rows_without_secid() {
        let table = Table::from_block(&parse(SECURITIES), "securities").expect("table");
        assert_eq!(table.len(), 3);
        let ids: Vec<String> = table
            .maps()
            .iter()
            .filter_map(|m| as_str(m.get("SECID")).map(str::to_string))
            .collect();
        assert_eq!(ids, vec!["SBER", "GAZP"]);
    }

    #[test]
    fn quote_prefers_last_trade() {
        let table = Table::from_block(&parse(MARKETDATA), "marketdata").expect("table");
        let map = table.first_map().expect("row");
        let q = MoexClient::quote_from_marketdata("SBER", &map).expect("quote");
        assert_eq!(q.instrument.venue, VenueKind::Moex);
        assert!((q.mid - 311.4).abs() < 1e-9);
        assert!((q.bid - 311.3).abs() < 1e-9);
        assert!((q.ask - 311.5).abs() < 1e-9);
        assert!(q.funding_rate.is_none(), "MOEX has no funding");
    }

    #[test]
    fn crossed_or_one_sided_touch_falls_back_to_mid() {
        // Saturday snapshot: BID 297 / OFFER 295 is crossed, LAST 296.02.
        let root = parse(
            r#"{"marketdata":{"columns":["SECID","LAST","BID","OFFER","MARKETPRICE"],
                 "data":[["SBER",296.02,297.0,295.0,296.02]]}}"#,
        );
        let table = Table::from_block(&root, "marketdata").expect("table");
        let map = table.first_map().expect("row");
        let q = MoexClient::quote_from_marketdata("SBER", &map).expect("quote");
        assert!((q.mid - 296.02).abs() < 1e-9);
        assert!((q.bid - 296.02).abs() < 1e-9, "crossed bid must not leak");
        assert!((q.ask - 296.02).abs() < 1e-9, "crossed ask must not leak");
        assert!(q.bid <= q.ask, "never publish a crossed touch");
    }

    #[test]
    fn quote_without_trades_is_insufficient_data_not_zero() {
        let table = Table::from_block(&parse(MARKETDATA_NO_TRADES), "marketdata").expect("table");
        let map = table.first_map().expect("row");
        let err = MoexClient::quote_from_marketdata("SBER", &map).expect_err("must not be 0");
        assert!(matches!(err, TradingError::InsufficientData), "{err:?}");
    }

    #[test]
    fn orderbook_sorts_best_first_and_truncates() {
        let book = MoexClient::book_from_block("SBER", &parse(ORDERBOOK), 1).expect("book");
        assert_eq!(book.bids.len(), 1);
        assert_eq!(book.asks.len(), 1);
        assert!((book.bids[0].price - 311.3).abs() < 1e-9);
        assert!((book.asks[0].price - 311.5).abs() < 1e-9);

        let full = MoexClient::book_from_block("SBER", &parse(ORDERBOOK), 10).expect("book");
        assert_eq!(full.bids.len(), 2);
        assert!((full.bids[1].price - 311.2).abs() < 1e-9);
    }

    #[test]
    fn candles_parse_and_skip_malformed_rows() {
        let candles = MoexClient::candles_from_block(&parse(CANDLES)).expect("candles");
        assert_eq!(candles.len(), 2, "third row has a non-numeric open");
        assert_eq!(candles[0].open, 310.0);
        assert_eq!(candles[0].close, 311.4);
        assert_eq!(candles[0].begin.to_string(), "2026-10-30 07:00:00 UTC");
        assert_eq!(candles[1].volume, 3200.0);
    }

    #[test]
    fn dividends_block_absent_is_an_error_not_an_empty_list() {
        // Real open payload for SBER: description + boards, no dividends block.
        let root = parse(r#"{"description":{"columns":["name"],"data":[["SECID"]]},"boards":{"columns":["secid"],"data":[["SBER"]]}}"#);
        let err = MoexClient::dividends_from_block(&root).expect_err("must not be empty ok");
        assert!(
            matches!(err, TradingError::ExchangeApi(_)),
            "expected ExchangeApi, got {err:?}"
        );
    }

    #[test]
    fn dividends_parse_and_skip_unusable_rows() {
        let divs = MoexClient::dividends_from_block(&parse(DIVIDENDS)).expect("dividends");
        assert_eq!(divs.len(), 2, "null value and empty secid are skipped");
        assert_eq!(divs[0].secid, "SBER");
        assert_eq!(divs[0].registry_close, "2026-07-18");
        assert!((divs[0].value_rub - 37.64).abs() < 1e-9);
        assert_eq!(divs[0].currency, "RUB");
        assert!((divs[1].value_rub - 33.3).abs() < 1e-9);
    }

    #[test]
    fn missing_block_is_empty_not_an_error() {
        let table = Table::from_block(&parse("{}"), "marketdata").expect("table");
        assert!(table.is_empty());
        assert!(table.first_map().is_none());
        // An empty marketdata block for a real symbol = no quote.
        let root = parse(r#"{"marketdata":{"columns":[],"data":[]}}"#);
        let t = Table::from_block(&root, "marketdata").expect("table");
        assert!(t.first_map().is_none());
    }

    #[test]
    fn malformed_block_reports_serialization_error() {
        let err = Table::from_block(&parse(r#"{"marketdata":{"data":[]}}"#), "marketdata")
            .expect_err("no columns");
        assert!(matches!(err, TradingError::Serialization(_)), "{err:?}");
    }

    /// Live ISS smoke. Run explicitly:
    /// `cargo test -p trading-exchange --lib moex_live -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "network: hits iss.moex.com"]
    async fn moex_live_quote_sber() {
        let c = MoexClient::default();
        let q = c.get_quote(&"SBER".to_string()).await.expect("live quote");
        assert!(q.mid > 0.0, "SBER mid {}", q.mid);
        assert_eq!(q.instrument.venue, VenueKind::Moex);
        println!("SBER bid={} ask={} mid={}", q.bid, q.ask, q.mid);
        // Dividends: same policy as the orderbook - "no access" must never look
        // like "no dividends" (the open payload omits the block entirely).
        match c.dividends("SBER").await {
            Ok(d) => println!("SBER dividends: {} declared", d.len()),
            Err(e) => {
                println!("SBER dividends unavailable: {e}");
                assert!(
                    matches!(e, TradingError::Auth(_) | TradingError::ExchangeApi(_)),
                    "unexpected error kind: {e:?}"
                );
            }
        }

        // ISS orderbook needs a subscription; the client must say that clearly
        // rather than reporting a JSON parse failure.
        match c.get_orderbook(&"SBER".to_string(), 3).await {
            Ok(book) => println!("SBER bids={:?} asks={:?}", book.bids, book.asks),
            Err(e) => {
                println!("SBER orderbook unavailable: {e}");
                assert!(
                    matches!(e, TradingError::Auth(_) | TradingError::ExchangeApi(_)),
                    "unexpected error kind: {e:?}"
                );
            }
        }
    }

    #[test]
    fn client_has_expected_venue_and_board_path() {
        let c = MoexClient::new("https://example.test/").with_board("TQTF");
        assert_eq!(c.venue(), VenueKind::Moex);
        assert_eq!(
            c.shares_path("securities.json?iss.meta=off"),
            "https://example.test/iss/engines/stock/markets/shares/boards/TQTF/securities.json?iss.meta=off"
        );
    }
}
