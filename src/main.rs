use std::{env, net::SocketAddr, thread, time::Duration};

use anyhow::{Context, Result, bail};
use ratatui::DefaultTerminal;
use serialport::{SerialPortType, available_ports};

mod config;
mod log;
mod mcp;
mod picker;
mod probe;
mod session;
mod wsl;

const ATTACH_SENTINEL: &str = "\0usbipd-attach:";
const DEFAULT_BAUD: u32 = 115200;
// The MCP server always runs. --mcp only moves it off the default local bind.
const DEFAULT_MCP: &str = "127.0.0.1:4123";

const USAGE: &str = r"smon - minimalistic TUI serial monitor

Usage: smon [options]

Options:
  --eol <cr|lf|crlf|none>  line ending appended to sent lines, default crlf
  --mcp <host:port>        MCP server bind address, default 127.0.0.1:4123
  -h, --help               show this help
  -V, --version            show the version";

enum Cli {
    Run { eol: Vec<u8>, mcp: SocketAddr },
    Help,
    Version,
}

fn main() -> Result<()> {
    let (eol, mcp_bind) = match parse_cli(env::args().skip(1))? {
        Cli::Help => {
            println!("{USAGE}");
            return Ok(());
        }
        Cli::Version => {
            println!("smon {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Cli::Run { eol, mcp } => (eol, mcp),
    };
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &eol, mcp_bind);
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal, eol: &[u8], mcp_bind: SocketAddr) -> Result<()> {
    loop {
        let Some(port) = select_port(terminal)? else {
            return Ok(());
        };
        let Some(baud) = pick_baud(terminal, &port)? else {
            continue; // cancelling the baud picker returns to port selection
        };
        let mut config = config::Config::load();
        config.baud.insert(port.clone(), baud);
        config.save()?;
        return session::run(terminal, &port, baud, eol, mcp_bind);
    }
}

fn parse_cli(mut args: impl Iterator<Item = String>) -> Result<Cli> {
    let mut eol: Option<String> = None;
    let mut mcp: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Cli::Help),
            "-V" | "--version" => return Ok(Cli::Version),
            "--eol" => eol = Some(args.next().context("--eol needs a value")?),
            "--mcp" => mcp = Some(args.next().context("--mcp needs a value")?),
            other => {
                if let Some(v) = other.strip_prefix("--eol=") {
                    eol = Some(v.to_string());
                } else if let Some(v) = other.strip_prefix("--mcp=") {
                    mcp = Some(v.to_string());
                } else {
                    bail!("unknown argument '{other}', try --help");
                }
            }
        }
    }

    let eol = eol_bytes(eol.as_deref().unwrap_or("crlf"))?;
    let text = mcp.as_deref().unwrap_or(DEFAULT_MCP);
    let mcp = text.parse::<SocketAddr>().with_context(|| {
        format!("invalid --mcp address '{text}', expected host:port like {DEFAULT_MCP}")
    })?;
    Ok(Cli::Run { eol, mcp })
}

fn eol_bytes(name: &str) -> Result<Vec<u8>> {
    Ok(match name {
        "cr" => b"\r".to_vec(),
        "lf" => b"\n".to_vec(),
        "crlf" => b"\r\n".to_vec(),
        "none" => Vec::new(),
        other => bail!("invalid --eol '{other}', expected one of: cr lf crlf none"),
    })
}

fn pick_baud(terminal: &mut DefaultTerminal, port: &str) -> Result<Option<u32>> {
    let config = config::Config::load();
    let saved = config.baud.get(port).copied();

    let bauds: [u32; 8] = [9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];
    let baud_items: Vec<picker::Item> = bauds
        .iter()
        .map(|b| picker::Item {
            value: b.to_string(),
            label: b.to_string(),
            busy: false,
        })
        .collect();

    let Some(choice) = picker::pick(
        terminal,
        "Select baud rate",
        || baud_items.clone(),
        default_baud_index(&bauds, saved),
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

fn select_port(terminal: &mut DefaultTerminal) -> Result<Option<String>> {
    let usbipd = wsl::detect();
    let mut notice: Option<String> = None;

    loop {
        let title = match &notice {
            Some(msg) => format!("Select serial port  --  {msg}"),
            None => "Select serial port".to_string(),
        };

        let make_items = || {
            let mut items = serial_port_items();
            if let Some(u) = &usbipd {
                for device in u.serial_devices() {
                    items.push(picker::Item {
                        value: format!("{ATTACH_SENTINEL}{}", device.busid),
                        label: device.attach_label(),
                        busy: false,
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

fn serial_port_items() -> Vec<picker::Item> {
    let lock = probe::Lock::acquire();
    let mut items: Vec<picker::Item> = available_ports()
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
        .collect();
    drop(lock);
    items.sort_by_key(|a| port_sort_key(&a.value));
    items
}

// Sort COM9 before COM10: split the trailing number off the name so it compares numerically
// instead of lexically.
fn port_sort_key(name: &str) -> (String, Option<u64>) {
    let digits = name
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .count();
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

    const BAUDS: [u32; 8] = [9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];

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

    fn parse(args: &[&str]) -> Result<Cli> {
        parse_cli(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn unknown_or_incomplete_arguments_are_rejected() {
        assert!(parse(&["--elo=cr"]).is_err());
        assert!(parse(&["extra"]).is_err());
        assert!(parse(&["--eol"]).is_err());
        assert!(parse(&["--eol", "tab"]).is_err());
        assert!(parse(&["--mcp", "not-an-addr"]).is_err());
    }

    #[test]
    fn eol_and_mcp_parse_in_both_forms() {
        let Ok(Cli::Run { eol, mcp }) = parse(&["--eol=cr", "--mcp", "127.0.0.1:9000"]) else {
            panic!("expected Cli::Run");
        };
        assert_eq!(eol, b"\r");
        assert_eq!(mcp.port(), 9000);
    }

    #[test]
    fn defaults_apply_with_no_arguments() {
        let Ok(Cli::Run { eol, mcp }) = parse(&[]) else {
            panic!("expected Cli::Run");
        };
        assert_eq!(eol, b"\r\n");
        assert_eq!(mcp.to_string(), DEFAULT_MCP);
    }

    #[test]
    fn help_and_version_short_circuit() {
        assert!(matches!(parse(&["--help"]), Ok(Cli::Help)));
        assert!(matches!(parse(&["-V", "--bogus"]), Ok(Cli::Version)));
    }
}
