//! Local ops dashboard: S1 · risk shell · journal · honest backtest snapshot.
//! Not a live trading UI. Reads `runtime/` + `data/reports/` from the workspace root.

use chrono::{Datelike, NaiveDate, Utc};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tracing::{info, warn};

static ROOT: OnceLock<PathBuf> = OnceLock::new();

const BANNER: &str = r#"
 _____                            _ __  
|_   _|__  _ __ __ _  __ _  _ __ (_) | __
  | |/ _ \| '__/ _` |/ _` || '__|| | |/ /
  | | (_) | | | (_| | (_| || |   | |   < 
  |_|\___/|_|  \__,_|\__,_||_|   |_|_|\_\
--- Torgarik Risk Shell v1.0 [Hyperliquid] ---
"#;

fn main() {
    println!("{BANNER}");
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let root = discover_root();
    ROOT.set(root.clone()).ok();
    info!(path = %root.display(), "workspace root");

    let port: u16 = std::env::var("DASHBOARD_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);
    let bind = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&bind).unwrap_or_else(|e| {
        panic!("bind {bind}: {e} (set DASHBOARD_PORT or free the port)");
    });
    info!("Торгарик ops dashboard → http://127.0.0.1:{port}");
    info!("S1 / shell / journal only — no live order placement from this UI");

    let mut req_n: u64 = 0;
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let _ = stream.set_nodelay(true);
                let mut buf = [0u8; 16384];
                let n = match stream.read(&mut buf) {
                    Ok(n) if n > 0 => n,
                    _ => continue,
                };
                let req = String::from_utf8_lossy(&buf[..n]);
                let (method, path) = parse_request_line(&req);
                let path_only = path.split('?').next().unwrap_or(path.as_str());
                req_n += 1;

                let (status, content_type, body) = match (method.as_str(), path_only) {
                    ("GET", "/") | ("GET", "/index.html") => (
                        "200 OK",
                        "text/html; charset=utf-8",
                        include_str!("index.html").to_string(),
                    ),
                    ("GET", "/api/health") | ("GET", "/api/ping") => (
                        "200 OK",
                        "application/json",
                        json!({
                            "ok": true,
                            "service": "dashboard",
                            "n": req_n,
                            "ts": Utc::now().to_rfc3339(),
                        })
                        .to_string(),
                    ),
                    ("GET", "/api/status") => match build_status() {
                        Ok(mut v) => {
                            if let Some(obj) = v.as_object_mut() {
                                obj.insert("req_n".into(), json!(req_n));
                            }
                            ("200 OK", "application/json", v.to_string())
                        }
                        Err(e) => {
                            warn!("status error: {e}");
                            (
                                "500 Internal Server Error",
                                "application/json",
                                json!({"error": e}).to_string(),
                            )
                        }
                    },
                    ("OPTIONS", _) => ("204 No Content", "text/plain", String::new()),
                    _ => (
                        "404 Not Found",
                        "application/json",
                        json!({"error": "not found", "path": path_only}).to_string(),
                    ),
                };

                info!(n = req_n, %method, path = %path_only, "http");
                let response = format!(
                    "HTTP/1.1 {status}\r\n\
                     Content-Type: {content_type}\r\n\
                     Access-Control-Allow-Origin: *\r\n\
                     Access-Control-Allow-Methods: GET, OPTIONS\r\n\
                     Access-Control-Allow-Headers: *\r\n\
                     Cache-Control: no-store, no-cache, must-revalidate\r\n\
                     Connection: close\r\n\
                     Content-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
            Err(e) => warn!("accept: {e}"),
        }
    }
}

fn parse_request_line(req: &str) -> (String, String) {
    let line = req.lines().next().unwrap_or("GET /");
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    (method, path)
}

fn discover_root() -> PathBuf {
    if let Ok(p) = std::env::var("TRADING_ROOT") {
        let pb = PathBuf::from(p);
        if pb.is_dir() {
            return pb;
        }
    }
    let mut cur = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for _ in 0..8 {
        if cur.join("runtime").is_dir() && cur.join("Cargo.toml").is_file() {
            return cur;
        }
        // running from crates/dashboard
        if cur.join("../../runtime").is_dir() && cur.join("../../Cargo.toml").is_file() {
            return cur.join("../..").canonicalize().unwrap_or(cur.join("../.."));
        }
        if !cur.pop() {
            break;
        }
    }
    PathBuf::from(".")
}

fn root() -> &'static Path {
    ROOT.get().map(|p| p.as_path()).unwrap_or_else(|| Path::new("."))
}

