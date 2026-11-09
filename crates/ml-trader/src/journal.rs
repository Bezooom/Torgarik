//! Append-only trade / risk / signal journal (JSONL).
//!
//! Each line is a JSON object with a `version` field and a `kind` discriminant:
//! ```json
//! {"version":"1.0","kind":"heartbeat","ts":"...","equity":123.4,"available":50.0,...}
//! ```
//!
//! Compatibility policy (fail-soft):
//! - missing `version` → accept with warning (legacy lines)
//! - same major as [`JOURNAL_VERSION`] (e.g. `1.x`) → accept (minor drift OK)
//! - different major (e.g. `2.0`) → **reject** with error log (do not feed dashboard)
//! - unknown `kind` / malformed JSON → reject / skip

use std::fs::{create_dir_all, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{error, warn};

/// Journal schema version (bumped when event fields or semantics change).
/// Major.minor — only major breaks readers.
pub const JOURNAL_VERSION: &str = "1.0";

/// Major component of [`JOURNAL_VERSION`].
pub fn journal_major() -> u32 {
    parse_major(JOURNAL_VERSION).unwrap_or(1)
}

/// Returns true if `version` is empty (legacy) or shares the current major.
pub fn is_version_compatible(version: &str) -> bool {
    if version.is_empty() {
        return true;
    }
    match parse_major(version) {
        Some(m) => m == journal_major(),
        None => false,
    }
}

fn parse_major(version: &str) -> Option<u32> {
    let major = version.split('.').next()?.trim();
    major.parse().ok()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JournalEvent {
    Heartbeat {
        ts: DateTime<Utc>,
        equity: f64,
        available: f64,
        daily_pnl: f64,
        n_positions: usize,
        mode: String,
    },
    RiskBlock {
        ts: DateTime<Utc>,
        reason: String,
        detail: String,
    },
    KillSwitch {
        ts: DateTime<Utc>,
        daily_pnl: f64,
        equity: f64,
        limit: f64,
    },
    ExternalSignal {
        ts: DateTime<Utc>,
        source: String,
        coin: String,
        side: String,
        size: f64,
        note: String,
    },
    OrderIntent {
        ts: DateTime<Utc>,
        coin: String,
        side: String,
        size: f64,
        reduce_only: bool,
        reasoning: String,
        dry_run: bool,
    },
    OrderResult {
        ts: DateTime<Utc>,
        coin: String,
        status: String,
        filled: f64,
        avg_price: Option<f64>,
        ok: bool,
        error: Option<String>,
    },
    Anomaly {
        ts: DateTime<Utc>,
        coin: String,
        name: String,
        value: f64,
        context: String,
    },
    /// OOS / kill-switch advisory snapshot (shell startup or state change).
    OosStatus {
        ts: DateTime<Utc>,
        avg_sharpe: f64,
        avg_pnl: f64,
        n_folds: usize,
        profitable_folds: usize,
        blocking: bool,
        note: String,
    },
    /// Locally managed bracket (SL/TP) lifecycle.
    ///
    /// - `action`: "register" | "trigger_sl" | "trigger_tp" | "removed"
    /// - `level`: triggering level (SL/TP price; representative level for
    ///   register/removed)
    /// - `mid`: observed mid at decision time (`0.0` when not observed, e.g. removal)
    Bracket {
        ts: DateTime<Utc>,
        coin: String,
        action: String,
        level: f64,
        mid: f64,
    },
    Note {
        ts: DateTime<Utc>,
        text: String,
    },
}

/// Wrapper that adds the `version` field around a `JournalEvent` on write.
#[derive(Debug, Serialize)]
struct JournalLine {
    version: &'static str,
    #[serde(flatten)]
    event: JournalEvent,
}

/// Deserialization helper that accepts versioned and versionless events.
#[derive(Debug, Deserialize)]
struct JournalLineRaw {
    #[serde(default = "default_version")]
    version: String,
    #[serde(flatten)]
    event: JournalEvent,
}

fn default_version() -> String {
    String::new()
}

/// Result of parsing a single journal line.
#[derive(Debug)]
pub enum JournalEntry {
    /// Successfully parsed event (possibly with a version warning).
    Ok {
        event: JournalEvent,
        had_version: bool,
    },
    /// Line was valid JSON but the `kind` discriminant is unknown — skipped.
    UnknownKind { raw: String },
    /// Version major incompatible with this binary — skipped (do not plot).
    UnsupportedVersion { version: String, raw: String },
    /// Line could not be parsed as JSON at all.
    Malformed { raw: String },
}

pub struct Journal {
    path: PathBuf,
}

impl Journal {
    pub fn open(dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let dir = dir.as_ref();
        create_dir_all(dir)?;
        let day = Utc::now().format("%Y%m%d");
        let path = dir.join(format!("journal_{day}.jsonl"));
        OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self, event: &JournalEvent) {
        let line = match serde_json::to_string(&JournalLine {
            version: JOURNAL_VERSION,
            event: event.clone(),
        }) {
            Ok(s) => s,
            Err(e) => {
                error!("journal serialize failed: {e}");
                return;
            }
        };
        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(mut f) => {
                if let Err(e) = writeln!(f, "{line}") {
                    error!("journal write failed: {e}");
                    return;
                }
                if let Err(e) = f.flush() {
                    error!("journal flush failed: {e}");
                }
            }
            Err(e) => error!("journal open for write failed: {e}"),
        }
    }

    /// Read all journal entries with fail-soft version / kind validation.
    pub fn read(&self) -> Vec<JournalEntry> {
        let mut entries = Vec::new();
        let file = match OpenOptions::new().read(true).open(&self.path) {
            Ok(f) => f,
            Err(e) => {
                warn!("journal read open: {e}");
                return entries;
            }
        };
        let reader = BufReader::new(file);
        for (lineno, line) in reader.lines().enumerate() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    warn!("journal line {} read error: {e}", lineno + 1);
                    continue;
                }
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Peek version before full event parse when possible.
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Some(ver) = v.get("version").and_then(|x| x.as_str()) {
                    if !is_version_compatible(ver) {
                        error!(
                            "journal line {}: unsupported version {ver} (need major {}), rejecting",
                            lineno + 1,
                            journal_major()
                        );
                        entries.push(JournalEntry::UnsupportedVersion {
                            version: ver.to_string(),
                            raw: trimmed.to_string(),
                        });
                        continue;
                    }
                }
            }

            let parsed: Result<JournalLineRaw, _> = serde_json::from_str(trimmed);
            match parsed {
                Ok(JournalLineRaw { version, event }) => {
                    if version.is_empty() {
                        warn!(
                            "journal line {}: missing version field, accepting (backward compat)",
                            lineno + 1
                        );
                        entries.push(JournalEntry::Ok {
                            event,
                            had_version: false,
                        });
                    } else if version != JOURNAL_VERSION {
                        // Compatible major, minor drift — accept.
                        warn!(
                            "journal line {}: minor version drift ({} != {}), accepting",
                            lineno + 1,
                            version,
                            JOURNAL_VERSION
                        );
                        entries.push(JournalEntry::Ok {
                            event,
                            had_version: true,
                        });
                    } else {
                        entries.push(JournalEntry::Ok {
                            event,
                            had_version: true,
                        });
                    }
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("kind") || msg.contains("unknown") || msg.contains("match") {
                        error!(
                            "journal line {}: unknown event kind, rejecting: {}",
                            lineno + 1,
                            e
                        );
                        entries.push(JournalEntry::UnknownKind {
                            raw: trimmed.to_string(),
                        });
                    } else {
                        warn!("journal line {}: malformed JSON, skipping: {e}", lineno + 1);
                        entries.push(JournalEntry::Malformed {
                            raw: trimmed.to_string(),
                        });
                    }
                }
            }
        }
        entries
    }

    /// Read events only (drops UnsupportedVersion / UnknownKind / Malformed).
    pub fn read_events(&self) -> Vec<JournalEvent> {
        self.read()
            .into_iter()
            .filter_map(|e| match e {
                JournalEntry::Ok { event, .. } => Some(event),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn temp_journal(entries: &[&str]) -> (Journal, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal_20260101.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        for line in entries {
            writeln!(f, "{line}").unwrap();
        }
        let j = Journal { path };
        (j, dir)
    }

    #[test]
    fn rejects_unknown_kind() {
        let (j, _dir) = temp_journal(&[
            r#"{"version":"1.0","kind":"heartbeat","ts":"2026-01-01T00:00:00Z","equity":1.0,"available":1.0,"daily_pnl":0.0,"n_positions":0,"mode":"dry"}"#,
            r#"{"version":"1.0","kind":"bogus_event","foo":"bar"}"#,
        ]);
        let entries = j.read();
        assert!(matches!(
            entries[0],
            JournalEntry::Ok {
                had_version: true,
                ..
            }
        ));
        assert!(matches!(entries[1], JournalEntry::UnknownKind { .. }));
    }

    #[test]
    fn accepts_versionless_with_warning() {
        let (j, _dir) =
            temp_journal(&[r#"{"kind":"note","ts":"2026-01-01T00:00:00Z","text":"hello"}"#]);
        let entries = j.read();
        assert!(matches!(
            entries[0],
            JournalEntry::Ok {
                had_version: false,
                ..
            }
        ));
    }

    #[test]
    fn rejects_incompatible_major_version() {
        let (j, _dir) = temp_journal(&[
            r#"{"version":"2.0","kind":"note","ts":"2026-01-01T00:00:00Z","text":"future"}"#,
        ]);
        let entries = j.read();
        assert!(matches!(
            entries[0],
            JournalEntry::UnsupportedVersion { ref version, .. } if version == "2.0"
        ));
        assert!(j.read_events().is_empty());
    }

    #[test]
    fn accepts_compatible_minor_drift() {
        let (j, _dir) = temp_journal(&[
            r#"{"version":"1.9","kind":"note","ts":"2026-01-01T00:00:00Z","text":"ok"}"#,
        ]);
        let entries = j.read();
        assert!(matches!(
            entries[0],
            JournalEntry::Ok {
                had_version: true,
                ..
            }
        ));
    }

    #[test]
    fn write_includes_version() {
        let dir = tempfile::tempdir().unwrap();
        let day = Utc::now().format("%Y%m%d");
        let path = dir.path().join(format!("journal_{day}.jsonl"));
        let j = Journal { path: path.clone() };
        j.write(&JournalEvent::Note {
            ts: Utc::now(),
            text: "test".into(),
        });
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"version\":\"1.0\""));
        assert!(content.contains("\"kind\":\"note\""));
    }

    #[test]
    fn version_compat_helpers() {
        assert!(is_version_compatible(""));
        assert!(is_version_compatible("1.0"));
        assert!(is_version_compatible("1.99"));
        assert!(!is_version_compatible("2.0"));
        assert!(!is_version_compatible("not-a-version"));
    }

    #[test]
    fn bracket_event_roundtrip_with_kind_tag() {
        let dir = tempfile::tempdir().unwrap();
        let day = Utc::now().format("%Y%m%d");
        let path = dir.path().join(format!("journal_{day}.jsonl"));
        let j = Journal { path: path.clone() };
        j.write(&JournalEvent::Bracket {
            ts: Utc::now(),
            coin: "ETH".into(),
            action: "trigger_sl".into(),
            level: 98.0,
            mid: 97.9,
        });
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"kind\":\"bracket\""));

        let events = j.read_events();
        match &events[0] {
            JournalEvent::Bracket {
                coin,
                action,
                level,
                mid,
                ..
            } => {
                assert_eq!(coin, "ETH");
                assert_eq!(action, "trigger_sl");
                assert!((level - 98.0).abs() < 1e-9);
                assert!((mid - 97.9).abs() < 1e-9);
            }
            other => panic!("expected bracket event, got {other:?}"),
        }
    }
}
