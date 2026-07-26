//! The standalone TUI: pick a port, take it over, and watch it.
//!
//! This is the path with no daemon involved. It builds one console of its own,
//! runs a server for it so an agent can still reach the port, and hands the
//! screen to the session.

use std::{
    net::SocketAddr,
    sync::{Arc, mpsc::channel},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use ratatui::DefaultTerminal;
use serde::Serialize;
use serialport::{SerialPortType, available_ports};
use tokio::sync::oneshot;

use crate::{
    attached::Local,
    client,
    config::Config,
    console::{Console, ConsoleSpec},
    log::{ConsoleLog, DEFAULT_RETENTION_DAYS},
    mcp, picker, probe,
    registry::Registry,
    remote,
    ring::DEFAULT_RING_CAP,
    runner::Runner,
    session, wsl,
};

const ATTACH_SENTINEL: &str = "\0usbipd-attach:";
// A console a daemon already holds, as opposed to a raw device path.
const CONSOLE_SENTINEL: &str = "\0console:";
// What a viewer is shown of the past when it attaches.
const BACKLOG_LINES: usize = 2000;
const DEFAULT_BAUD: u32 = 115200;
const BAUDS: [u32; 8] = [9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];
// The server binds on its own thread, so the session waits this long to hear
// where it landed before giving up on it.
const READY_WAIT: Duration = Duration::from_secs(5);

/// `remote` means `mcp_bind` is the near end of an ssh tunnel. Then the only
/// consoles that exist are the ones on the far side, and this machine's own
/// serial ports have nothing to do with it.
pub fn run(eol: Vec<u8>, mcp_bind: SocketAddr, remote: bool) -> Result<()> {
    // Probed before the screen is taken over, so a failure to reach the far end
    // prints plainly instead of flashing past an alternate screen.
    let daemon = client::find_daemon(mcp_bind);
    if remote && daemon.is_none() {
        bail!("no smon answering through the tunnel at {mcp_bind}");
    }
    let mut terminal = ratatui::init();
    let result = pick_and_attach(&mut terminal, &eol, mcp_bind, daemon.as_ref(), remote);
    ratatui::restore();
    result
}

fn pick_and_attach(
    terminal: &mut DefaultTerminal,
    eol: &[u8],
    mcp_bind: SocketAddr,
    daemon: Option<&client::Daemon>,
    remote: bool,
) -> Result<()> {
    loop {
        let Some(choice) = select_port(terminal, daemon, remote)? else {
            return Ok(());
        };
        if let Some(console) = choice.strip_prefix(CONSOLE_SENTINEL) {
            let Some(daemon) = daemon else {
                bail!("no smon to attach {console} to");
            };
            let mut attached = remote::attach(&daemon.addr.to_string(), console)?;
            return session::run(terminal, &mut attached);
        }
        let device = choice;
        let Some(baud) = pick_baud(terminal, &device)? else {
            continue; // cancelling the baud picker returns to port selection
        };
        let mut config = Config::load();
        config.baud.insert(device.clone(), baud);
        config.save()?;

        // With a daemon running, the port goes to it rather than being opened
        // here. Then it stays up after this viewer quits, and it is logged and
        // reachable like every other console instead of only existing while
        // this window does.
        if let Some(daemon) = daemon {
            let name = adopt(daemon, &device, baud, eol)?;
            let mut attached = remote::attach(&daemon.addr.to_string(), &name)?;
            return session::run(terminal, &mut attached);
        }
        return attach(terminal, &device, baud, eol, mcp_bind);
    }
}

fn attach(
    terminal: &mut DefaultTerminal,
    device: &str,
    baud: u32,
    eol: &[u8],
    mcp_bind: SocketAddr,
) -> Result<()> {
    let log = ConsoleLog::open(device, DEFAULT_RETENTION_DAYS, None)?;
    let (inject_tx, inject_rx) = channel();
    let console = Console::new(
        ConsoleSpec {
            device:   device.to_string(),
            label:    None,
            baud,
            eol:      eol.to_vec(),
            ring_cap: DEFAULT_RING_CAP,
            bridge:   None,
        },
        log,
        inject_tx,
    );
    // The first open must succeed so a bad pick fails fast back at the picker.
    // Failures after that go through the runner's reconnect loop instead of
    // ending the session and losing the scrollback.
    let runner = Runner::start(Arc::clone(&console), inject_rx, true)?;

    let server = match start_server(&console, mcp_bind) {
        Ok(server) => server,
        Err(e) => {
            runner.stop();
            return Err(e);
        }
    };

    let mut local = Local::new(Arc::clone(&console), BACKLOG_LINES);
    let result = session::run(terminal, &mut local);

    if !server.stop() {
        console.note("mcp server had already stopped");
    }
    runner.stop();
    result
}

// Hand a device to the daemon and return the name it answers to.
fn adopt(daemon: &client::Daemon, device: &str, baud: u32, eol: &[u8]) -> Result<String> {
    let request = AdoptRequest {
        device,
        baud,
        eol: eol_name(eol),
    };
    let body = client::call(daemon.addr, "console_adopt", &serde_json::to_string(&request)?)?;
    let adopted: mcp::StatusResult =
        serde_json::from_str(&body).with_context(|| format!("bad console_adopt reply: {body}"))?;
    Ok(adopted.label.unwrap_or(adopted.port))
}

#[derive(Serialize)]
struct AdoptRequest<'a> {
    device: &'a str,
    baud:   u32,
    eol:    &'static str,
}