fn build_status() -> Result<Value, String> {
    let state = load_state();
    let s1 = load_s1();
    // Larger tail so risk_block / oos_status still visible after heartbeats.
    let journal = load_journal_tail(200);
    let signals = load_signals_tail(30);
    let backtest = load_backtest_summary();
    let equity_series = equity_from_journal(&journal, &state);
    let live = shell_from_journal(&journal, &state);
    let risk_blocks = extract_journal_kinds(&journal, "risk_block", 15);
    let kill_switches = extract_journal_kinds(&journal, "kill_switch", 5);
    let oos_journal = extract_journal_kinds(&journal, "oos_status", 3);
    let block = build_block_status(&backtest, &live, &risk_blocks, &kill_switches, &oos_journal);

    Ok(json!({
        "status": "online",
        "ts": Utc::now().to_rfc3339(),
        "root": root().display().to_string(),
        "policy": {
            "program": "S1",
            "label": "Только shell + доход навыком",
            "primary_venue": "hyperliquid",
            "live_auto_alpha": false,
            "live_l1_emit": false,
            "new_bot_deposits": false,
            "on": ["risk_shell", "journal", "skill_work", "paper_hobby"],
            "frozen": ["alpha_live", "l1_emit_live", "new_bot_deposits"],
            "on_ru": ["риск-shell", "журнал", "навык / фриланс", "paper-хобби"],
            "frozen_ru": ["live alpha", "L1 emit live", "новые депозиты в бота"],
            "honesty": "Auto-alpha на OOS красный — live торговля заморожена. Этот дашборд только смотрит runtime/ и отчёты, ордера не шлёт."
        },
        "block": block,
        "shell": live,
        "s1": s1,
        "journal": journal.iter().rev().take(40).cloned().collect::<Vec<_>>(),
        "risk_blocks": risk_blocks,
        "kill_switches": kill_switches,
        "oos_journal": oos_journal,
        "signals": signals,
        "backtest": backtest,
        "equity_series": equity_series,
        "docs": [
            {"id": "s1", "title": "S1: доход навыком", "path": "docs/S1_SHELL_INCOME.md"},
            {"id": "shell", "title": "Risk shell", "path": "docs/RISK_SHELL.md"},
            {"id": "ops", "title": "Runbook", "path": "docs/OPS.md"},
            {"id": "backtest", "title": "Бэктест", "path": "docs/BACKTEST.md"},
            {"id": "stats", "title": "DSR / PBO", "path": "docs/STATS.md"},
            {"id": "prereg", "title": "Pre-registration", "path": "docs/PREREG.md"},
            {"id": "rf", "title": "РФ-контур", "path": "docs/RF.md"},
            {"id": "guide", "title": "Гайд", "path": "docs/GUIDE.md"},
            {"id": "dashboard", "title": "Дашборд", "path": "docs/DASHBOARD.md"},
            {"id": "index", "title": "Все документы", "path": "docs/README.md"},
        ]
    }))
}

/// Last N events of a given journal `kind` (newest first).
fn extract_journal_kinds(journal: &[Value], kind: &str, n: usize) -> Vec<Value> {
    journal
        .iter()
        .rev()
        .filter(|e| e.get("kind").and_then(|k| k.as_str()) == Some(kind))
        .take(n)
        .cloned()
        .collect()
}

