//! Remote stop flag: the smallest possible kill switch.
//!
//! A file, not an RPC. Any process that can write a file can stop new entries
//! (Telegram bot, cron, a human in a terminal), and the shell needs no listener,
//! no port and no extra credentials. Presence = stopped, absence = running.
//!
//! The shell re-reads it every cycle, so the worst case delay is one loop
//! interval. Closing positions is never blocked by this flag: a stop is a
//! risk-reducing action and must stay available.

use std::path::{Path, PathBuf};

/// Default flag location, relative to the working directory.
pub const STOP_FLAG: &str = "runtime/stop.flag";

/// Default path.
pub fn default_path() -> PathBuf {
    PathBuf::from(STOP_FLAG)
}

/// True when the flag exists. An unreadable directory counts as "not set":
/// a missing runtime dir must not stop the shell from starting.
pub fn is_set(path: &Path) -> bool {
    path.exists()
}

/// Reason text stored in the flag, trimmed to one line.
pub fn read_reason(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let line = raw.lines().next().unwrap_or("").trim().to_string();
    if line.is_empty() {
        None
    } else {
        Some(line)
    }
}

/// Create the flag with a one-line reason.
pub fn set(path: &Path, reason: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let clean: String = reason
        .replace('\n', " ")
        .chars()
        .take(200)
        .collect();
    std::fs::write(path, format!("{clean}\n"))
}

/// Remove the flag. `Ok(false)` when it was not set.
pub fn clear(path: &Path) -> std::io::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
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
            .join(format!("stopflag-{tag}-{nanos}-{}", std::process::id()))
            .join("stop.flag")
    }

    #[test]
    fn set_read_clear_roundtrip() {
        let path = tmp("roundtrip");
        assert!(!is_set(&path));
        assert_eq!(read_reason(&path), None);
        set(&path, "manual: too much chop").expect("set");
        assert!(is_set(&path));
        assert_eq!(read_reason(&path).as_deref(), Some("manual: too much chop"));
        assert!(clear(&path).expect("clear"));
        assert!(!is_set(&path));
    }

    #[test]
    fn clearing_an_unset_flag_is_not_an_error() {
        let path = tmp("unset");
        assert!(!clear(&path).expect("idempotent"));
    }

    #[test]
    fn reason_is_flattened_to_one_line_and_truncated() {
        let path = tmp("reason");
        let long = format!("line one\nline two {}", "x".repeat(300));
        set(&path, &long).expect("set");
        let reason = read_reason(&path).expect("reason");
        assert!(!reason.contains('\n'));
        assert!(reason.len() <= 200, "len={}", reason.len());
    }

    #[test]
    fn empty_file_has_no_reason_but_still_stops() {
        let path = tmp("empty");
        set(&path, "").expect("set");
        assert!(is_set(&path), "presence alone stops the shell");
        assert_eq!(read_reason(&path), None);
    }
}
