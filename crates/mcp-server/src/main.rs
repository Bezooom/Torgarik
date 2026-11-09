//! MCP server for advisory copilot access to Hyperliquid market data.
//!
//! Focus: S1 / Risk Shell tooling — not multi-venue PM routing.
//! Speaks JSON-RPC over stdin/stdout (Claude Desktop / agent hosts).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::sync::Arc;

use trading_common::{OosStats, PortfolioSnapshot, Settings, VenueKind};
use trading_exchange::{build_exchange_pair, Exchange, ExchangeHandle, HyperliquidClient};

use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

struct AppState {
    exchange: ExchangeHandle,
    hl: Arc<HyperliquidClient>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let settings = Settings::default();
    let bankroll = settings.risk.starting_bankroll;
    let (exchange, hl) = match build_exchange_pair(&settings.exchange, settings.mode, bankroll) {
        Ok(pair) => pair,
        Err(e) => {
            // Fall back to public HL client without signer (read-only tools).
            eprintln!("build_exchange_pair: {e}; using public Hyperliquid client");
            let hl = Arc::new(HyperliquidClient::new(
                &settings.exchange.hyperliquid_base_url,
                &settings.exchange.wallet_address,
                settings.exchange.proxy_url.as_deref(),
            )?);
            (hl.clone() as ExchangeHandle, hl)
        }
    };

    let mut state = AppState { exchange, hl };

    let stdin = io::stdin();
    let handle = stdin.lock();
    let mut stdout = io::stdout();

    for line in handle.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        if line.trim().is_empty() {
            continue;
        }

        if let Ok(req) = serde_json::from_str::<JsonRpcRequest>(&line) {
            let res_val = handle_mcp_request(&req, &mut state).await;
            let id_val = req.id.unwrap_or(Value::Null);

            let response = match res_val {
                Ok(val) => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: id_val,
                    result: Some(val),
                    error: None,
                },
                Err(err_msg) => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: id_val,
                    result: None,
                    error: Some(json!({ "code": -32603, "message": err_msg })),
                },
            };

            let out_str =
                serde_json::to_string(&response).map_err(|e| format!("serialize response: {e}"))?;
            writeln!(stdout, "{out_str}")?;
            stdout.flush()?;
        }
    }

    Ok(())
}

async fn handle_mcp_request(req: &JsonRpcRequest, state: &mut AppState) -> Result<Value, String> {
    match req.method.as_str() {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "trading-hl-mcp-server",
                "version": "0.1.0"
            }
        })),
        "tools/list" => Ok(json!({
            "tools": [
                {
                    "name": "list_instruments",
                    "description": "List tradeable Hyperliquid instruments (perps)",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "get_quote",
                    "description": "Fetch mid/bid/ask/funding for a coin (e.g. ETH)",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "coin": { "type": "string" }
                        },
                        "required": ["coin"]
                    }
                },
                {
                    "name": "rank_funding",
                    "description": "Rank coins by absolute funding rate (top opportunities)",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": { "type": "integer", "default": 10 },
                            "min_oi": { "type": "number", "default": 0.0 }
                        }
                    }
                },
                {
                    "name": "get_portfolio",
                    "description": "Query current portfolio equity, cash, and positions",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "get_kill_switch_status",
                    "description": "OOS kill-switch status: is live auto-alpha / live shell execute blocked and why (reads data/reports/wf_latest.json)",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "get_shell_status",
                    "description": "Latest risk-shell heartbeat / oos_status / risk_block from runtime/journal",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "tail": { "type": "integer", "default": 20 }
                        }
                    }
                }
            ]
        })),
        "tools/call" => {
            let params = req.params.as_ref().ok_or("Missing params")?;
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or("Missing tool name")?;
            let args = params.get("arguments").cloned().unwrap_or(json!({}));

            match name {
                "list_instruments" => {
                    let instruments = state
                        .exchange
                        .list_instruments()
                        .await
                        .map_err(|e| e.to_string())?;
                    let text =
                        serde_json::to_string_pretty(&instruments).map_err(|e| e.to_string())?;
                    Ok(json!({ "content": [{ "type": "text", "text": text }] }))
                }
                "get_quote" => {
                    let coin = args
                        .get("coin")
                        .and_then(|v| v.as_str())
                        .ok_or("coin required")?;
                    // Ensure market data is fresh when possible.
                    let _ = state.hl.refresh_market_data().await;
                    let quote = state
                        .exchange
                        .get_quote(&coin.to_string())
                        .await
                        .map_err(|e| e.to_string())?;
                    let text = serde_json::to_string_pretty(&quote).map_err(|e| e.to_string())?;
                    Ok(json!({ "content": [{ "type": "text", "text": text }] }))
                }
                "rank_funding" => {
                    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                    let min_oi = args.get("min_oi").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let min_abs = args
                        .get("min_abs_funding")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let _ = state.hl.refresh_market_data().await;
                    let rows = state
                        .hl
                        .rank_by_funding(min_abs, min_oi)
                        .await
                        .map_err(|e| e.to_string())?;
                    let top: Vec<_> = rows.into_iter().take(limit).collect();
                    let text = serde_json::to_string_pretty(&top).map_err(|e| e.to_string())?;
                    Ok(json!({ "content": [{ "type": "text", "text": text }] }))
                }
                "get_portfolio" => {
                    let portfolio: PortfolioSnapshot =
                        state.exchange.portfolio().await.unwrap_or_else(|_| {
                            PortfolioSnapshot::empty(VenueKind::Hyperliquid, 0.0)
                        });
                    let text =
                        serde_json::to_string_pretty(&portfolio).map_err(|e| e.to_string())?;
                    Ok(json!({ "content": [{ "type": "text", "text": text }] }))
                }
                "get_kill_switch_status" => {
                    let payload = kill_switch_status_json();
                    let text = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
                    Ok(json!({ "content": [{ "type": "text", "text": text }] }))
                }
                "get_shell_status" => {
                    let tail = args.get("tail").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
                    let payload = shell_status_json(tail);
                    let text = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
                    Ok(json!({ "content": [{ "type": "text", "text": text }] }))
                }
                _ => Err(format!("Unknown tool: {name}")),
            }
        }
        _ => Err(format!("Unknown method: {}", req.method)),
    }
}

