//! The daemon's config file: which consoles to own and how to serve them.

use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{log::DEFAULT_RETENTION_DAYS, ring::DEFAULT_RING_CAP};

pub const DEFAULT_BIND: &str = "127.0.0.1:4123";
const DEFAULT_BAUD: u32 = 115200;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default = "default_bind")]
    pub bind:               String,
    #[serde(default = "default_retention")]
    pub log_retention_days: i64,
    #[serde(default, rename = "console")]
    pub consoles:           Vec<ConsoleSettings>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleSettings {
    pub device:  String,
    /// A name for this console. smon gives it no meaning, it only makes the
    /// console addressable by something shorter than its device path.
    #[serde(default)]
    pub label:   Option<String>,
    #[serde(default = "default_baud")]
    pub baud:    u32,
    #[serde(default = "default_eol")]
    pub eol:     String,
    #[serde(default = "default_ring_kb")]
    pub ring_kb: usize,
    /// Offer this console as a raw byte stream on this loopback port, for a
    /// program that would otherwise need the device node itself.
    #[serde(default)]
    pub bridge_port: Option<u16>,
}

fn default_bind() -> String {
    DEFAULT_BIND.to_string()
}

fn default_retention() -> i64 {
    DEFAULT_RETENTION_DAYS
}

fn default_baud() -> u32 {
    DEFAULT_BAUD
}

fn default_eol() -> String {
    "crlf".to_string()
}

fn default_ring_kb() -> usize {
    DEFAULT_RING_CAP / 1024
}

impl Settings {
    /// Read the config from `path`, or from the first default location that
    /// exists when no path is given.
    ///
    /// # Errors
    /// Returns an error when the named file is missing, when no default
    /// location has one, or when the file does not parse.
    pub fn load(path: Option<&str>) -> Result<Settings> {
        let path = match path {
            Some(path) => PathBuf::from(path),
            None => Self::find()?,
        };
        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let settings: Settings =
            toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))?;
        if settings.consoles.is_empty() {
            bail!("{} lists no consoles, add a [[console]] section", path.display());
        }
        Ok(settings)
    }

    fn find() -> Result<PathBuf> {
        let candidates = search_paths();
        for path in &candidates {
            if path.is_file() {
                return Ok(path.clone());
            }
        }
        let looked: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();
        bail!("no daemon config found, looked in {}", looked.join(", "))
    }
}

fn search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(from_env) = env::var_os("SMON_CONFIG")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        paths.push(from_env);
    }
    if let Some(dir) = crate::config::config_dir() {
        paths.push(dir.join("smon").join("daemon.toml"));
    }
    if cfg!(unix) {
        paths.push(PathBuf::from("/etc/smon/daemon.toml"));
    }
    paths
}

/// The bytes appended to a sent line.
///
/// # Errors
/// Returns an error for a name that is not one of cr, lf, crlf or none.
pub fn eol_bytes(name: &str) -> Result<Vec<u8>> {
    Ok(match name {
        "cr" => b"\r".to_vec(),
        "lf" => b"\n".to_vec(),
        "crlf" => b"\r\n".to_vec(),
        "none" => Vec::new(),
        other => bail!("invalid eol '{other}', expected one of: cr lf crlf none"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_console_needs_only_a_device() {
        let settings: Settings = toml::from_str(
            r#"
[[console]]
device = "/dev/ttyUSB0"
"#,
        )
        .unwrap();

        assert_eq!(settings.bind, DEFAULT_BIND);
        assert_eq!(settings.log_retention_days, DEFAULT_RETENTION_DAYS);
        let console = &settings.consoles[0];
        assert_eq!(console.device, "/dev/ttyUSB0");
        assert_eq!(console.label, None);
        assert_eq!(console.baud, DEFAULT_BAUD);
        assert_eq!(console.eol, "crlf");
        assert_eq!(console.ring_kb, DEFAULT_RING_CAP / 1024);
        assert_eq!(console.bridge_port, None);
    }

    #[test]
    fn every_field_can_be_set() {
        let settings: Settings = toml::from_str(
            r#"
bind = "127.0.0.1:5000"
log_retention_days = 7

[[console]]
device = "/dev/ttyUSB0"
label = "left"
baud = 57600
eol = "cr"
ring_kb = 64
bridge_port = 4500

[[console]]
device = "/dev/ttyUSB2"
label = "right"
"#,
        )
        .unwrap();

        assert_eq!(settings.bind, "127.0.0.1:5000");
        assert_eq!(settings.log_retention_days, 7);
        assert_eq!(settings.consoles.len(), 2);
        assert_eq!(settings.consoles[0].label.as_deref(), Some("left"));
        assert_eq!(settings.consoles[0].baud, 57600);
        assert_eq!(settings.consoles[0].ring_kb, 64);
        assert_eq!(settings.consoles[0].bridge_port, Some(4500));
        assert_eq!(settings.consoles[1].eol, "crlf");
    }

    // A typo in a key would otherwise be accepted in silence and the console
    // would run with a default nobody asked for.
    #[test]
    fn an_unknown_key_is_an_error() {
        let error = toml::from_str::<Settings>(
            r#"
[[console]]
device = "/dev/ttyUSB0"
bauds = 57600
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("bauds"), "{error}");
    }

    #[test]
    fn eol_names_map_to_bytes() {
        assert_eq!(eol_bytes("crlf").unwrap(), b"\r\n");
        assert_eq!(eol_bytes("cr").unwrap(), b"\r");
        assert_eq!(eol_bytes("lf").unwrap(), b"\n");
        assert!(eol_bytes("none").unwrap().is_empty());
        assert!(eol_bytes("tab").is_err());
    }
}
