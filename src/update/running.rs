//! The smon processes already running on this machine, and getting them off the
//! old binary.
//!
//! Replacing the file changes nothing for a process that is already up, so an
//! update ends by telling each one to stand down. A daemon comes back on the
//! new binary, a TUI simply goes.

use std::{
    io::{IsTerminal, Write, stdin, stdout},
    net::SocketAddr,
    path::Path,
    thread::sleep,
    time::{Duration, Instant},
};

use anyhow::{Result, bail};

use crate::{
    client::call,
    control::{Info, Role},
    mcp::PORT_HUNT_RANGE,
};

/// How long a daemon gets to come back before the update says it did not.
const RETURN_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);

pub struct Running {
    pub addr: SocketAddr,
    pub info: Info,
}

impl Running {
    fn describe(&self) -> String {
        let consoles = match self.info.consoles {
            0 => "no consoles".to_string(),
            1 => "1 console".to_string(),
            n => format!("{n} consoles"),
        };
        format!(
            "  {} on {}, v{}, pid {}, {consoles}",
            self.info.role.label(),
            self.addr,
            self.info.version,
            self.info.pid
        )
    }
}

/// Every smon answering on this machine, from `bind` across the range a TUI may
/// have hunted into.
pub fn find(bind: SocketAddr) -> Vec<Running> {
    let end = bind.port().saturating_add(PORT_HUNT_RANGE - 1);
    (bind.port()..=end)
        .map(|port| SocketAddr::new(bind.ip(), port))
        .filter_map(|addr| {
            let body = call(addr, "smon_info", "{}").ok()?;
            let info: Info = serde_json::from_str(&body).ok()?;
            Some(Running { addr, info })
        })
        .collect()
}

/// What has to happen before an update touches anything.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Gate {
    Proceed,
    Ask,
    /// Nobody can answer, and killing a live console session on a guess is not
    /// a default worth having.
    Refuse,
}

fn gate(running: &[Running], yes: bool, interactive: bool) -> Gate {
    if running.is_empty() || yes {
        Gate::Proceed
    } else if interactive {
        Gate::Ask
    } else {
        Gate::Refuse
    }
}

/// Say what will be killed and get a yes, because none of it can be undone.
///
/// # Errors
/// Returns an error if the answer is no, or if nobody can be asked.
pub fn confirm(running: &[Running], yes: bool) -> Result<()> {
    let gate = gate(running, yes, stdin().is_terminal());
    if gate == Gate::Proceed {
        return Ok(());
    }

    println!("this update restarts or kills every smon running on this machine:");
    for process in running {
        println!("{}", process.describe());
    }
    println!("open sessions end and consoles drop while each one comes back.");

    if gate == Gate::Refuse {
        bail!("run it again with --yes to go ahead without being asked");
    }
    print!("continue? [y/N] ");
    stdout().flush()?;
    let mut answer = String::new();
    stdin().read_line(&mut answer)?;
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        bail!("cancelled, nothing was changed");
    }
    Ok(())
}

/// Tell each process to stand down, then wait for the daemons to come back.
pub fn restart(running: &[Running], target: &Path) {
    for process in running {
        if let Some(exe) = process.info.exe.as_deref()
            && !same_file(exe, target)
        {
            println!(
                "note: the {} on {} runs {}, not the binary just updated, so it comes back on {}",
                process.info.role.label(),
                process.addr,
                exe.display(),
                process.info.version
            );
        }
        match call(process.addr, "smon_restart", "{}") {
            Ok(_) => println!(
                "asked the {} on {} to stand down",
                process.info.role.label(),
                process.addr
            ),
            Err(error) => eprintln!(
                "warning: could not stop the {} on {}: {error:#}",
                process.info.role.label(),
                process.addr
            ),
        }
    }

    for process in running {
        if process.info.role == Role::Daemon {
            await_return(process);
        }
    }
}

/// A daemon rebinds the same port, so it is back when that port answers again.
fn await_return(process: &Running) {
    let deadline = Instant::now() + RETURN_TIMEOUT;
    while Instant::now() < deadline {
        sleep(POLL);
        let Ok(body) = call(process.addr, "smon_info", "{}") else {
            continue;
        };
        let Ok(info) = serde_json::from_str::<Info>(&body) else {
            continue;
        };
        // The old process answers until the moment it lets the port go, so
        // something has to prove this is the replacement. A unix exec keeps the
        // pid and a daemon on an untouched binary keeps the version, so the
        // start time is the only thing that always differs.
        if info.started == process.info.started {
            continue;
        }
        println!("the daemon on {} is back on v{}", process.addr, info.version);
        return;
    }
    eprintln!(
        "warning: the daemon on {} did not come back within {}s, start it again by hand",
        process.addr,
        RETURN_TIMEOUT.as_secs()
    );
}

fn same_file(left: &Path, right: &Path) -> bool {
    let resolve = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    resolve(left) == resolve(right)
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, path::PathBuf};

    use super::{Gate, Running, gate, same_file};
    use crate::control::{Info, Role};

    fn running(role: Role, consoles: usize) -> Running {
        Running {
            addr: "127.0.0.1:4123".parse::<SocketAddr>().unwrap(),
            info: Info {
                version: "0.1.1".to_string(),
                role,
                pid: 42,
                started: 1,
                exe: Some(PathBuf::from("/home/x/.cargo/bin/smon")),
                consoles,
            },
        }
    }

    // Nothing running means nothing destructive, so an update must not stop to
    // ask a question that has no consequence.
    #[test]
    fn nothing_running_needs_no_confirmation() {
        assert_eq!(gate(&[], false, true), Gate::Proceed);
        assert_eq!(gate(&[], false, false), Gate::Proceed);
    }

    #[test]
    fn the_yes_flag_skips_the_question() {
        let live = [running(Role::Daemon, 2)];
        assert_eq!(gate(&live, true, true), Gate::Proceed);
        assert_eq!(gate(&live, true, false), Gate::Proceed);
    }

    #[test]
    fn a_live_process_is_asked_about_on_a_terminal() {
        assert_eq!(gate(&[running(Role::Tui, 1)], false, true), Gate::Ask);
    }

    #[test]
    fn a_live_process_without_a_terminal_refuses() {
        assert_eq!(gate(&[running(Role::Tui, 1)], false, false), Gate::Refuse);
    }

    #[test]
    fn console_counts_read_as_plain_english() {
        assert!(running(Role::Daemon, 0).describe().contains("no consoles"));
        assert!(running(Role::Daemon, 1).describe().contains("1 console"));
        assert!(running(Role::Daemon, 4).describe().contains("4 consoles"));
        assert!(running(Role::Tui, 1).describe().contains("tui on 127.0.0.1:4123"));
    }

    #[test]
    fn a_path_matches_itself_even_when_it_does_not_exist() {
        let path = PathBuf::from("/nowhere/smon");
        assert!(same_file(&path, &path));
        assert!(!same_file(&path, &PathBuf::from("/elsewhere/smon")));
    }
}