// The daemon takes the end-of-line by name, the session already holds it as
// bytes, so it is named back here rather than parsed twice.
fn eol_name(eol: &[u8]) -> &'static str {
    match eol {
        b"\r" => "cr",
        b"\n" => "lf",
        b"\r\n" => "crlf",
        _ => "none",
    }
}

struct Server {
    shutdown: oneshot::Sender<()>,
    thread:   thread::JoinHandle<()>,
}

impl Server {
    // Whether the server was still there to be told. The thread is detached
    // rather than joined: a client holding a stream open can keep graceful
    // shutdown from returning, so quitting must not wait on a client.
    fn stop(self) -> bool {
        let told = self.shutdown.send(()).is_ok();
        drop(self.thread);
        told
    }
}

// An agent must always be able to reach a running smon, so a failed bind ends
// the session instead of degrading to a monitor without an endpoint.
fn start_server(console: &Arc<Console>, bind: SocketAddr) -> Result<Server> {
    let (ready_tx, ready_rx) = channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let registry = Registry::new(vec![Arc::clone(console)], 0);
    let thread = mcp::spawn(bind, registry, ready_tx, shutdown_rx);

    match ready_rx.recv_timeout(READY_WAIT) {
        Ok(Ok(addr)) => {
            console.note(&format!("mcp serving http://{addr}/mcp"));
            Ok(Server {
                shutdown: shutdown_tx,
                thread,
            })
        }
        Ok(Err(e)) => Err(anyhow!("mcp bind failed: {e}")),
        Err(e) => Err(anyhow!("mcp server did not start: {e}")),
    }
}

fn pick_baud(terminal: &mut DefaultTerminal, device: &str) -> Result<Option<u32>> {
    let config = Config::load();
    let saved = config.baud.get(device).copied();

    let baud_items: Vec<picker::Item> = BAUDS
        .iter()
        .map(|b| picker::Item {
            value: b.to_string(),
            label: b.to_string(),
            busy:  false,
        })
        .collect();

    let Some(choice) = picker::pick(
        terminal,
        "Select baud rate",
        || baud_items.clone(),
        default_baud_index(&BAUDS, saved),
        false,
    )?
    else {
        return Ok(None);
    };

    Ok(Some(choice.parse::<u32>().context("parsing baud rate")?))
}

fn default_baud_index(bauds: &[u32], saved: Option<u32>) -> Option<usize> {
    let target = saved.unwrap_or(DEFAULT_BAUD);
    bauds
        .iter()
        .position(|b| *b == target)
        .or_else(|| bauds.iter().position(|b| *b == DEFAULT_BAUD))
}