/// Single answer to: «заблокирован ли live и почему?»
fn build_block_status(
    backtest: &Value,
    shell: &Value,
    risk_blocks: &[Value],
    kill_switches: &[Value],
    oos_journal: &[Value],
) -> Value {
    let mut reasons: Vec<Value> = Vec::new();
    let mut live_auto_blocked = true; // default safe: blocked unless OOS green
    let mut live_execute_blocked = true;

    let oos_available = backtest
        .get("available")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let oos_ready = backtest
        .get("oos_ready_for_live")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let oos_blocking = backtest
        .get("oos_blocking")
        .and_then(|v| v.as_bool())
        .unwrap_or(!oos_ready);

    if !oos_available {
        reasons.push(json!({
            "code": "oos_missing",
            "severity": "high",
            "text": "Нет OOS-отчёта (data/reports). Live auto-alpha заблокирован."
        }));
    } else if oos_blocking {
        let sharpe = backtest.get("avg_test_sharpe").cloned().unwrap_or(json!(null));
        let pnl = backtest.get("avg_test_pnl").cloned().unwrap_or(json!(null));
        let age = backtest.get("report_age_human").and_then(|v| v.as_str()).unwrap_or("?");
        reasons.push(json!({
            "code": "oos_red",
            "severity": "high",
            "text": format!(
                "OOS kill-switch: avg Sharpe={sharpe}, avg PnL={pnl} (отчёт {age}). Live auto + live shell --execute → dry."
            )
        }));
    } else {
        live_auto_blocked = false;
        live_execute_blocked = false;
        reasons.push(json!({
            "code": "oos_green",
            "severity": "info",
            "text": "OOS зелёный по метрикам — live всё равно требует CONFIRM_LIVE + keys + чеклист."
        }));
    }

    if let Some(ks) = kill_switches.first() {
        reasons.push(json!({
            "code": "daily_kill_switch",
            "severity": "high",
            "text": format!(
                "Дневной kill-switch в journal (daily_pnl={}, equity={})",
                ks.get("daily_pnl").map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                ks.get("equity").map(|v| v.to_string()).unwrap_or_else(|| "?".into())
            )
        }));
        live_execute_blocked = true;
    }

    let shell_status = shell.get("status").and_then(|v| v.as_str()).unwrap_or("unknown");
    if shell_status == "stopped" || shell_status == "stale" {
        reasons.push(json!({
            "code": "shell_not_alive",
            "severity": "medium",
            "text": format!("Risk shell {shell_status} (нет свежего heartbeat ≤45s). S1 shield не активен.")
        }));
    } else {
        reasons.push(json!({
            "code": "shell_ok",
            "severity": "info",
            "text": format!(
                "Shell {} mode={} equity={}",
                shell_status,
                shell.get("mode").and_then(|v| v.as_str()).unwrap_or("?"),
                shell.get("equity").map(|v| v.to_string()).unwrap_or_else(|| "?".into())
            )
        }));
    }

    if let Some(rb) = risk_blocks.first() {
        reasons.push(json!({
            "code": "last_risk_block",
            "severity": "medium",
            "text": format!(
                "Последний risk_block: {} — {}",
                rb.get("reason").and_then(|v| v.as_str()).unwrap_or("?"),
                rb.get("detail").and_then(|v| v.as_str()).unwrap_or("")
            )
        }));
    }

    if let Some(oj) = oos_journal.first() {
        reasons.push(json!({
            "code": "shell_oos_snapshot",
            "severity": "info",
            "text": format!(
                "Shell oos_status: blocking={} sharpe={} pnl={}",
                oj.get("blocking").map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                oj.get("avg_sharpe").map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                oj.get("avg_pnl").map(|v| v.to_string()).unwrap_or_else(|| "?".into())
            )
        }));
    }

    let headline = if live_auto_blocked {
        "LIVE AUTO-ALPHA ЗАБЛОКИРОВАН"
    } else {
        "OOS не блокирует live (проверь CONFIRM_LIVE)"
    };

    let summary_ru = if live_auto_blocked {
        "Оператор: S1 + shell (dry). Не размораживать alpha."
    } else {
        "OOS metrics ok — live unlock только по полному чеклисту."
    };

    // Plain-language copy for first-time operators (UI + MCP).
    let verdict_plain_ru = if live_auto_blocked {
        "Реальными деньгами бот сейчас НЕ торгует. Это нормально и безопасно."
    } else {
        "Метрики OOS зелёные, но live всё равно включается только вручную по чеклисту."
    };
    let money_question_ru = if live_auto_blocked {
        "Можно ли сейчас торговать реальными деньгами через авто-стратегию?"
    } else {
        "Авто-стратегия не заблокирована OOS — но live всё ещё требует подтверждения."
    };
    let money_answer_ru = if live_auto_blocked {
        "НЕТ — live auto-alpha заморожен"
    } else {
        "Условно — только после CONFIRM_LIVE + ключей + чеклиста"
    };

    let mut next_actions_ru: Vec<Value> = Vec::new();
    let mut alerts: Vec<Value> = Vec::new();

    if live_auto_blocked {
        next_actions_ru.push(json!({
            "id": "stay_dry",
            "priority": 1,
            "title": "Торгуйте только в учебном режиме",
            "detail": "Запускайте risk shell без --execute. Реальные ордера не уйдут.",
            "cmd": "cargo run -p ml-trader -- --mode shell"
        }));
        next_actions_ru.push(json!({
            "id": "s1_hours",
            "priority": 2,
            "title": "Запишите часы навыка (S1)",
            "detail": "Основной путь заработка — практика и дисциплина, не авто-бот.",
            "cmd": "python3 scripts/s1_daily_log.py --hours 2 --track skill --shell --note \"...\""
        }));
        if oos_available {
            next_actions_ru.push(json!({
                "id": "read_oos",
                "priority": 3,
                "title": "Поймите, почему live закрыт",
                "detail": "Красный OOS/holdout = стратегия на истории теряла деньги. Это защита, не баг.",
                "cmd": "bash scripts/ready.sh"
            }));
        } else {
            next_actions_ru.push(json!({
                "id": "run_backtest",
                "priority": 3,
                "title": "Соберите отчёт OOS",
                "detail": "Без walk-forward отчёта kill-switch считает систему неготовой.",
                "cmd": "cargo run -p backtester -- --walk-forward --preset ethsol --report data/reports/wf_latest.json"
            }));
        }
    } else {
        next_actions_ru.push(json!({
            "id": "checklist",
            "priority": 1,
            "title": "Пройдите LIVE_UNLOCK чеклист",
            "detail": "Зелёный OOS ≠ «жми live». Нужны ключи, CONFIRM_LIVE=1 и осознанное решение.",
            "cmd": "less docs/LIVE_UNLOCK.md"
        }));
    }

    if shell_status == "stopped" || shell_status == "stale" {
        next_actions_ru.insert(0, json!({
            "id": "start_shell",
            "priority": 0,
            "title": "Запустите risk shell",
            "detail": "Без shell нет «щита» (лимиты, журнал, kill-switch). Откройте второй терминал.",
            "cmd": "cargo run -p ml-trader -- --mode shell"
        }));
        alerts.push(json!({
            "id": "shell_down",
            "level": "warning",
            "title": "Risk shell не активен",
            "body": "Нет свежего heartbeat. Защита капитала и журнал не работают, пока shell выключен.",
            "sticky": true
        }));
    }

    if oos_blocking || !oos_available {
        alerts.push(json!({
            "id": "oos_lock",
            "level": "danger",
            "title": "Live auto-торговля заблокирована",
            "body": "Система специально не даёт включить авто-стратегию на реальные деньги, пока OOS красный или отсутствует.",
            "sticky": true
        }));
    }

    if !kill_switches.is_empty() {
        alerts.push(json!({
            "id": "daily_ks",
            "level": "danger",
            "title": "Сработал дневной стоп-убыток",
            "body": "Зафиксирован kill-switch по дневному убытку. Сегодня новые рисковые действия блокируются.",
            "sticky": true
        }));
    }

    if backtest
        .get("report_stale")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        alerts.push(json!({
            "id": "report_stale",
            "level": "warning",
            "title": "Отчёт OOS устарел",
            "body": "Файл walk-forward старше 7 дней. Перезапустите бэктест, чтобы цифры были актуальны.",
            "sticky": false
        }));
    }

    alerts.push(json!({
        "id": "ui_readonly",
        "level": "info",
        "title": "Этот экран только для просмотра",
        "body": "Дашборд не отправляет ордера. Даже зелёный статус здесь не значит, что деньги уже торгуются.",
        "sticky": false
    }));

    json!({
        "headline": headline,
        "summary_ru": summary_ru,
        "verdict_plain_ru": verdict_plain_ru,
        "money_question_ru": money_question_ru,
        "money_answer_ru": money_answer_ru,
        "next_actions_ru": next_actions_ru,
        "alerts": alerts,
        "live_auto_alpha_blocked": live_auto_blocked,
        "live_shell_execute_blocked": live_execute_blocked,
        "paper_shell_allowed": true,
        "s1_manual_allowed": true,
        "reasons": reasons,
        "checked_at": Utc::now().to_rfc3339(),
    })
}

