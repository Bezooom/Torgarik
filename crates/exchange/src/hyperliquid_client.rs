//! Hyperliquid REST client: info + signed exchange orders.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use ethers::signers::{LocalWallet, Signer};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use tracing::{debug, info};
use trading_common::{
    InstrumentId, InstrumentRef, MarketType, OrderAck, OrderRequest, OrderSide, OrderStatus,
    PortfolioSnapshot, Position, Quote, TimeInForce, TradingError, TradingResult, VenueKind,
};

use crate::hl_signer::{
    cancel_action_value, float_to_wire, order_action_value, sign_l1_action, wallet_from_private_key,
};
use crate::traits::{Book, BookLevel, Exchange};

#[derive(Debug, Clone)]
struct AssetMeta {
    index: u32,
    sz_decimals: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct MarketData {
    pub mid: f64,
    pub funding: Option<f64>,
    pub mark: Option<f64>,
    pub open_interest: Option<f64>,
}

#[derive(Debug, Clone, Default)]
struct AssetMetaCache {
    meta: HashMap<String, AssetMeta>,
    universe: Vec<String>,
}

/// Hyperliquid public + optional authenticated trading client.
pub struct HyperliquidClient {
    client: Client,
    base_url: String,
    wallet_address: String,
    wallet: Option<LocalWallet>,
    is_mainnet: bool,
    market_data_cache: tokio::sync::RwLock<HashMap<String, MarketData>>,
    asset_meta_cache: tokio::sync::RwLock<AssetMetaCache>,
}

impl HyperliquidClient {
    pub fn new(base_url: &str, wallet_address: &str, proxy_url: Option<&str>) -> TradingResult<Self> {
        Self::with_signer(base_url, wallet_address, None, proxy_url)
    }

