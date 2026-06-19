use std::{thread, time::Duration};

use anyhow::{Context, Result};
use ratatui::DefaultTerminal;
use serialport::{SerialPortType, available_ports};

mod config;
mod picker;
mod session;
mod wsl;

const ATTACH_SENTINEL: &str = "\0usbipd-attach:";
const DEFAULT_BAUD: u32 = 115200;

fn main() -> Result<()> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal) -> Result<()> {
    loop {
        let Some(port) = select_port(terminal)? else {
            return Ok(());
        };
        let Some(baud) = pick_baud(terminal)? else {
            continue; // cancelling the baud picker returns to port selection
        };
        config::Config { baud: Some(baud) }.save()?;
        return session::run(terminal, &port, baud);
    }
}

fn pick_baud(terminal: &mut DefaultTerminal) -> Result<Option<u32>> {
    let config = config::Config::load();

    let bauds: [u32; 8] = [9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];
    let baud_items: Vec<picker::Item> = bauds
        .iter()
        .map(|b| picker::Item {
            value: b.to_string(),
            label: b.to_string(),
        })
        .collect();

    let Some(choice) = picker::pick(
        terminal,
        "Select baud rate",
        &baud_items,
        default_baud_index(&bauds, config.baud),
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
        let mut items = serial_port_items();
        let port_count = items.len();

        if let Some(u) = &usbipd {
            for device in u.serial_devices() {
                items.push(picker::Item {
                    value: format!("{ATTACH_SENTINEL}{}", device.busid),
                    label: device.attach_label(),
                });
            }
        }

        let title = match &notice {
            Some(msg) => format!("Select serial port  --  {msg}"),
            None => "Select serial port".to_string(),
        };

        let value = match picker::pick(terminal, &title, &items, None, true)? {
            Some(v) => v,
            None => return Ok(None),
        };

        let Some(busid) = value.strip_prefix(ATTACH_SENTINEL) else {
            return Ok(Some(value));
        };

        if let Some(u) = &usbipd {
            match u.attach(busid) {
                Ok(()) => {
                    wait_for_new_ports(port_count);
                    notice = Some(format!("attached {busid}"));
                }
                Err(e) => notice = Some(e.to_string()),
            }
        }
    }
}

fn serial_port_items() -> Vec<picker::Item> {
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
            picker::Item {
                value: p.port_name,
                label,
            }
        })
        .collect()
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
}
