//! Keeping cargo's install list honest after a download.
//!
//! Cargo writes these two files when it installs. A downloaded asset that skips
//! them leaves cargo believing the old version is still installed, and
//! `cargo install-update` would then reinstall over a newer binary.

use std::{
    fs::{read_to_string, write},
    path::Path,
    process::Command,
};

use anyhow::{Context, Result};
use serde_json::{Value, from_str, json, to_string};
use toml::{Table, Value as Toml};

use super::release::PACKAGE;

/// smon is published to crates.io and installed from there, both by
/// `cargo install smon` and by this command's own source fallback, so this is
/// the source cargo records it under.
const SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// The `name version (source)` key cargo tracks an install under.
pub fn install_key(tag: &str) -> String {
    let version = tag.strip_prefix('v').unwrap_or(tag);
    format!("{PACKAGE} {version} ({SOURCE})")
}

/// # Errors
/// Returns an error if either record file cannot be read, parsed or written.
pub fn record(cargo_home: &Path, key: &str, fallback_target: &str) -> Result<()> {
    let info = rustc_info();
    let (rustc, target) = match &info {
        Some((rustc, host)) => (Some(rustc.as_str()), host.as_str()),
        None => (None, fallback_target),
    };
    patch_crates_toml(&cargo_home.join(".crates.toml"), key)?;
    patch_crates2_json(&cargo_home.join(".crates2.json"), key, target, rustc)
}

fn package_of(key: &str) -> &str {
    key.split_whitespace().next().unwrap_or_default()
}

fn patch_crates_toml(path: &Path, key: &str) -> Result<()> {
    let mut doc: Table = if path.exists() {
        read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?
            .parse()
            .with_context(|| format!("could not parse {}", path.display()))?
    } else {
        Table::new()
    };

    let installs = doc
        .entry("v1")
        .or_insert_with(|| Toml::Table(Table::new()))
        .as_table_mut()
        .with_context(|| format!("v1 in {} is not a table", path.display()))?;

    for stale in stale_keys(installs.keys().map(String::as_str)) {
        installs.remove(&stale);
    }
    installs.insert(
        key.to_string(),
        Toml::Array(vec![Toml::String("smon".to_string())]),
    );

    let text = toml::to_string(&doc).context("could not serialize the cargo install list")?;
    write(path, text).with_context(|| format!("could not write {}", path.display()))
}

fn patch_crates2_json(path: &Path, key: &str, target: &str, rustc: Option<&str>) -> Result<()> {
    let mut doc: Value = if path.exists() {
        let text = read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
        from_str(&text).with_context(|| format!("could not parse {}", path.display()))?
    } else {
        json!({ "installs": {}, "v": 1 })
    };

    let root = doc
        .as_object_mut()
        .with_context(|| format!("{} is not a json object", path.display()))?;
    root.insert("v".to_string(), json!(1));
    let installs = root
        .entry("installs")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .with_context(|| format!("installs in {} is not an object", path.display()))?;

    let mut previous_rustc = String::new();
    for stale in stale_keys(installs.keys().map(String::as_str)) {
        if let Some(rustc) = installs
            .remove(&stale)
            .as_ref()
            .and_then(|entry| entry.get("rustc"))
            .and_then(Value::as_str)
        {
            previous_rustc = rustc.to_string();
        }
    }

    installs.insert(
        key.to_string(),
        json!({
            "version_req": Value::Null,
            "bins": ["smon"],
            "features": [],
            "all_features": false,
            "no_default_features": false,
            "profile": "release",
            "target": target,
            "rustc": rustc.unwrap_or(&previous_rustc),
        }),
    );

    let text = to_string(&doc).context("could not serialize the cargo install list")?;
    write(path, text).with_context(|| format!("could not write {}", path.display()))
}

fn stale_keys<'a>(keys: impl Iterator<Item = &'a str>) -> Vec<String> {
    keys.filter(|key| package_of(key) == PACKAGE).map(str::to_string).collect()
}

/// The toolchain that built the asset is unknowable, so the local one is
/// recorded instead. Its host triple is also the honest value for `target`,
/// unlike the `universal-apple-darwin` asset name.
fn rustc_info() -> Option<(String, String)> {
    let output = Command::new("rustc").arg("-vV").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let host = text.lines().find_map(|line| line.strip_prefix("host: "))?.to_string();
    Some((text, host))
}