    pub fn with_signer(
        base_url: &str,
        wallet_address: &str,
        private_key: Option<&str>,
        proxy_url: Option<&str>,
    ) -> TradingResult<Self> {
        let mut builder = Client::builder().timeout(std::time::Duration::from_secs(20));
        if let Some(p) = proxy_url.filter(|s| !s.is_empty()) {
            let proxy = reqwest::Proxy::all(p)
                .map_err(|e| TradingError::Config(format!("invalid proxy_url: {e}")))?;
            builder = builder.proxy(proxy);
            info!("HyperliquidClient using explicit proxy");
        }
        let client = builder
            .build()
            .map_err(|e| TradingError::Network(e.to_string()))?;

        let wallet = match private_key.filter(|k| !k.is_empty()) {
            Some(pk) => Some(wallet_from_private_key(pk)?),
            None => None,
        };

        let mut addr = wallet_address.to_string();
        if addr.is_empty() {
            if let Some(w) = &wallet {
                addr = format!("{:?}", w.address());
            }
        }
        // ethers Debug address is lowercase 0x...
        addr = addr.to_lowercase();
        if !addr.starts_with("0x") && addr.len() == 40 {
            addr = format!("0x{addr}");
        }

        let is_mainnet = !base_url.contains("testnet");

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            wallet_address: addr,
            wallet,
            is_mainnet,
            market_data_cache: tokio::sync::RwLock::new(HashMap::new()),
            asset_meta_cache: tokio::sync::RwLock::new(AssetMetaCache::default()),
        })
    }

    fn info_url(&self) -> String {
        format!("{}/info", self.base_url)
    }

    fn exchange_url(&self) -> String {
        format!("{}/exchange", self.base_url)
    }

    async fn post_info_raw(&self, body: JsonValue) -> TradingResult<JsonValue> {
        let resp = self
            .client
            .post(self.info_url())
            .json(&body)
            .send()
            .await
            .map_err(|e| TradingError::Network(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| TradingError::Network(e.to_string()))?;
        if !status.is_success() {
            return Err(TradingError::Api(format!(
                "HL info HTTP {status}: {}",
                text.chars().take(240).collect::<String>()
            )));
        }
        serde_json::from_str(&text).map_err(|e| TradingError::Serialization(e.to_string()))
    }

    async fn post_info<T: for<'de> Deserialize<'de>>(&self, body: JsonValue) -> TradingResult<T> {
        let v = self.post_info_raw(body).await?;
        serde_json::from_value(v).map_err(|e| TradingError::Serialization(e.to_string()))
    }

    /// Refresh mids, funding, mark, OI, asset indices.
    ///
    /// Fetches `allMids` and `metaAndAssetCtxs` **concurrently** (`tokio::join!`).
    pub async fn refresh_market_data(&self) -> TradingResult<()> {
        let (mids_res, raw_res) = tokio::join!(
            self.post_info::<HashMap<String, String>>(json!({ "type": "allMids" })),
            self.post_info_raw(json!({ "type": "metaAndAssetCtxs" })),
        );

        let mids = mids_res?;
        let raw = raw_res?;

        let mut mid_map = HashMap::new();
        for (k, v) in mids {
            if let Ok(px) = v.parse::<f64>() {
                mid_map.insert(k, px);
            }
        }

        let mut funding_map = HashMap::new();
        let mut mark_map = HashMap::new();
        let mut oi_map = HashMap::new();
        let mut names = Vec::new();
        let mut meta_map = HashMap::new();

        if let Some(arr) = raw.as_array() {
            let meta = arr.first();
            let ctxs = arr.get(1).and_then(|v| v.as_array());
            if let (Some(meta), Some(ctxs)) = (meta, ctxs) {
                let universe = meta
                    .get("universe")
                    .and_then(|u| u.as_array())
                    .cloned()
                    .unwrap_or_default();

                for (i, asset) in universe.iter().enumerate() {
                    if asset.get("isDelisted").and_then(|x| x.as_bool()) == Some(true) {
                        continue;
                    }
                    let name = asset
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    if name.is_empty() {
                        continue;
                    }
                    let sz_decimals = asset
                        .get("szDecimals")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(3) as u32;
                    names.push(name.clone());
                    meta_map.insert(
                        name.clone(),
                        AssetMeta {
                            index: i as u32,
                            sz_decimals,
                        },
                    );
                    if let Some(ctx) = ctxs.get(i) {
                        if let Some(f) = parse_f64_field(ctx, "funding") {
                            funding_map.insert(name.clone(), f);
                        }
                        if let Some(m) = parse_f64_field(ctx, "markPx") {
                            mark_map.insert(name.clone(), m);
                        }
                        if let Some(oi) = parse_f64_field(ctx, "openInterest") {
                            oi_map.insert(name.clone(), oi);
                        }
                    }
                }
            }
        }

        debug!(
            "HL refresh: {} mids, {} funding, {} assets",
            mid_map.len(),
            funding_map.len(),
            names.len()
        );

        let mut market_data_map = HashMap::new();
        for (coin, mid) in mid_map {
            let funding = funding_map.get(&coin).copied();
            let mark = mark_map.get(&coin).copied();
            let open_interest = oi_map.get(&coin).copied();
            market_data_map.insert(coin, MarketData {
                mid,
                funding,
                mark,
                open_interest,
            });
        }

        *self.market_data_cache.write().await = market_data_map;

        let mut meta_cache = self.asset_meta_cache.write().await;
        meta_cache.meta = meta_map;
        if !names.is_empty() {
            meta_cache.universe = names;
        }
        Ok(())
    }

    /// Rank coins by |funding| (desc). Returns (coin, funding, mid, oi).
    pub async fn rank_by_funding(&self, min_abs_funding: f64, min_oi: f64) -> TradingResult<Vec<FundingOpp>> {
        if self.market_data_cache.read().await.is_empty() {
            self.refresh_market_data().await?;
        }
        let market_data = self.market_data_cache.read().await;

        let mut rows: Vec<FundingOpp> = market_data
            .iter()
            .filter_map(|(coin, data)| {
                let funding = data.funding.unwrap_or(0.0);
                if funding.abs() < min_abs_funding {
                    return None;
                }
                let open_interest = data.open_interest.unwrap_or(0.0);
                if open_interest < min_oi {
                    return None;
                }
                Some(FundingOpp {
                    coin: coin.clone(),
                    funding,
                    mid: data.mid,
                    open_interest,
                })
            })
            .collect();
        rows.sort_by(|a, b| b.funding.abs().partial_cmp(&a.funding.abs()).unwrap_or(std::cmp::Ordering::Equal));
        Ok(rows)
    }

    fn instrument_ref(id: &str) -> InstrumentRef {
        InstrumentRef {
            venue: VenueKind::Hyperliquid,
            id: id.to_string(),
            market_type: MarketType::Perp,
        }
    }

    fn round_size(size: f64, sz_decimals: u32) -> f64 {
        let mult = 10f64.powi(sz_decimals as i32);
        (size * mult).floor() / mult
    }

    fn round_price(price: f64) -> f64 {
        // HL: max 5 significant figures for perps typically; use 5 sig + wire
        if price <= 0.0 {
            return price;
        }
        let digits = 5;
        let exp = price.abs().log10().floor() as i32 - (digits as i32 - 1);
        let mult = 10f64.powi(-exp);
        (price * mult).round() / mult
    }

    async fn post_signed_action(&self, action: rmpv::Value) -> TradingResult<JsonValue> {
        let wallet = self
            .wallet
            .as_ref()
            .ok_or_else(|| TradingError::Auth("no private key configured for live HL".into()))?;

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| TradingError::External(format!("system clock before UNIX_EPOCH: {e}")))?
            .as_millis() as u64;

        let sig = sign_l1_action(wallet, &action, None, nonce, None, self.is_mainnet).await?;

        // Convert rmpv action to JSON for wire (API expects JSON body, hash used msgpack)
        let action_json = rmpv_to_json(&action)?;

        let body = json!({
            "action": action_json,
            "nonce": nonce,
            "signature": {
                "r": sig.r,
                "s": sig.s,
                "v": sig.v,
            }
        });

        let resp = self
            .client
            .post(self.exchange_url())
            .json(&body)
            .send()
            .await
            .map_err(|e| TradingError::Network(e.to_string()))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| TradingError::Network(e.to_string()))?;

        if !status.is_success() {
            return Err(TradingError::Api(format!(
                "HL exchange HTTP {status}: {}",
                text.chars().take(300).collect::<String>()
            )));
        }

        let v: JsonValue = serde_json::from_str(&text)
            .map_err(|e| TradingError::Serialization(e.to_string()))?;

        if v.get("status").and_then(|s| s.as_str()) != Some("ok") {
            return Err(TradingError::Api(format!("HL exchange error: {text}")));
        }
        Ok(v)
    }
}

