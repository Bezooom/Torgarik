//! Telegram control for the risk shell: read status, stop, resume.
//!
//! Deliberately narrow. The bot can answer questions and set the
//! [`crate::kill_flag`]; it can never place, size or price an order, so a
//! compromised token costs an outage, not a deposit.
//!
//! Security:
//! - only `TELEGRAM_CHAT_ID` may talk to it; every other chat is ignored;
//! - the token comes from the environment, never from a file in the repo;
//! - `/stop` writes a flag file, `/resume` deletes it. Both are journaled by the
//!   shell, so a remote action is always visible in the audit trail.
//!
//! The parsing and rendering are pure functions with tests; the HTTP loop is a
//! thin adapter around them.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::{info, warn};

use crate::kill_flag;

/// What the operator asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Status,
    Block,
    Stop,
    Resume,
    Help,
    /// Not addressed to this bot (no slash command).
    Ignore,
    /// Addressed to the bot but not understood.
    Unknown(String),
}

/// Parse a Telegram message body into a [`Command`].
///
/// Accepts the `/cmd@botname` form Telegram uses in groups, ignores arguments.
pub fn parse(text: &str) -> Command {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return Command::Ignore;
    };
    let head = rest.split_whitespace().next().unwrap_or("");
    let head = head.split('@').next().unwrap_or(head).to_ascii_lowercase();
    match head.as_str() {
        "status" | "s" => Command::Status,
        "block" | "why" => Command::Block,
        "stop" | "halt" | "kill" => Command::Stop,
        "resume" | "start" | "go" => Command::Resume,
        "help" | "" => Command::Help,
        other => Command::Unknown(other.to_string()),
    }
}

/// Everything a reply needs, gathered once so rendering stays pure.
#[derive(Debug, Clone, Default)]
pub struct StatusSnapshot {
    pub stopped: bool,
    pub stop_reason: Option<String>,
    pub oos_blocking: bool,
    pub oos_reasons: Vec<String>,
    pub report_age: Option<String>,
    pub deflated_sharpe: Option<f64>,
    pub pbo: Option<f64>,
    pub equity: Option<f64>,
    pub daily_pnl: Option<f64>,
    pub last_events: Vec<String>,
}

impl StatusSnapshot {
    /// Load from the same files the shell and dashboard read.
    pub fn load(flag_path: &Path, journal_dir: &Path) -> Self {
        let stopped = kill_flag::is_set(flag_path);
        let stop_reason = kill_flag::read_reason(flag_path);

        let oos = trading_common::OosStats::load();
        let oos_blocking = oos
            .as_ref()
            .map(|s| s.is_blocking_with_settings(&trading_common::OosSettings::default()))
            .unwrap_or(false);
        let oos_reasons = oos
            .as_ref()
            .map(|s| s.block_reasons(&trading_common::OosSettings::default()))
            .unwrap_or_default();

        let mut snap = Self {
            stopped,
            stop_reason,
            oos_blocking,
            oos_reasons,
            deflated_sharpe: oos.as_ref().and_then(|s| s.deflated_sharpe),
            pbo: oos.as_ref().and_then(|s| s.pbo),
            ..Self::default()
        };

        let report = trading_common::OosStats::resolve_report_path();
        if let Some(path) = report {
            if let Ok(meta) = std::fs::metadata(&path) {
                if let Ok(modified) = meta.modified() {
                    if let Ok(age) = modified.elapsed() {
                        snap.report_age = Some(format!("{}h", age.as_secs() / 3600));
                    }
                }
            }
        }

        snap.last_events = tail_events(journal_dir, 5);
        snap
    }
}

