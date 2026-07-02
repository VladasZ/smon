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
        // Merge into the file's current contents rather than overwriting, so two
        // running instances do not wipe each other's saved bauds and history.
        let merged = self.merged_into(Config::load());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, serde_json::to_string_pretty(&merged)?)?;
        Ok(())
    }

    // Our entries win for ports we changed and rank freshest in history. Without
    // timestamps this is the best recency guess either instance can make.
    fn merged_into(&self, mut disk: Config) -> Config {
        for (port, baud) in &self.baud {
            disk.baud.insert(port.clone(), *baud);
        }
        for cmd in &self.history {
            disk.record_command(cmd);
        }
        disk
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_merges_disk_entries_instead_of_overwriting() {
        let mut disk = Config::default();
        disk.baud.insert("COM3".to_string(), 9600);
        disk.history = vec!["from_other".to_string()];

        let mut ours = Config::default();
        ours.baud.insert("COM4".to_string(), 115200);
        ours.record_command("ours");

        let merged = ours.merged_into(disk);
        assert_eq!(merged.baud["COM3"], 9600);
        assert_eq!(merged.baud["COM4"], 115200);
        assert_eq!(merged.history, ["from_other", "ours"]);
    }

    #[test]
    fn record_command_dedupes_and_moves_to_end() {
        let mut config = Config::default();
        config.record_command("a");
        config.record_command("b");
        config.record_command("a");
        assert_eq!(config.history, ["b", "a"]);
    }
}