/// Prefer live heartbeats from journal over stale runtime/state.json.
fn shell_from_journal(journal: &[Value], state: &Value) -> Value {
    let mut last_hb: Option<&Value> = None;
    for e in journal.iter().rev() {
        if e.get("kind").and_then(|k| k.as_str()) == Some("heartbeat") {
            last_hb = Some(e);
            break;
        }
    }

    let equity = last_hb
        .and_then(|e| e.get("equity").and_then(|v| v.as_f64()))
        .or_else(|| state.get("equity").and_then(|v| v.as_f64()))
        .unwrap_or(0.0);
    let daily_pnl = last_hb
        .and_then(|e| e.get("daily_pnl").and_then(|v| v.as_f64()))
        .or_else(|| state.get("daily_pnl").and_then(|v| v.as_f64()))
        .unwrap_or(0.0);
    let available = last_hb
        .and_then(|e| e.get("available").cloned())
        .or_else(|| state.get("available").cloned())
        .unwrap_or(json!(null));
    let n_pos = last_hb
        .and_then(|e| e.get("n_positions").and_then(|v| v.as_u64()))
        .map(|n| n as usize)
        .or_else(|| {
            state
                .get("positions")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
        })
        .unwrap_or(0);
    let mode = last_hb
        .and_then(|e| e.get("mode").and_then(|v| v.as_str()))
        .unwrap_or("unknown");
    let hb_ts = last_hb
        .and_then(|e| e.get("ts").and_then(|v| v.as_str()))
        .map(|s| s.to_string());

    let age_secs = hb_ts.as_deref().and_then(|ts| {
        chrono::DateTime::parse_from_rfc3339(ts)
            .ok()
            .map(|t| (Utc::now() - t.with_timezone(&Utc)).num_seconds().max(0))
    });
    let alive = age_secs.map(|a| a <= 45).unwrap_or(false);

    json!({
        "equity": equity,
        "daily_pnl": daily_pnl,
        "available": available,
        "venue": state.get("venue").and_then(|v| v.as_str()).unwrap_or("hyperliquid"),
        "status": if alive { "running" } else if hb_ts.is_some() { "stale" } else { "stopped" },
        "strategy": state.get("strategy").cloned().unwrap_or(json!("shell")),
        "n_positions": n_pos,
        "positions": state.get("positions").cloned().unwrap_or(json!([])),
        "watch": state.get("watch").cloned().unwrap_or(json!([])),
        "state_ts": state.get("ts").cloned().unwrap_or(json!(null)),
        "last_heartbeat_ts": hb_ts,
        "heartbeat_age_secs": age_secs,
        "shell_alive": alive,
        "mode": mode,
        "cycle": state.get("cycle").cloned().unwrap_or(json!(null)),
        "source": if last_hb.is_some() { "journal_heartbeat" } else { "state_json" },
    })
}