/// Last `n` journal lines rendered as `kind: detail` strings.
fn tail_events(dir: &Path, n: usize) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    files.sort();
    let Some(last) = files.last() else {
        return Vec::new();
    };
    let Ok(body) = std::fs::read_to_string(last) else {
        return Vec::new();
    };
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    lines
        .iter()
        .rev()
        .take(n)
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .map(|v| {
            let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("?");
            let extra = v
                .get("reason")
                .or_else(|| v.get("detail"))
                .or_else(|| v.get("text"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if extra.is_empty() {
                kind.to_string()
            } else {
                format!("{kind}: {extra}")
            }
        })
        .collect()
}

/// Render a `/status` reply. Plain text: Telegram markdown is easy to break.
pub fn render_status(s: &StatusSnapshot) -> String {
    let mut out = String::new();
    out.push_str(if s.stopped {
        "STOPPED (entries blocked)\n"
    } else {
        "RUNNING\n"
    });
    if let Some(reason) = &s.stop_reason {
        out.push_str(&format!("stop reason: {reason}\n"));
    }
    out.push_str(if s.oos_blocking {
        "live: LOCKED\n"
    } else {
        "live: unlocked by OOS\n"
    });
    if let Some(d) = s.deflated_sharpe {
        out.push_str(&format!("deflated Sharpe: {d:.3}\n"));
    }
    if let Some(p) = s.pbo {
        out.push_str(&format!("PBO: {p:.2}\n"));
    }
    if let Some(age) = &s.report_age {
        out.push_str(&format!("report age: {age}\n"));
    }
    if let Some(e) = s.equity {
        out.push_str(&format!("equity: {e:.2}\n"));
    }
    if let Some(p) = s.daily_pnl {
        out.push_str(&format!("daily PnL: {p:.2}\n"));
    }
    if !s.last_events.is_empty() {
        out.push_str("recent:\n");
        for ev in &s.last_events {
            out.push_str(&format!("  {ev}\n"));
        }
    }
    out.trim_end().to_string()
}

/// Render a `/block` reply: why live is locked, in the kill-switch's own words.
pub fn render_block(s: &StatusSnapshot) -> String {
    if s.oos_reasons.is_empty() {
        return "no OOS block reasons (report missing or green)".to_string();
    }
    let mut out = String::from("block reasons:\n");
    for r in &s.oos_reasons {
        out.push_str(&format!("- {r}\n"));
    }
    out.trim_end().to_string()
}

/// Reply text for a command, plus the flag side effect it implies.
pub fn respond(cmd: &Command, snap: &StatusSnapshot, flag_path: &Path) -> (String, bool) {
    match cmd {
        Command::Status => (render_status(snap), false),
        Command::Block => (render_block(snap), false),
        Command::Stop => match kill_flag::set(flag_path, "telegram /stop") {
            Ok(()) => (
                "STOPPED: new entries blocked. Open positions still follow their brackets.\nSend /resume to undo.".to_string(),
                true,
            ),
            Err(e) => (format!("failed to write stop flag: {e}"), false),
        },
        Command::Resume => match kill_flag::clear(flag_path) {
            Ok(true) => ("RESUMED: stop flag removed.".to_string(), true),
            Ok(false) => ("nothing to resume: flag was not set.".to_string(), false),
            Err(e) => (format!("failed to remove stop flag: {e}"), false),
        },
        Command::Help => (
            "/status - state, OOS verdict, recent journal\n/block - why live is locked\n/stop - block new entries\n/resume - undo /stop\n/help - this text".to_string(),
            false,
        ),
        Command::Ignore => (String::new(), false),
        Command::Unknown(cmd) => (format!("unknown command /{cmd}. Send /help."), false),
    }
}

// ── Telegram adapter ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Updates {
    #[serde(default)]
    result: Vec<Update>,
}

#[derive(Debug, Deserialize)]
struct Update {
    update_id: i64,
    #[serde(default)]
    message: Option<Message>,
}

#[derive(Debug, Deserialize)]
struct Message {
    chat: Chat,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Chat {
    id: i64,
}

/// Bot configuration from the environment. `None` = bot disabled.
pub struct BotConfig {
    pub token: String,
    pub chat_id: i64,
    pub flag_path: PathBuf,
    pub journal_dir: PathBuf,
}

impl BotConfig {
    /// Read `TELEGRAM_BOT_TOKEN` + `TELEGRAM_CHAT_ID`; either missing = disabled.
    pub fn from_env() -> Option<Self> {
        let token = std::env::var("TELEGRAM_BOT_TOKEN").ok()?;
        let chat_id: i64 = std::env::var("TELEGRAM_CHAT_ID").ok()?.parse().ok()?;
        if token.trim().is_empty() {
            return None;
        }
        Some(Self {
            token,
            chat_id,
            flag_path: kill_flag::default_path(),
            journal_dir: PathBuf::from("runtime/journal"),
        })
    }

