//! Putting a downloaded binary in place without ever leaving the machine
//! without a working one.

use std::{
    env::var_os,
    fs::{remove_file, rename},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use dirs::home_dir;
use which::which;

pub const BINARY: &str = if cfg!(windows) { "smon.exe" } else { "smon" };

/// Where cargo keeps its installed binaries and its install records. smon is
/// installed with `cargo install smon`, so a missing cargo directory means the
/// binary came from somewhere this cannot safely replace.
///
/// # Errors
/// Returns an error if the home directory or the cargo bin directory is
/// missing.
pub fn cargo_home() -> Result<PathBuf> {
    let home = match var_os("CARGO_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home_dir().context("could not find the home directory")?.join(".cargo"),
    };
    let bin = home.join("bin");
    if !bin.is_dir() {
        bail!(
            "{} does not exist, smon update replaces a cargo installed binary",
            bin.display()
        );
    }
    Ok(home)
}

/// An older copy earlier on PATH keeps winning after a successful update, so
/// say which file is really being run.
pub fn warn_if_shadowed(target: &Path) {
    let Ok(found) = which("smon") else {
        return;
    };
    if same_file(&found, target) {
        return;
    }
    eprintln!("warning: the smon on your PATH is {}", found.display());
    eprintln!(
        "warning: it shadows the updated {}, remove it or fix the PATH order",
        target.display()
    );
}

fn same_file(left: &Path, right: &Path) -> bool {
    let resolve = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    resolve(left) == resolve(right)
}

/// Prove the downloaded binary runs and is the version it claims, while the
/// installed one is still untouched.
///
/// # Errors
/// Returns an error if the binary will not run or reports another version.
pub fn verify(binary: &Path, tag: &str) -> Result<()> {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .with_context(|| format!("the downloaded binary at {} did not run", binary.display()))?;
    if !output.status.success() {
        bail!("the downloaded binary exited with {} on --version", output.status);
    }
    let reported = String::from_utf8_lossy(&output.stdout);
    let expected = format!("smon {}", tag.strip_prefix('v').unwrap_or(tag));
    if !reported.trim_start().starts_with(&expected) {
        bail!(
            "the downloaded binary reports `{}`, expected {expected}",
            reported.trim()
        );
    }
    Ok(())
}

/// Put `staged` in place of `target`, keeping the old binary until the new one
/// is in place so a failure can put it back.
///
/// # Errors
/// Returns an error if the old binary cannot be moved aside or the new one
/// cannot be moved in.
pub fn swap(staged: &Path, target: &Path) -> Result<()> {
    let moved = if target.exists() {
        Some(move_aside(target)?)
    } else {
        None
    };

    if let Err(error) = rename(staged, target) {
        let failure = format!(
            "could not move {} to {}: {error}",
            staged.display(),
            target.display()
        );
        if let Some(old) = &moved
            && let Err(restore_error) = restore(old, target)
        {
            bail!("{failure}; restoring the previous smon binary also failed: {restore_error:#}");
        }
        bail!("{failure}");
    }

    if let Some(old) = &moved
        && let Err(error) = remove_file(old)
    {
        eprintln!(
            "smon update: the previous binary remains at {} and will be removed next time: {error}",
            old.display()
        );
    }
    Ok(())
}

fn old_path(target: &Path, index: usize) -> PathBuf {
    let suffix = if index == 0 {
        ".old".to_string()
    } else {
        format!(".old{index}")
    };
    PathBuf::from(format!("{}{suffix}", target.display()))
}

/// Windows cannot delete a running binary, so its leftovers go on the next run
/// instead.
pub fn cleanup_stale_binaries(target: &Path) {
    for index in 0..100 {
        let old = old_path(target, index);
        if !old.exists() {
            continue;
        }
        if let Err(error) = remove_file(&old) {
            eprintln!(
                "smon update: could not remove stale binary {}: {error}",
                old.display()
            );
        }
    }
}

/// # Errors
/// Returns an error if no backup name is free or the rename fails.
pub fn move_aside(target: &Path) -> Result<PathBuf> {
    let mut last_error = None;
    for index in 0..100 {
        let old = old_path(target, index);
        if old.exists()
            && let Err(error) = remove_file(&old)
        {
            last_error = Some(error);
            continue;
        }
        match rename(target, &old) {
            Ok(()) => return Ok(old),
            Err(error) => last_error = Some(error),
        }
    }
    let detail = last_error.map_or_else(|| "no free backup name".to_string(), |e| e.to_string());
    bail!("could not move {} aside: {detail}", target.display())
}

/// # Errors
/// Returns an error if the failed update cannot be removed or the previous
/// binary cannot be moved back.
pub fn restore(old: &Path, target: &Path) -> Result<()> {
    if target.exists() {
        remove_file(target)
            .with_context(|| format!("could not remove failed update at {}", target.display()))?;
    }
    rename(old, target).with_context(|| {
        format!(
            "could not restore previous binary from {} to {}",
            old.display(),
            target.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use std::{
        env::temp_dir,
        fs::{create_dir_all, read_to_string, remove_dir_all, write},
        path::{Path, PathBuf},
    };

    use super::{move_aside, old_path, restore, swap};

    fn scratch(name: &str) -> PathBuf {
        let dir = temp_dir().join(format!("smon-install-test-{name}"));
        if dir.exists() {
            remove_dir_all(&dir).unwrap();
        }
        create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn old_paths_are_stable() {
        let target = Path::new("/tmp/smon");
        assert_eq!(old_path(target, 0), Path::new("/tmp/smon.old"));
        assert_eq!(old_path(target, 2), Path::new("/tmp/smon.old2"));
    }

    #[test]
    fn move_and_restore_preserve_the_previous_binary() {
        let dir = scratch("restore");
        let target = dir.join("smon");
        write(&target, "working").unwrap();

        let old = move_aside(&target).unwrap();
        assert!(!target.exists());
        assert_eq!(read_to_string(&old).unwrap(), "working");

        write(&target, "failed update").unwrap();
        restore(&old, &target).unwrap();
        assert_eq!(read_to_string(&target).unwrap(), "working");
        assert!(!old.exists());
        remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_swap_replaces_the_binary_and_leaves_no_backup() {
        let dir = scratch("swap");
        let target = dir.join("smon");
        let staged = dir.join("smon.new");
        write(&target, "old").unwrap();
        write(&staged, "new").unwrap();

        swap(&staged, &target).unwrap();

        assert_eq!(read_to_string(&target).unwrap(), "new");
        assert!(!staged.exists());
        assert!(!old_path(&target, 0).exists());
        remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_swap_works_when_nothing_is_installed_yet() {
        let dir = scratch("fresh");
        let target = dir.join("smon");
        let staged = dir.join("smon.new");
        write(&staged, "new").unwrap();

        swap(&staged, &target).unwrap();

        assert_eq!(read_to_string(&target).unwrap(), "new");
        remove_dir_all(&dir).unwrap();
    }
}
