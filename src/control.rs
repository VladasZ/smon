//! What a running smon says about itself, and how it is told to stand down.
//!
//! `smon update` replaces the binary on disk, but a process that is already
//! running keeps the old code until it goes. This is the channel it goes
//! through, so an update leaves nothing behind on the previous version.

use std::{
    env::{args_os, current_exe},
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, id},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

/// What a process is, which decides what standing down means. A daemon owns
/// consoles named in a config file, so it comes back on the new binary. A TUI
/// has nothing to come back to and just goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Daemon,
    Tui,
}

impl Role {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Daemon => "daemon",
            Self::Tui => "tui",
        }
    }
}

/// What `smon update` needs to know about a process before it replaces the
/// binary underneath it.
#[derive(Debug, Serialize, Deserialize)]
pub struct Info {
    pub version:  String,
    pub role:     Role,
    pub pid:      u32,
    /// When this process started, in nanoseconds since the unix epoch. It is
    /// what tells a replacement apart from the process it replaced. The pid
    /// cannot: a unix exec keeps it, and so does the version when a daemon
    /// comes back on a binary the update did not touch.
    pub started:  u128,
    /// The binary this process runs. Captured at startup because reading it
    /// later would name a deleted inode, an update having renamed the file away.
    pub exe:      Option<PathBuf>,
    pub consoles: usize,
}

pub struct Control {
    role:    Role,
    exe:     Option<PathBuf>,
    args:    Vec<OsString>,
    started: u128,
    asked:   AtomicBool,
    /// Ends the server so its port is free before the process is replaced.
    release: Mutex<Option<oneshot::Sender<()>>>,
}

impl Control {
    pub fn new(role: Role) -> Self {
        Self {
            role,
            exe: current_exe().ok(),
            args: args_os().skip(1).collect(),
            started: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos()),
            asked: AtomicBool::new(false),
            release: Mutex::new(None),
        }
    }

    /// Hand over the signal that ends the server. Firing it before the process
    /// is replaced is what frees the port for the new one to bind.
    pub fn arm(&self, release: oneshot::Sender<()>) {
        *self.release.lock().unwrap_or_else(|e| e.into_inner()) = Some(release);
    }

    /// Stop the server so its port is free. Returns whether it was still there
    /// to be told. This is the only way the server is ever stopped, whether a
    /// session ended or an update asked for it.
    pub fn release(&self) -> bool {
        let Some(release) = self.release.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return false;
        };
        release.send(()).is_ok()
    }

    pub fn info(&self, consoles: usize) -> Info {
        Info {
            version: env!("CARGO_PKG_VERSION").to_string(),
            role: self.role,
            pid: id(),
            started: self.started,
            exe: self.exe.clone(),
            consoles,
        }
    }

    /// An update asked this process to stand down. The flag outlives the
    /// server, because a TUI session loop reads it to know why it is ending.
    pub fn request_stop(&self) {
        self.asked.store(true, Ordering::SeqCst);
        self.release();
    }

    pub fn stopping(&self) -> bool {
        self.asked.load(Ordering::SeqCst)
    }

    pub fn exe(&self) -> Option<&Path> {
        self.exe.as_deref()
    }

    /// Start over on whatever binary now sits at this process's own path,
    /// carrying the same arguments. Never returns on success.
    ///
    /// # Errors
    /// Returns an error if the path was never known or the new process cannot
    /// be started, leaving this one running the old code.
    pub fn relaunch(&self) -> Result<()> {
        let exe = self
            .exe
            .as_deref()
            .context("this process does not know its own binary, restart it by hand")?;
        replace(exe, &self.args)
    }
}

/// Unix keeps the pid across an exec, so a daemon under systemd or launchd
/// restarts without the supervisor seeing anything happen at all.
#[cfg(unix)]
fn replace(exe: &Path, args: &[OsString]) -> Result<()> {
    use std::os::unix::process::CommandExt;

    Err(Command::new(exe).args(args).exec())
        .with_context(|| format!("could not restart {}", exe.display()))
}

/// Windows has no exec, so the replacement is a fresh process and this one
/// ends once it is up.
#[cfg(windows)]
fn replace(exe: &Path, args: &[OsString]) -> Result<()> {
    use std::process::exit;

    Command::new(exe)
        .args(args)
        .spawn()
        .with_context(|| format!("could not restart {}", exe.display()))?;
    exit(0);
}

#[cfg(test)]
mod tests {
    use tokio::sync::oneshot;

    use super::{Control, Role};

    #[test]
    fn a_fresh_control_is_not_stopping() {
        let control = Control::new(Role::Daemon);
        assert!(!control.stopping());
        assert_eq!(control.info(3).consoles, 3);
        assert_eq!(control.info(0).role, Role::Daemon);
    }

    #[test]
    fn a_stop_request_fires_the_release_and_sets_the_flag() {
        let control = Control::new(Role::Daemon);
        let (tx, mut rx) = oneshot::channel();
        control.arm(tx);

        control.request_stop();

        assert!(control.stopping());
        assert!(rx.try_recv().is_ok());
    }

    // A unix exec keeps the pid, so a restarted daemon reports the very same
    // one. The start time is what tells the replacement apart from the process
    // it replaced, and an update waits on exactly that.
    #[test]
    fn a_restart_is_recognisable_even_though_the_pid_is_not() {
        let before = Control::new(Role::Daemon).info(0);
        let after = Control::new(Role::Daemon).info(0);
        assert_eq!(before.pid, after.pid);
        assert_ne!(before.started, after.started);
    }

    #[test]
    fn a_stop_request_works_with_nothing_armed() {
        let control = Control::new(Role::Tui);
        control.request_stop();
        assert!(control.stopping());
        assert_eq!(control.info(0).role, Role::Tui);
    }

    #[test]
    fn a_second_stop_request_is_harmless() {
        let control = Control::new(Role::Daemon);
        let (tx, _rx) = oneshot::channel();
        control.arm(tx);
        control.request_stop();
        control.request_stop();
        assert!(control.stopping());
    }

    // A session ending is not an update, so quitting must not leave the process
    // claiming it was stopped by one.
    #[test]
    fn releasing_the_server_does_not_look_like_an_update() {
        let control = Control::new(Role::Tui);
        let (tx, _rx) = oneshot::channel();
        control.arm(tx);

        assert!(control.release());
        assert!(!control.stopping());
        assert!(!control.release());
    }
}