/// Ranked funding opportunity.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FundingOpp {
    pub coin: String,
    pub funding: f64,
    pub mid: f64,
    pub open_interest: f64,
}

fn parse_f64_field(obj: &JsonValue, key: &str) -> Option<f64> {
    obj.get(key)
        .and_then(|x| x.as_str().and_then(|s| s.parse().ok()).or_else(|| x.as_f64()))
}

fn rmpv_to_json(v: &rmpv::Value) -> TradingResult<JsonValue> {
    Ok(match v {
        rmpv::Value::Nil => JsonValue::Null,
        rmpv::Value::Boolean(b) => JsonValue::Bool(*b),
        rmpv::Value::Integer(i) => {
            if let Some(u) = i.as_u64() {
                JsonValue::Number(u.into())
            } else if let Some(n) = i.as_i64() {
                JsonValue::Number(n.into())
            } else {
                return Err(TradingError::Serialization("int overflow".into()));
            }
        }
        rmpv::Value::F32(f) => JsonValue::from(*f as f64),
        rmpv::Value::F64(f) => JsonValue::from(*f),
        rmpv::Value::String(s) => JsonValue::String(s.as_str().unwrap_or("").to_string()),
        rmpv::Value::Binary(_) => {
            return Err(TradingError::Serialization("binary not supported".into()))
        }
        rmpv::Value::Array(a) => {
            let mut out = Vec::new();
            for x in a {
                out.push(rmpv_to_json(x)?);
            }
            JsonValue::Array(out)
        }
        rmpv::Value::Map(m) => {
            let mut map = serde_json::Map::new();
            for (k, val) in m {
                let key = match k {
                    rmpv::Value::String(s) => s.as_str().unwrap_or("").to_string(),
                    other => other.to_string(),
                };
                map.insert(key, rmpv_to_json(val)?);
            }
            JsonValue::Object(map)
        }
        rmpv::Value::Ext(_, _) => {
            return Err(TradingError::Serialization("ext not supported".into()))
        }
    })
}

#[async_trait]
impl Exchange for HyperliquidClient {
    fn venue(&self) -> VenueKind {
        VenueKind::Hyperliquid
    }

    async fn list_instruments(&self) -> TradingResult<Vec<InstrumentRef>> {
        if self.asset_meta_cache.read().await.universe.is_empty() {
            self.refresh_market_data().await?;
        }
        Ok(self
            .asset_meta_cache
            .read()
            .await
            .universe
            .iter()
            .map(|id| Self::instrument_ref(id))
            .collect())
    }

    async fn get_quote(&self, id: &InstrumentId) -> TradingResult<Quote> {
        if !self.market_data_cache.read().await.contains_key(id) {
            self.refresh_market_data().await?;
        }
        let cache = self.market_data_cache.read().await;
        let data = cache
            .get(id)
            .ok_or_else(|| TradingError::InstrumentNotFound(id.clone()))?;
        let mid = data.mid;
        let mark = data.mark;
        let funding = data.funding;
        let half = mid * 0.00005;
        Ok(Quote {
            instrument: Self::instrument_ref(id),
            bid: mid - half,
            ask: mid + half,
            mid,
            mark,
            funding_rate: funding,
            ts: Utc::now(),
        })
    }