fn load_state() -> Value {
    let path = root().join("runtime/state.json");
    read_json_file(&path).unwrap_or_else(|| {
        json!({
            "equity": 0.0,
            "available": 0.0,
            "daily_pnl": 0.0,
            "positions": [],
            "status": "no_state",
            "venue": "hyperliquid",
            "strategy": "shell",
            "watch": []
        })
    })
}

fn load_s1() -> Value {
    let path = root().join("runtime/s1/hours.csv");
    let week_target = 15.0_f64;
    let shell_target_7 = 5_u32;
    let month_target_h = 60.0_f64;

    if !path.exists() {
        return json!({
            "has_data": false,
            "week_hours": 0.0,
            "week_target": week_target,
            "month_hours": 0.0,
            "month_target_hours": month_target_h,
            "shell_days_7": 0,
            "shell_target_7": shell_target_7,
            "shell_days_month": 0,
            "tracks": {},
            "recent": [],
            "need_hours_week": week_target,
        });
    }

    let text = fs::read_to_string(&path).unwrap_or_default();
    let mut rows: Vec<Value> = Vec::new();
    let mut rdr = text.lines();
    let header = rdr.next().unwrap_or("");
    let cols: Vec<&str> = header.split(',').collect();
    let idx = |name: &str| cols.iter().position(|c| *c == name);

    let i_date = idx("date");
    let i_hours = idx("hours");
    let i_track = idx("track");
    let i_shell = idx("shell");
    let i_note = idx("note");
    let i_mood = idx("mood");

    for line in rdr {
        if line.trim().is_empty() {
            continue;
        }
        let fields = parse_csv_line(line);
        let get = |i: Option<usize>| {
            i.and_then(|ix| fields.get(ix).map(|s| s.as_str()))
                .unwrap_or("")
                .to_string()
        };
        rows.push(json!({
            "date": get(i_date),
            "hours": get(i_hours).parse::<f64>().unwrap_or(0.0),
            "track": get(i_track),
            "shell": get(i_shell),
            "note": get(i_note),
            "mood": get(i_mood),
        }));
    }

    let today = Utc::now().date_naive();
    let week_start = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let month_start = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap_or(today);
    let last7 = today - chrono::Duration::days(6);

    let parse_d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();

    let mut week_h = 0.0;
    let mut month_h = 0.0;
    let mut shell_week = 0_u32;
    let mut shell_7 = 0_u32;
    let mut shell_month = 0_u32;
    let mut tracks: BTreeMap<String, f64> = BTreeMap::new();
    let mut hours_by_day: BTreeMap<String, f64> = BTreeMap::new();

    for r in &rows {
        let dstr = r["date"].as_str().unwrap_or("");
        let Some(d) = parse_d(dstr) else { continue };
        let h = r["hours"].as_f64().unwrap_or(0.0);
        let shell = r["shell"]
            .as_str()
            .map(|s| {
                matches!(
                    s.to_ascii_lowercase().as_str(),
                    "yes" | "y" | "1" | "true"
                )
            })
            .unwrap_or(false);
        let track = r["track"].as_str().unwrap_or("?").to_string();

        if d >= week_start {
            week_h += h;
            if shell {
                shell_week += 1;
            }
        }
        if d >= month_start {
            month_h += h;
            *tracks.entry(track).or_default() += h;
            if shell {
                shell_month += 1;
            }
        }
        if d >= last7 {
            *hours_by_day.entry(dstr.to_string()).or_default() += h;
            if shell {
                shell_7 += 1;
            }
        }
    }

    let recent: Vec<Value> = rows.iter().rev().take(14).cloned().collect();
    let need = (week_target - week_h).max(0.0);

    json!({
        "has_data": true,
        "week_hours": round2(week_h),
        "week_target": week_target,
        "week_start": week_start.to_string(),
        "month_hours": round2(month_h),
        "month_target_hours": month_target_h,
        "shell_days_7": shell_7,
        "shell_days_week": shell_week,
        "shell_target_7": shell_target_7,
        "shell_days_month": shell_month,
        "need_hours_week": round2(need),
        "tracks": tracks,
        "hours_by_day": hours_by_day,
        "recent": recent,
    })
}

