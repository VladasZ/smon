//! The always-on service. It owns every console in its config, keeps them
//! logged, and serves them to clients.
//!
//! Nothing here opens a device directly. Each console gets a runner that holds
//! its port and reconnects on its own, so an adapter that is unplugged at boot
//! costs that one console and not the service.

use std::{
    net::SocketAddr,
    sync::{Arc, mpsc::channel},
};

use anyhow::{Context, Result};

use crate::{
    console::{Console, ConsoleSpec},
    control::{Control, Role},
    log::ConsoleLog,
    mcp,
    registry::Registry,
    runner::Runner,
    settings::{Settings, eol_bytes},
};

/// Load the config, take over every console it names, and serve until killed.
///
/// # Errors
/// Returns an error if the config is missing or invalid, if a console cannot be
/// set up, or if the bind address is taken.
pub fn run(config: Option<&str>, bind: Option<SocketAddr>) -> Result<()> {
    let settings = Settings::load(config)?;
    let bind = match bind {
        Some(bind) => bind,
        None => settings
            .bind
            .parse()
            .with_context(|| format!("invalid bind address '{}'", settings.bind))?,
    };

    let mut consoles = Vec::new();
    // Runners are kept for the life of the process. Dropping one would only
    // detach its thread, but holding them keeps the ownership honest.
    let mut runners = Vec::new();
    for wanted in &settings.consoles {
        let eol = eol_bytes(&wanted.eol).with_context(|| format!("console {}", wanted.device))?;
        // The log is named after the label when there is one, so a by-id device
        // path does not become a 60 character file name.
        let name = wanted.label.as_deref().unwrap_or(&wanted.device);
        let log = ConsoleLog::open(name, settings.log_retention_days, None)?;
        let path = log.info().path.display().to_string();

        let (inject_tx, inject_rx) = channel();
        let console = Console::new(
            ConsoleSpec {
                device: wanted.device.clone(),
                label: wanted.label.clone(),
                baud: wanted.baud,
                eol,
                ring_cap: wanted.ring_kb.saturating_mul(1024),
                bridge: wanted.bridge_port,
            },
            log,
            inject_tx,
        );
        runners.push(Runner::start(Arc::clone(&console), inject_rx, false)?);
        println!(
            "smon: console {} on {} at {}, logging to {}",
            console.name(),
            wanted.device,
            wanted.baud,
            path
        );
        consoles.push(console);
    }

    // Captured before serving, so an update that renames the binary away cannot
    // leave this process unable to name the file it has to come back on.
    let control = Arc::new(Control::new(Role::Daemon));
    mcp::run(
        bind,
        Registry::new(consoles, settings.log_retention_days),
        Arc::clone(&control),
    )?;

    if !control.stopping() {
        return Ok(());
    }
    // The server has already let the port go, and every serial fd is O_CLOEXEC,
    // so the replacement finds nothing of this process still holding a device.
    println!("smon: restarting on the updated binary");
    control.relaunch()
}