#[cfg(test)]
mod tests {
    use std::{
        env::temp_dir,
        fs::{create_dir_all, read_to_string, remove_dir_all, write},
        path::PathBuf,
    };

    use serde_json::{Value, from_str};
    use toml::Table;

    use super::{install_key, patch_crates_toml, patch_crates2_json};

    const KEY: &str = "smon 0.1.2 (registry+https://github.com/rust-lang/crates.io-index)";
    const OTHER: &str = "ripgrep 14.1.1 (registry+https://github.com/rust-lang/crates.io-index)";

    fn scratch(name: &str) -> PathBuf {
        let dir = temp_dir().join(format!("smon-records-test-{name}"));
        if dir.exists() {
            remove_dir_all(&dir).unwrap();
        }
        create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_install_key_matches_what_a_crates_io_install_writes() {
        assert_eq!(install_key("v0.1.2"), KEY);
        assert_eq!(install_key("0.1.2"), KEY);
    }

    #[test]
    fn the_old_entry_is_replaced_and_other_crates_are_left_alone() {
        let dir = scratch("toml");
        let path = dir.join(".crates.toml");
        write(
            &path,
            format!(
                "[v1]\n\"{OTHER}\" = [\"rg\"]\n\"smon 0.1.1 (registry+https://github.com/rust-lang/crates.io-index)\" = [\"smon\"]\n"
            ),
        )
        .unwrap();

        patch_crates_toml(&path, KEY).unwrap();

        let doc: Table = read_to_string(&path).unwrap().parse().unwrap();
        let installs = doc["v1"].as_table().unwrap();
        assert_eq!(installs.len(), 2);
        assert_eq!(installs[KEY].as_array().unwrap().len(), 1);
        assert!(installs.contains_key(OTHER));
        remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_crates_toml_is_created() {
        let dir = scratch("fresh-toml");
        let path = dir.join(".crates.toml");

        patch_crates_toml(&path, KEY).unwrap();

        let doc: Table = read_to_string(&path).unwrap().parse().unwrap();
        assert!(doc["v1"].as_table().unwrap().contains_key(KEY));
        remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_json_entry_is_replaced_and_the_new_toolchain_recorded() {
        let dir = scratch("json");
        let path = dir.join(".crates2.json");
        write(
            &path,
            format!(
                r#"{{"installs":{{"{OTHER}":{{"bins":["rg"]}},"smon 0.1.1 (registry+https://github.com/rust-lang/crates.io-index)":{{"bins":["smon"],"rustc":"rustc 1.90.0","target":"aarch64-apple-darwin"}}}},"v":1}}"#
            ),
        )
        .unwrap();

        patch_crates2_json(&path, KEY, "aarch64-apple-darwin", Some("rustc 1.95.0")).unwrap();

        let doc: Value = from_str(&read_to_string(&path).unwrap()).unwrap();
        let installs = doc["installs"].as_object().unwrap();
        assert_eq!(installs.len(), 2);
        assert_eq!(installs[KEY]["rustc"], "rustc 1.95.0");
        assert_eq!(installs[KEY]["target"], "aarch64-apple-darwin");
        assert_eq!(installs[KEY]["bins"], from_str::<Value>(r#"["smon"]"#).unwrap());
        assert_eq!(installs[KEY]["profile"], "release");
        assert!(installs.contains_key(OTHER));
        remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unreadable_toolchain_keeps_the_previous_one() {
        let dir = scratch("no-rustc");
        let path = dir.join(".crates2.json");
        write(
            &path,
            r#"{"installs":{"smon 0.1.1 (registry+x)":{"bins":["smon"],"rustc":"rustc 1.90.0"}},"v":1}"#,
        )
        .unwrap();

        patch_crates2_json(&path, KEY, "x86_64-unknown-linux-musl", None).unwrap();

        let doc: Value = from_str(&read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["installs"][KEY]["rustc"], "rustc 1.90.0");
        remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_crates2_json_is_created() {
        let dir = scratch("fresh-json");
        let path = dir.join(".crates2.json");

        patch_crates2_json(&path, KEY, "x86_64-unknown-linux-musl", Some("rustc 1.95.0")).unwrap();

        let doc: Value = from_str(&read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["v"], 1);
        assert_eq!(doc["installs"][KEY]["target"], "x86_64-unknown-linux-musl");
        remove_dir_all(&dir).unwrap();
    }
}