fn kill_switch_status_json() -> Value {
    match OosStats::load() {
        Some(s) => {
            let blocking = s.is_blocking();
            json!({
                "available": true,
                "blocking": blocking,
                "live_auto_alpha_blocked": blocking,
                "live_shell_execute_blocked": blocking,
                "paper_shell_allowed": true,
                "s1_manual_allowed": true,
                "avg_sharpe": s.avg_sharpe,
                "avg_pnl": s.avg_pnl,
                "n_folds": s.n_folds,
                "profitable_folds": s.profitable_folds,
                "holdout_blocking": s.holdout_blocking,
                "report": OosStats::default_report_path().display().to_string(),
                "headline": if blocking {
                    "LIVE AUTO-ALPHA BLOCKED (OOS red)"
                } else {
                    "OOS metrics green — still need CONFIRM_LIVE + checklist (docs/LIVE_UNLOCK.md)"
                },
                "rule": "blocking if avg_sharpe < 0 OR avg_pnl < 0 OR holdout red",
                "checklist": "docs/LIVE_UNLOCK.md"
            })
        }
        None => json!({
            "available": false,
            "blocking": true,
            "live_auto_alpha_blocked": true,
            "live_shell_execute_blocked": true,
            "paper_shell_allowed": true,
            "s1_manual_allowed": true,
            "holdout_blocking": false,
            "headline": "LIVE BLOCKED — no parseable wf_latest.json",
            "report": OosStats::default_report_path().display().to_string(),
            "checklist": "docs/LIVE_UNLOCK.md"
        }),
    }
}

fn shell_status_json(tail: usize) -> Value {
    let dir = PathBuf::from("runtime/journal");
    let mut lines: Vec<String> = Vec::new();
    if dir.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "jsonl").unwrap_or(false))
            .collect();
        files.sort();
        for f in files {
            if let Ok(text) = std::fs::read_to_string(f) {
                for line in text.lines() {
                    if !line.trim().is_empty() {
                        lines.push(line.to_string());
                    }
                }
            }
        }
    }
    let start = lines.len().saturating_sub(tail.max(5));
    let events: Vec<Value> = lines[start..]
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    let last_hb = events
        .iter()
        .rev()
        .find(|e| e.get("kind").and_then(|k| k.as_str()) == Some("heartbeat"));
    let last_oos = events
        .iter()
        .rev()
        .find(|e| e.get("kind").and_then(|k| k.as_str()) == Some("oos_status"));
    let last_rb = events
        .iter()
        .rev()
        .find(|e| e.get("kind").and_then(|k| k.as_str()) == Some("risk_block"));

    json!({
        "journal_dir": "runtime/journal",
        "events_returned": events.len(),
        "last_heartbeat": last_hb,
        "last_oos_status": last_oos,
        "last_risk_block": last_rb,
        "tail": events.iter().rev().take(tail).cloned().collect::<Vec<_>>(),
    })
}
