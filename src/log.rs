//! The per-console log file, its naming, its rotation and its retention.
//!
//! A log is a sequence of segments. A segment is one file, named after the
//! console and the moment it was opened. The daemon rolls to a new segment when
//! the date changes, and a client can roll one at any time, which is how a test
//! run gets its own file instead of sharing a day-sized one.

use std::{
    env,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate};

use crate::console::Origin;

pub const DEFAULT_RETENTION_DAYS: i64 = 30;

/// Where a segment lives and when it started, which is what a client needs to
/// read back exactly the output of its own run.
#[derive(Clone)]
pub struct LogInfo {
    pub path:    PathBuf,
    pub started: DateTime<Local>,
}

pub struct ConsoleLog {
    dir:       PathBuf,
    stem:      String,
    retention: i64,
    file:      File,
    info:      LogInfo,
    day:       NaiveDate,
}

impl ConsoleLog {
    /// `name` is the console label when it has one, else its device path. It
    /// only shapes the file name, so a readable label beats a long by-id path.
    pub fn open(name: &str, retention_days: i64, tag: Option<&str>) -> Result<ConsoleLog> {
        let dir = log_dir().context("resolving log directory")?;
        Self::open_in(dir, name, retention_days, tag)
    }

    pub fn open_in(dir: PathBuf, name: &str, retention_days: i64, tag: Option<&str>) -> Result<ConsoleLog> {
        fs::create_dir_all(&dir).with_context(|| format!("creating log dir {}", dir.display()))?;
        let stem = sanitize(name);
        let started = Local::now();
        let path = segment_path(&dir, &stem, started, tag);
        let log = ConsoleLog {
            file: open_segment(&path)?,
            info: LogInfo { path, started },
            day: started.date_naive(),
            dir,
            stem,
            retention: retention_days,
        };
        log.prune()?;
        Ok(log)
    }

    /// Close the current segment and start a new one. Returns where the new
    /// segment lives, so the caller never has to guess which file is its own.
    pub fn roll(&mut self, tag: Option<&str>) -> Result<LogInfo> {
        let started = Local::now();
        let path = segment_path(&self.dir, &self.stem, started, tag);
        self.file = open_segment(&path)?;
        self.info = LogInfo { path, started };
        self.day = started.date_naive();
        self.prune()?;
        Ok(self.info.clone())
    }

    pub fn info(&self) -> LogInfo {
        self.info.clone()
    }

    pub fn rx(&mut self, bytes: &[u8]) -> Result<()> {
        let text = String::from_utf8_lossy(bytes);
        self.entry("RX", &escape(&text))
    }

    pub fn tx(&mut self, origin: Origin, bytes: &[u8], echo: &str) -> Result<()> {
        let text = String::from_utf8_lossy(bytes);
        match origin {
            Origin::Typed => self.entry("TX", &escape(&text)),
            // A control byte has no readable form, so the label is logged instead.
            Origin::Key => self.entry("TX", &format!("<{echo}>")),
            // Marked so the log tells injected input apart from what a person
            // typed, while still recording it as sent to the device.
            Origin::Agent => self.entry("TX", &format!("[mcp] {}", escape(&text))),
            Origin::Bridge => self.entry("TX", &format!("[bridge] {}", escape(&text))),
        }
    }

    /// An smon side note, such as a disconnect or a reconnect.
    pub fn system(&mut self, text: &str) -> Result<()> {
        self.entry("SYS", text)
    }

    fn entry(&mut self, dir: &str, text: &str) -> Result<()> {
        let now = Local::now();
        if now.date_naive() != self.day {
            self.roll(None)?;
        }
        let ts = now.format("%Y-%m-%d %H:%M:%S%.3f");
        writeln!(self.file, "{ts}  {dir}  {text}")?;
        self.file.flush()?;
        Ok(())
    }

    // Drop segments of this console older than the retention window. Only files
    // carrying this console's own prefix are considered, so a shared log
    // directory never loses another console's history.
    fn prune(&self) -> Result<()> {
        if self.retention <= 0 {
            return Ok(());
        }
        let cutoff = Local::now().date_naive() - chrono::Duration::days(self.retention);
        let prefix = format!("smon-{}-", self.stem);
        let entries = fs::read_dir(&self.dir).with_context(|| format!("reading {}", self.dir.display()))?;
        for entry in entries {
            let path = entry?.path();
            let Some(date) = segment_date(&path, &prefix) else {
                continue;
            };
            if date < cutoff {
                fs::remove_file(&path).with_context(|| format!("removing old log {}", path.display()))?;
            }
        }
        Ok(())
    }
}

// The date a segment was opened, read back out of its file name. The stem can
// itself contain dashes, as a by-id device path does, so the date is taken at a
// fixed offset after the known prefix rather than by splitting on dashes.
fn segment_date(path: &Path, prefix: &str) -> Option<NaiveDate> {
    if path.extension().is_none_or(|e| e != "log") {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix(prefix)?;
    let date = rest.get(..8)?;
    NaiveDate::parse_from_str(date, "%Y%m%d").ok()
}

fn segment_path(dir: &Path, stem: &str, started: DateTime<Local>, tag: Option<&str>) -> PathBuf {
    let stamp = started.format("%Y%m%d-%H%M%S");
    let name = match tag.map(sanitize).filter(|t| t != "port") {
        Some(tag) => format!("smon-{stem}-{stamp}-{tag}.log"),
        None => format!("smon-{stem}-{stamp}.log"),
    };
    dir.join(name)
}

// Append rather than truncate. Two segments can only collide when a roll lands
// in the same second as another under the same tag, and losing the earlier one
// would be worse than mixing them.
fn open_segment(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("creating log file {}", path.display()))
}