    async fn get_orderbook(&self, id: &InstrumentId, depth: usize) -> TradingResult<Book> {
        #[derive(Deserialize)]
        struct L2Level {
            px: String,
            sz: String,
        }
        #[derive(Deserialize)]
        struct L2Book {
            levels: Option<Vec<Vec<L2Level>>>,
        }

        let book: L2Book = self
            .post_info(json!({ "type": "l2Book", "coin": id }))
            .await
            .unwrap_or(L2Book { levels: None });

        let mut bids = Vec::new();
        let mut asks = Vec::new();
        if let Some(levels) = book.levels {
            if let Some(bid_side) = levels.first() {
                for l in bid_side.iter().take(depth) {
                    if let (Ok(p), Ok(s)) = (l.px.parse(), l.sz.parse()) {
                        bids.push(BookLevel { price: p, size: s });
                    }
                }
            }
            if let Some(ask_side) = levels.get(1) {
                for l in ask_side.iter().take(depth) {
                    if let (Ok(p), Ok(s)) = (l.px.parse(), l.sz.parse()) {
                        asks.push(BookLevel { price: p, size: s });
                    }
                }
            }
        }
        if bids.is_empty() && asks.is_empty() {
            let q = self.get_quote(id).await?;
            bids.push(BookLevel {
                price: q.bid,
                size: 1.0,
            });
            asks.push(BookLevel {
                price: q.ask,
                size: 1.0,
            });
        }
        Ok(Book {
            instrument_id: id.clone(),
            bids,
            asks,
        })
    }

