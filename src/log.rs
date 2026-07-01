use std::{
    env,
    fs::{self, File},
    io::Write,
    path::PathBuf,
};

use anyhow::{Context, Result};
use chrono::Local;

pub struct SessionLog {
    file: File,
}

impl SessionLog {
    pub fn create(port_name: &str) -> Result<SessionLog> {
        let dir = log_dir().context("resolving log directory")?;
        fs::create_dir_all(&dir).with_context(|| format!("creating log dir {}", dir.display()))?;

        let stamp = Local::now().format("%Y%m%d-%H%M%S");
        let name = format!("smon-{}-{stamp}.log", sanitize(port_name));
        let path = dir.join(name);
        let file =
            File::create(&path).with_context(|| format!("creating log file {}", path.display()))?;

        Ok(SessionLog { file })
    }

    pub fn rx(&mut self, bytes: &[u8]) -> Result<()> {
        let text = String::from_utf8_lossy(bytes);
        self.entry("RX", &escape(&text))
    }

    pub fn tx_line(&mut self, bytes: &[u8]) -> Result<()> {
        let text = String::from_utf8_lossy(bytes);
        self.entry("TX", &escape(&text))
    }

    pub fn tx_key(&mut self, label: &str) -> Result<()> {
        self.entry("TX", &format!("<{label}>"))
    }

    fn entry(&mut self, dir: &str, text: &str) -> Result<()> {
        let ts = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        writeln!(self.file, "{ts}  {dir}  {text}")?;
        self.file.flush()?;
        Ok(())
    }
}

fn log_dir() -> Option<PathBuf> {
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
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())?;
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
            c if c.is_control() && (c as u32) < 0x100 => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
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
}
