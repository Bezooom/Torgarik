//! Minimal persistent journal for advisory-copilot kill-switch and trade logging.
//!
//! Stores structured entries as JSONL append-only log in `data/journal/`.

use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

/// A journal entry written when the OOS kill-switch state changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OosKillSwitchEntry {
    pub ts: i64,
    pub sharpe: f64,
    pub avg_pnl: f64,
    pub n_folds: usize,
    pub profitable_folds: usize,
    pub state: String,
    pub note: String,
}

/// Append-only journal.
#[derive(Debug, Clone)]
pub struct Journal {
    pub dir: PathBuf,
}

impl Journal {
    pub fn default() -> Self {
        Self {
            dir: PathBuf::from("data/journal"),
        }
    }

    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self) -> PathBuf {
        let mut p = self.dir.clone();
        p.push("journal.jsonl");
        p
    }

    fn ensure_dir(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        Ok(())
    }

    /// Write a kill-switch state-change entry.
    pub fn log_kill_switch(&self, entry: &OosKillSwitchEntry) -> anyhow::Result<()> {
        self.ensure_dir()?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path())?;
        writeln!(f, "{}", serde_json::to_string(entry)?)?;
        Ok(())
    }

    /// Read all kill-switch entries (oldest first).
    pub fn read_entries(&self) -> anyhow::Result<Vec<OosKillSwitchEntry>> {
        let p = self.path();
        if !p.exists() {
            return Ok(vec![]);
        }
        let f = OpenOptions::new().read(true).open(p)?;
        let reader = BufReader::new(f);
        let mut out = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(e) = serde_json::from_str::<OosKillSwitchEntry>(&line) {
                out.push(e);
            }
        }
        Ok(out)
    }
}