fn select_port(
    terminal: &mut DefaultTerminal,
    daemon: Option<&client::Daemon>,
    remote: bool,
) -> Result<Option<String>> {
    let usbipd = if remote { None } else { wsl::detect() };
    let mut notice: Option<String> = None;

    loop {
        let title = match &notice {
            Some(msg) => format!("Select serial port  --  {msg}"),
            None => "Select serial port".to_string(),
        };

        let make_items = || {
            let mut items = daemon_items(daemon);
            // A device on this machine is not reachable through a tunnel, so
            // with a remote daemon its consoles are the whole list.
            if !remote {
                items.extend(serial_port_items());
            }
            if let Some(u) = &usbipd {
                for device in u.serial_devices() {
                    items.push(picker::Item {
                        value: format!("{ATTACH_SENTINEL}{}", device.busid),
                        label: device.attach_label(),
                        busy:  false,
                    });
                }
            }
            items
        };

        let value = match picker::pick(terminal, &title, make_items, None, true)? {
            Some(v) => v,
            None => return Ok(None),
        };

        let Some(busid) = value.strip_prefix(ATTACH_SENTINEL) else {
            return Ok(Some(value));
        };

        if let Some(u) = &usbipd {
            match u.attach(busid) {
                Ok(()) => {
                    wait_for_new_ports(serial_port_items().len());
                    notice = Some(format!("attached {busid}"));
                }
                Err(e) => notice = Some(e.to_string()),
            }
        }
    }
}

// The consoles a running smon already holds, offered ahead of raw devices.
fn daemon_items(daemon: Option<&client::Daemon>) -> Vec<picker::Item> {
    let Some(daemon) = daemon else {
        return Vec::new();
    };
    daemon
        .consoles
        .iter()
        .map(|console| {
            let state = if console.connected { "connected" } else { "disconnected" };
            picker::Item {
                value: format!("{CONSOLE_SENTINEL}{}", console.name),
                label: format!("{}  (smon on {}, {state})", console.name, daemon.addr),
                busy:  false,
            }
        })
        .collect()
}

fn serial_port_items() -> Vec<picker::Item> {
    let mut items: Vec<picker::Item> = probe::hold(|| {
        available_ports()
            .unwrap_or_default()
            .into_iter()
            .map(|p| {
                let label = match &p.port_type {
                    SerialPortType::UsbPort(info) => {
                        let product = info.product.as_deref().unwrap_or("USB");
                        format!("{}  ({product})", p.port_name)
                    }
                    SerialPortType::BluetoothPort => format!("{}  (Bluetooth)", p.port_name),
                    SerialPortType::PciPort => format!("{}  (PCI)", p.port_name),
                    SerialPortType::Unknown => p.port_name.clone(),
                };
                let busy = probe::is_busy(&p.port_name);
                picker::Item {
                    value: p.port_name,
                    label,
                    busy,
                }
            })
            .collect()
    });
    items.sort_by_key(|a| port_sort_key(&a.value));
    items
}

// Sort COM9 before COM10: split the trailing number off the name so it compares numerically
// instead of lexically.
fn port_sort_key(name: &str) -> (String, Option<u64>) {
    let digits = name.chars().rev().take_while(char::is_ascii_digit).count();
    let (prefix, number) = name.split_at(name.len() - digits);
    (prefix.to_string(), number.parse().ok())
}

fn wait_for_new_ports(baseline: usize) {
    for _ in 0..15 {
        if serial_port_items().len() > baseline {
            return;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_baud_is_preselected() {
        assert_eq!(default_baud_index(&BAUDS, Some(57600)), Some(3));
    }

    #[test]
    fn missing_or_unsaved_baud_falls_back_to_default() {
        assert_eq!(default_baud_index(&BAUDS, None), Some(4)); // 115200
        assert_eq!(default_baud_index(&BAUDS, Some(12345)), Some(4));
    }

    #[test]
    fn ports_sort_numerically_not_lexically() {
        let mut names = vec!["COM10", "COM9", "COM1"];
        names.sort_by_key(|a| port_sort_key(a));
        assert_eq!(names, ["COM1", "COM9", "COM10"]);
    }
}