/// Minimal CSV line parser (handles quoted fields with commas).
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_q && chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_q = !in_q;
                }
            }
            ',' if !in_q => {
                out.push(cur.clone());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn load_journal_tail(n: usize) -> Vec<Value> {
    let dir = root().join("runtime/journal");
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .map(|e| e == "jsonl")
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    let mut lines: Vec<String> = Vec::new();
    for f in files {
        if let Ok(text) = fs::read_to_string(&f) {
            for line in text.lines() {
                if !line.trim().is_empty() {
                    lines.push(line.to_string());
                }
            }
        }
    }
    let start = lines.len().saturating_sub(n);
    lines[start..]
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

fn load_signals_tail(n: usize) -> Vec<Value> {
    let path = root().join("runtime/signals.jsonl");
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let start = lines.len().saturating_sub(n);
    lines[start..]
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

fn load_backtest_summary() -> Value {
    // Prefer kill-switch report first (aligned with OosStats / RiskShell).
    let prefer = [
        "wf_latest.json",
        "wf_trend_ethsol_realistic.json",
        "wf_trend_core_realistic.json",
    ];
    let dir = root().join("data/reports");
    let path = prefer
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.exists())
        .or_else(|| {
            fs::read_dir(&dir).ok().and_then(|rd| {
                let mut files: Vec<PathBuf> = rd
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
                    .collect();
                files.sort();
                files.pop()
            })
        });

    let Some(path) = path else {
        return json!({
            "available": false,
            "oos_blocking": true,
            "oos_ready_for_live": false,
            "message": "Нет data/reports/*.json. Запустите walk-forward (docs/BACKTEST.md)."
        });
    };

    let meta = fs::metadata(&path).ok();
    let mtime = meta.as_ref().and_then(|m| m.modified().ok());

    let Some(raw) = read_json_file(&path) else {
        return json!({
            "available": false,
            "oos_blocking": true,
            "oos_ready_for_live": false,
            "message": "не удалось прочитать отчёт"
        });
    };

    // Prefer in-report generated_at (schema 1.1); fall back to file mtime.
    let generated_at = raw
        .get("generated_at")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            mtime.map(|t| {
                let dt: chrono::DateTime<Utc> = t.into();
                dt.to_rfc3339()
            })
        });
    let age_secs = generated_at
        .as_deref()
        .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
        .map(|t| (Utc::now() - t.with_timezone(&Utc)).num_seconds().max(0))
        .or_else(|| {
            mtime.map(|t| {
                std::time::SystemTime::now()
                    .duration_since(t)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0)
            })
        });
    let age_human = age_secs.map(format_age_secs);

    let folds = raw
        .get("folds")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut test_pnls: Vec<f64> = Vec::new();
    let mut test_sharpes: Vec<f64> = Vec::new();
    let mut n_neg = 0usize;
    for f in &folds {
        if let Some(test) = f.get("test") {
            if let Some(pnl) = test.get("total_pnl").and_then(|v| v.as_f64()) {
                test_pnls.push(pnl);
                if pnl < 0.0 {
                    n_neg += 1;
                }
            }
            if let Some(s) = test.get("sharpe_ratio").and_then(|v| v.as_f64()) {
                test_sharpes.push(s);
            }
        }
    }

    // Prefer top-level aggregates when present (wf_latest style).
    let avg_pnl = raw
        .get("test_total_pnl")
        .and_then(|v| v.as_f64())
        .map(|total| {
            if test_pnls.is_empty() {
                total
            } else {
                total / test_pnls.len() as f64
            }
        })
        .unwrap_or_else(|| {
            if test_pnls.is_empty() {
                0.0
            } else {
                test_pnls.iter().sum::<f64>() / test_pnls.len() as f64
            }
        });
    // For kill-switch parity with OosStats: use test_avg_sharpe if present.
    let avg_sharpe = raw
        .get("test_avg_sharpe")
        .and_then(|v| v.as_f64())
        .unwrap_or_else(|| {
            if test_sharpes.is_empty() {
                0.0
            } else {
                test_sharpes.iter().sum::<f64>() / test_sharpes.len() as f64
            }
        });
    // Fold-mean PnL for display; blocking uses OR like OosStats (sharpe<0 || pnl<0).
    let fold_mean_pnl = if test_pnls.is_empty() {
        avg_pnl
    } else {
        test_pnls.iter().sum::<f64>() / test_pnls.len() as f64
    };
    let top_total_pnl = raw
        .get("test_total_pnl")
        .and_then(|v| v.as_f64())
        .unwrap_or(fold_mean_pnl);
    let oos_blocking = avg_sharpe < 0.0 || fold_mean_pnl < 0.0 || top_total_pnl < 0.0;
    let oos_ok = !oos_blocking && avg_sharpe > 0.0 && fold_mean_pnl > 0.0;
    let stale = age_secs.map(|a| a > 7 * 24 * 3600).unwrap_or(false);

    let mut block_reasons = Vec::new();
    if avg_sharpe < 0.0 {
        block_reasons.push(format!("avg Sharpe {avg_sharpe:.3} < 0"));
    }
    if fold_mean_pnl < 0.0 || top_total_pnl < 0.0 {
        block_reasons.push(format!(
            "PnL negative (fold_mean={:.2}, total={:.2})",
            fold_mean_pnl, top_total_pnl
        ));
    }
    if stale {
        block_reasons.push(format!(
            "report older than 7d ({})",
            age_human.clone().unwrap_or_else(|| "?".into())
        ));
    }

    let holdout = raw.get("holdout").cloned().unwrap_or(json!(null));
    let holdout_blocking = raw
        .get("holdout")
        .map(|h| {
            let pnl = h.get("total_pnl").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let sh = h.get("avg_sharpe").and_then(|v| v.as_f64()).unwrap_or(0.0);
            // Fail closed on NaN/Inf (same policy as OosStats).
            !pnl.is_finite() || !sh.is_finite() || pnl < 0.0 || sh < 0.0
        })
        .unwrap_or(false);
    if holdout_blocking {
        block_reasons.push("holdout red (pnl or sharpe < 0)".into());
    }

    // Selection-bias verdicts (schema 1.2): worst-fold deflated Sharpe and PBO.
    let deflated_sharpe = raw.get("deflated_sharpe_min").and_then(|v| v.as_f64());
    let pbo = raw.get("pbo_avg").and_then(|v| v.as_f64());
    let dsr_blocking = deflated_sharpe
        .map(|d| !d.is_finite() || d <= 0.0)
        .unwrap_or(false);
    if let Some(d) = deflated_sharpe {
        if dsr_blocking {
            block_reasons.push(format!("deflated Sharpe {d:.3} <= 0 (selection bias)"));
        }
    }
    let pbo_blocking = pbo.map(|p| !p.is_finite() || p > 0.5).unwrap_or(false);
    if let Some(p) = pbo {
        if pbo_blocking {
            block_reasons.push(format!("PBO {p:.2} > 0.5 (overfit likely)"));
        }
    }

    json!({
        "available": true,
        "file": path.file_name().and_then(|s| s.to_str()).unwrap_or("?").to_string(),
        "path": path.display().to_string(),
        "schema_version": raw.get("schema_version").cloned().unwrap_or(json!(null)),
        "generated_at": generated_at,
        "report_age_secs": age_secs,
        "report_age_human": age_human,
        "report_stale": stale,
        "alpha": raw.get("alpha").cloned().unwrap_or(json!(null)),
        "strategy": raw.get("strategy").cloned().unwrap_or(json!(null)),
        "window": raw.get("window").cloned().unwrap_or(json!(null)),
        "exec": raw.get("exec").cloned().unwrap_or(json!(null)),
        "n_coins": raw.get("n_coins").cloned().unwrap_or(json!(null)),
        "n_folds": raw.get("n_folds").cloned().unwrap_or(json!(folds.len())),
        "n_test_samples": test_pnls.len(),
        "profitable_test_folds": raw.get("profitable_test_folds").cloned().unwrap_or(json!(null)),
        "test_total_pnl": round2(top_total_pnl),
        "avg_test_pnl": round2(fold_mean_pnl),
        "avg_test_sharpe": round2(avg_sharpe),
        "test_win_rate": raw.get("test_win_rate").cloned().unwrap_or(json!(null)),
        "n_negative_folds": n_neg,
        "holdout": holdout,
        "holdout_blocking": holdout_blocking,
        "oos_blocking": oos_blocking || holdout_blocking || dsr_blocking || pbo_blocking,
        "oos_ready_for_live": oos_ok && !stale && !holdout_blocking,
        "block_reasons": block_reasons,
        "realism_notes": raw.get("realism_notes").cloned().unwrap_or(json!([])),
        "verdict": if oos_ok && !stale && !holdout_blocking {
            "OOS metrics green — before live still full checklist in docs/BACKTEST.md."
        } else if oos_blocking || holdout_blocking || dsr_blocking || pbo_blocking {
            "OOS kill-switch ACTIVE (Sharpe < 0 or PnL < 0, and/or holdout red). S1 + shell dry only."
        } else {
            "OOS not ready for live auto-alpha. Primary path: S1 + risk shell."
        }
    })
}

fn format_age_secs(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{:.1}h", secs as f64 / 3600.0)
    } else {
        format!("{:.1}d", secs as f64 / 86400.0)
    }
}

fn equity_from_journal(journal: &[Value], state: &Value) -> Value {
    let mut labels = Vec::new();
    let mut values = Vec::new();
    for e in journal {
        if e.get("kind").and_then(|k| k.as_str()) == Some("heartbeat") {
            if let Some(eq) = e.get("equity").and_then(|v| v.as_f64()) {
                let ts = e
                    .get("ts")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .chars()
                    .take(19)
                    .collect::<String>()
                    .replace('T', " ");
                labels.push(if ts.is_empty() {
                    format!("{}", labels.len())
                } else {
                    ts
                });
                values.push(eq);
            }
        }
    }
    if values.is_empty() {
        if let Some(eq) = state.get("equity").and_then(|v| v.as_f64()) {
            labels.push("state".into());
            values.push(eq);
        }
    }
    // keep last 40 points
    let start = values.len().saturating_sub(40);
    json!({
        "labels": labels[start..].to_vec(),
        "values": values[start..].to_vec(),
    })
}

fn read_json_file(path: &Path) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}