pub fn log_dir() -> Option<PathBuf> {
    if let Some(state) = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        return Some(state.join("smon").join("logs"));
    }
    if cfg!(windows)
        && let Some(local) = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
    {
        return Some(local.join("smon").join("logs"));
    }
    let home = env::var_os("HOME").map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())?;
    Some(home.join(".local").join("state").join("smon").join("logs"))
}

fn sanitize(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = mapped.trim_matches('_');
    if trimmed.is_empty() {
        "port".to_string()
    } else {
        trimmed.to_string()
    }
}

fn escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() && (c as u32) < 0x100 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs::read_to_string;

    use super::*;

    #[test]
    fn escapes_control_bytes_keeps_printable() {
        assert_eq!(escape("OK temp=42C\r\n"), "OK temp=42C\\r\\n");
        assert_eq!(escape("a\x1bb"), "a\\x1bb");
        assert_eq!(escape("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn sanitize_port_names() {
        assert_eq!(sanitize("COM3"), "COM3");
        assert_eq!(sanitize("/dev/ttyUSB0"), "dev_ttyUSB0");
        assert_eq!(sanitize("///"), "port");
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("smon-log-test-{name}"));
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        dir
    }

    #[test]
    fn roll_starts_a_new_file_and_reports_it() {
        let dir = temp_dir("roll");
        let mut log = ConsoleLog::open_in(dir.clone(), "COM3", 30, None).unwrap();
        let first = log.info();
        log.rx(b"before\n").unwrap();

        let second = log.roll(Some("PCSWORK-17780")).unwrap();
        log.rx(b"after\n").unwrap();

        assert_ne!(first.path, second.path);
        assert!(read_to_string(&first.path).unwrap().contains("before"));
        let after = read_to_string(&second.path).unwrap();
        assert!(after.contains("after"));
        assert!(!after.contains("before"));
        assert!(
            second
                .path
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with("-PCSWORK-17780.log"),
            "tag missing from {}",
            second.path.display()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    // Retention must never reach another console's files, which is the whole
    // reason the prefix is matched rather than just the smon- part.
    #[test]
    fn prune_drops_old_segments_of_this_console_only() {
        let dir = temp_dir("prune");
        fs::create_dir_all(&dir).unwrap();
        let old = dir.join("smon-COM3-20200101-101010.log");
        let other = dir.join("smon-COM4-20200101-101010.log");
        fs::write(&old, "old").unwrap();
        fs::write(&other, "other").unwrap();

        ConsoleLog::open_in(dir.clone(), "COM3", 30, None).unwrap();

        assert!(!old.exists(), "old segment of this console should be gone");
        assert!(other.exists(), "another console's segment must survive");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prune_keeps_segments_inside_the_window() {
        let dir = temp_dir("keep");
        fs::create_dir_all(&dir).unwrap();
        let stamp = Local::now().format("%Y%m%d");
        let fresh = dir.join(format!("smon-COM3-{stamp}-010101.log"));
        fs::write(&fresh, "fresh").unwrap();

        ConsoleLog::open_in(dir.clone(), "COM3", 30, None).unwrap();

        assert!(fresh.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    // A by-id device path keeps its dashes through sanitize, so a date read by
    // splitting on dashes would land on the wrong field and never prune.
    #[test]
    fn segment_date_survives_a_stem_full_of_dashes() {
        let prefix = "smon-dev_serial_by-id_usb-FTDI-if00-port0-";
        let path = PathBuf::from(format!("/logs/{prefix}20260726-143205.log"));
        assert_eq!(
            segment_date(&path, prefix),
            Some(NaiveDate::from_ymd_opt(2026, 7, 26).unwrap())
        );
    }

    #[test]
    fn segment_date_ignores_foreign_files() {
        let prefix = "smon-COM3-";
        assert_eq!(segment_date(Path::new("/logs/notes.txt"), prefix), None);
        assert_eq!(
            segment_date(Path::new("/logs/smon-COM4-20260726-1.log"), prefix),
            None
        );
    }

    #[test]
    fn tx_records_keep_their_direction_markers() {
        let dir = temp_dir("tx");
        let mut log = ConsoleLog::open_in(dir.clone(), "COM3", 30, None).unwrap();
        log.tx(Origin::Typed, b"version\r\n", "version").unwrap();
        log.tx(Origin::Key, &[3], "Ctrl+C").unwrap();
        log.tx(Origin::Agent, b"reboot\r\n", "reboot").unwrap();
        let text = read_to_string(log.info().path).unwrap();

        assert!(text.contains("TX  version\\r\\n"), "{text}");
        assert!(text.contains("TX  <Ctrl+C>"), "{text}");
        assert!(text.contains("TX  [mcp] reboot\\r\\n"), "{text}");
        fs::remove_dir_all(&dir).unwrap();
    }
}
