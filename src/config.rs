use std::{collections::HashMap, env, fs, path::PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

const MAX_HISTORY: usize = 200;

#[derive(Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub baud: HashMap<String, u32>,
    #[serde(default)]
    pub history: Vec<String>,
}

impl Config {
    // Most-recent last. Re-sending a command moves it to the end so it ranks as the freshest.
    pub fn record_command(&mut self, cmd: &str) {
        if cmd.is_empty() {
            return;
        }
        self.history.retain(|c| c != cmd);
        self.history.push(cmd.to_string());
        if self.history.len() > MAX_HISTORY {
            let drop = self.history.len() - MAX_HISTORY;
            self.history.drain(..drop);
        }
    }

    pub fn load() -> Config {
        let Some(path) = config_path() else {
            return Config::default();
        };
        let Ok(text) = fs::read_to_string(path) else {
            return Config::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = config_path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

fn config_path() -> Option<PathBuf> {
    Some(config_dir()?.join("smon").join("config.json"))
}

fn config_dir() -> Option<PathBuf> {
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        return Some(xdg);
    }
    if cfg!(windows)
        && let Some(appdata) = env::var_os("APPDATA")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
    {
        return Some(appdata);
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .map(|home| home.join(".config"))
}