    fn url(&self, method: &str) -> String {
        format!("https://api.telegram.org/bot{}/{method}", self.token)
    }
}

/// Long-poll Telegram forever. Returns only on an unrecoverable error.
pub async fn run_bot(cfg: BotConfig) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let mut offset: i64 = 0;
    info!("telegram control: polling as chat {}", cfg.chat_id);
    loop {
        let resp = client
            .get(cfg.url("getUpdates"))
            .query(&[
                ("timeout", "30".to_string()),
                ("offset", offset.to_string()),
            ])
            .send()
            .await;
        let updates = match resp {
            Ok(r) => r.json::<Updates>().await.unwrap_or(Updates { result: vec![] }),
            Err(e) => {
                warn!("telegram getUpdates: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
        };
        for u in updates.result {
            offset = offset.max(u.update_id + 1);
            let Some(msg) = u.message else { continue };
            if msg.chat.id != cfg.chat_id {
                warn!("telegram: ignoring chat {} (not configured)", msg.chat.id);
                continue;
            }
            let Some(text) = msg.text else { continue };
            let cmd = parse(&text);
            if cmd == Command::Ignore {
                continue;
            }
            let snap = StatusSnapshot::load(&cfg.flag_path, &cfg.journal_dir);
            let (reply, changed) = respond(&cmd, &snap, &cfg.flag_path);
            if changed {
                warn!("telegram command {:?} applied", cmd);
            }
            if reply.is_empty() {
                continue;
            }
            let _ = client
                .post(cfg.url("sendMessage"))
                .form(&[("chat_id", cfg.chat_id.to_string()), ("text", reply)])
                .send()
                .await;
        }
    }
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
            .join(format!("tg-{tag}-{nanos}-{}", std::process::id()))
            .join("stop.flag")
    }

    #[test]
    fn parses_commands_including_group_suffix_and_aliases() {
        assert_eq!(parse("/status"), Command::Status);
        assert_eq!(parse("/status@example_bot"), Command::Status);
        assert_eq!(parse("  /STATUS  "), Command::Status);
        assert_eq!(parse("/block"), Command::Block);
        assert_eq!(parse("/why"), Command::Block);
        assert_eq!(parse("/stop now please"), Command::Stop);
        assert_eq!(parse("/halt"), Command::Stop);
        assert_eq!(parse("/resume"), Command::Resume);
        assert_eq!(parse("/help"), Command::Help);
        assert_eq!(parse("/"), Command::Help);
        assert_eq!(parse("/nonsense"), Command::Unknown("nonsense".into()));
    }

    #[test]
    fn ignores_plain_chatter() {
        assert_eq!(parse("hi bot"), Command::Ignore);
        assert_eq!(parse(""), Command::Ignore);
    }

    #[test]
    fn status_render_mentions_every_red_flag() {
        let snap = StatusSnapshot {
            stopped: true,
            stop_reason: Some("telegram /stop".into()),
            oos_blocking: true,
            oos_reasons: vec!["deflated_sharpe -0.100 < 0".into()],
            report_age: Some("3h".into()),
            deflated_sharpe: Some(-0.1),
            pbo: Some(0.76),
            equity: Some(1000.0),
            daily_pnl: Some(-2.5),
            last_events: vec!["risk_block: protection_cooldown".into()],
        };
        let text = render_status(&snap);
        assert!(text.contains("STOPPED"));
        assert!(text.contains("telegram /stop"));
        assert!(text.contains("LOCKED"));
        assert!(text.contains("-0.100"));
        assert!(text.contains("0.76"));
        assert!(text.contains("risk_block"));
        assert!(text.contains("1000.00"));
    }

    #[test]
    fn block_render_lists_reasons_or_says_none() {
        let mut snap = StatusSnapshot::default();
        assert!(render_block(&snap).contains("no OOS block reasons"));
        snap.oos_reasons = vec!["pbo 0.76 > 0.5".into(), "pnl -7.76 < 0".into()];
        let text = render_block(&snap);
        assert!(text.contains("pbo 0.76"));
        assert!(text.contains("pnl -7.76"));
    }

    #[test]
    fn stop_and_resume_write_and_remove_the_flag() {
        let path = tmp("stopresume");
        let snap = StatusSnapshot::default();

        let (reply, changed) = respond(&Command::Stop, &snap, &path);
        assert!(changed);
        assert!(reply.contains("STOPPED"));
        assert!(kill_flag::is_set(&path));
        assert_eq!(
            kill_flag::read_reason(&path).as_deref(),
            Some("telegram /stop")
        );

        let (reply, changed) = respond(&Command::Resume, &snap, &path);
        assert!(changed);
        assert!(reply.contains("RESUMED"));
        assert!(!kill_flag::is_set(&path));

        let (reply, changed) = respond(&Command::Resume, &snap, &path);
        assert!(!changed);
        assert!(reply.contains("nothing to resume"));
    }

    #[test]
    fn read_only_commands_never_touch_the_flag() {
        let path = tmp("readonly");
        let snap = StatusSnapshot::default();
        for cmd in [Command::Status, Command::Block, Command::Help] {
            let (reply, changed) = respond(&cmd, &snap, &path);
            assert!(!changed, "{cmd:?} must be read-only");
            assert!(!reply.is_empty());
            assert!(!kill_flag::is_set(&path));
        }
    }

    #[test]
    fn unknown_command_answers_with_help_hint() {
        let path = tmp("unknown");
        let (reply, changed) = respond(&Command::Unknown("boom".into()), &StatusSnapshot::default(), &path);
        assert!(!changed);
        assert!(reply.contains("/help"));
    }

    #[test]
    fn bot_is_disabled_without_env() {
        // Environment is process-wide; only assert the negative when unset.
        if std::env::var("TELEGRAM_BOT_TOKEN").is_err() {
            assert!(BotConfig::from_env().is_none());
        }
    }
}