    async fn place_order(&self, req: &OrderRequest) -> TradingResult<OrderAck> {
        let wallet = match &self.wallet {
            Some(w) => w,
            None => {
                return Err(TradingError::Auth(
                    "live HL place_order needs WALLET_PRIVATE_KEY; use PaperExchange for dry-run"
                        .into(),
                ));
            }
        };
        let _ = wallet;

        if self.asset_meta_cache.read().await.meta.is_empty() {
            self.refresh_market_data().await?;
        }
        let meta = self
            .asset_meta_cache
            .read()
            .await
            .meta
            .get(&req.instrument.id)
            .cloned()
            .ok_or_else(|| TradingError::InstrumentNotFound(req.instrument.id.clone()))?;

        let quote = self.get_quote(&req.instrument.id).await?;
        let raw_px = match req.side {
            OrderSide::Buy => req.price.unwrap_or(quote.ask * 1.001),
            OrderSide::Sell => req.price.unwrap_or(quote.bid * 0.999),
        };
        let px = Self::round_price(raw_px);
        let sz = Self::round_size(req.size, meta.sz_decimals);
        if sz <= 0.0 {
            return Err(TradingError::InvalidOrder(format!(
                "size rounded to 0 (szDecimals={})",
                meta.sz_decimals
            )));
        }
        // HL min notional ~$10
        if px * sz < 10.0 {
            return Err(TradingError::InvalidOrder(format!(
                "notional ${:.2} < $10 min",
                px * sz
            )));
        }

        let tif = match req.tif {
            TimeInForce::Ioc => "Ioc",
            TimeInForce::Fok => "Ioc",
            TimeInForce::Gtc => "Gtc",
        };
        let is_buy = matches!(req.side, OrderSide::Buy);
        let action = order_action_value(meta.index, is_buy, px, sz, req.reduce_only, tif)?;

        info!(
            "HL LIVE order {:?} {} {} @ {} reduce={}",
            req.side, sz, req.instrument.id, px, req.reduce_only
        );

        let resp = self.post_signed_action(action).await?;
        let status_arr = resp
            .pointer("/response/data/statuses")
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default();

        let first = status_arr.first().cloned().unwrap_or(JsonValue::Null);
        if let Some(err) = first.get("error").and_then(|e| e.as_str()) {
            return Err(TradingError::Api(format!("HL order error: {err}")));
        }

        let (exchange_id, filled, avg, status) = if let Some(resting) = first.get("resting") {
            let oid = resting
                .get("oid")
                .and_then(|o| o.as_u64().or_else(|| o.as_i64().map(|i| i as u64)))
                .map(|o| o.to_string());
            (oid, 0.0, None, OrderStatus::Accepted)
        } else if let Some(filled_obj) = first.get("filled") {
            let oid = filled_obj
                .get("oid")
                .and_then(|o| o.as_u64().or_else(|| o.as_i64().map(|i| i as u64)))
                .map(|o| o.to_string());
            let total_sz = filled_obj
                .get("totalSz")
                .and_then(|x| x.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or(sz);
            let avg_px = filled_obj
                .get("avgPx")
                .and_then(|x| x.as_str())
                .and_then(|s| s.parse().ok());
            (oid, total_sz, avg_px, OrderStatus::Filled)
        } else {
            (None, 0.0, None, OrderStatus::Accepted)
        };

        // silence unused wire helper warning path
        let _ = float_to_wire(px);

        Ok(OrderAck {
            client_id: uuid::Uuid::new_v4().to_string(),
            exchange_id,
            status,
            filled_size: filled,
            avg_price: avg.or(Some(px)),
        })
    }

    async fn cancel_order(&self, exchange_id: &str) -> TradingResult<()> {
        if self.wallet.is_none() {
            return Err(TradingError::Auth("no private key for cancel".into()));
        }

        // Format: coin:oid (e.g. BTC:12345)
        if let Some((coin, oid_s)) = exchange_id.split_once(':') {
            let oid: u64 = oid_s
                .parse()
                .map_err(|_| TradingError::InvalidOrder(format!("bad oid: {oid_s}")))?;
            let meta = self
                .asset_meta_cache
                .read()
                .await
                .meta
                .get(coin)
                .cloned()
                .ok_or_else(|| TradingError::InstrumentNotFound(coin.into()))?;
            let action = cancel_action_value(meta.index, oid);
            self.post_signed_action(action).await?;
            return Ok(());
        }

        Err(TradingError::InvalidOrder(
            "cancel_order expects exchange_id format coin:oid (e.g. BTC:12345)".into(),
        ))
    }

    async fn portfolio(&self) -> TradingResult<PortfolioSnapshot> {
        if self.wallet_address.is_empty() {
            return Ok(PortfolioSnapshot::empty(VenueKind::Hyperliquid, 0.0));
        }

        let raw = self
            .post_info_raw(json!({
                "type": "clearinghouseState",
                "user": self.wallet_address,
            }))
            .await?;

        let mut equity = 0.0;
        if let Some(ms) = raw
            .get("marginSummary")
            .or_else(|| raw.get("crossMarginSummary"))
        {
            if let Some(v) = parse_f64_field(ms, "accountValue") {
                equity = v;
            }
        }

        let mut positions = Vec::new();
        if let Some(arr) = raw.get("assetPositions").and_then(|a| a.as_array()) {
            for ap in arr {
                let pos = ap.get("position").unwrap_or(ap);
                let coin = pos
                    .get("coin")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                let szi = parse_f64_field(pos, "szi").unwrap_or(0.0);
                if szi.abs() < 1e-12 {
                    continue;
                }
                let entry = parse_f64_field(pos, "entryPx").unwrap_or(0.0);
                let upnl = parse_f64_field(pos, "unrealizedPnl").unwrap_or(0.0);
                positions.push(Position {
                    instrument_id: coin,
                    side: if szi >= 0.0 {
                        OrderSide::Buy
                    } else {
                        OrderSide::Sell
                    },
                    size: szi.abs(),
                    entry_price: entry,
                    unrealized_pnl: upnl,
                    timestamp: Utc::now(),
                    market_id: None,
                    outcome_id: None,
                });
            }
        }

        let available = parse_f64_field(&raw, "withdrawable").unwrap_or(equity);

        Ok(PortfolioSnapshot {
            equity,
            available,
            positions,
            daily_pnl: 0.0,
            venue: VenueKind::Hyperliquid,
            ts: Utc::now(),
        })
    }
}

pub fn build_hyperliquid(
    base_url: &str,
    wallet: &str,
    proxy: Option<&str>,
) -> TradingResult<Arc<HyperliquidClient>> {
    let pk = std::env::var("WALLET_PRIVATE_KEY")
        .or_else(|_| std::env::var("HL_API_PRIVATE_KEY"))
        .ok();
    Ok(Arc::new(HyperliquidClient::with_signer(
        base_url,
        wallet,
        pk.as_deref(),
        proxy,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fetch_all_mids_live_network() {
        let client = HyperliquidClient::new("https://api.hyperliquid.xyz", "", None).unwrap();
        match client.refresh_market_data().await {
            Ok(()) => {
                let q = client.get_quote(&"BTC".to_string()).await.unwrap();
                assert!(q.mid > 0.0);
                let opps = client.rank_by_funding(1e-6, 0.0).await.unwrap();
                assert!(!opps.is_empty());
                println!(
                    "top funding: {} {:+.6e}",
                    opps[0].coin, opps[0].funding
                );
            }
            Err(e) => eprintln!("skip network test: {e}"),
        }
    }
}
