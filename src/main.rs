use anyhow::{Context, Result};
use ratatui::DefaultTerminal;

mod picker;
mod session;

fn main() -> Result<()> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal) -> Result<()> {
    let ports = serialport::available_ports().context("listing serial ports")?;
    if ports.is_empty() {
        anyhow::bail!("no serial ports found");
    }

    let port_items: Vec<picker::Item> = ports
        .into_iter()
        .map(|p| {
            let label = match &p.port_type {
                serialport::SerialPortType::UsbPort(info) => {
                    let product = info.product.as_deref().unwrap_or("USB");
                    format!("{}  ({product})", p.port_name)
                }
                serialport::SerialPortType::BluetoothPort => {
                    format!("{}  (Bluetooth)", p.port_name)
                }
                serialport::SerialPortType::PciPort => format!("{}  (PCI)", p.port_name),
                serialport::SerialPortType::Unknown => p.port_name.clone(),
            };
            picker::Item {
                value: p.port_name,
                label,
            }
        })
        .collect();

    let port = match picker::pick(terminal, "Select serial port", &port_items, None)? {
        Some(p) => p,
        None => return Ok(()),
    };

    let bauds: [u32; 8] = [9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];
    let baud_items: Vec<picker::Item> = bauds
        .iter()
        .map(|b| picker::Item {
            value: b.to_string(),
            label: b.to_string(),
        })
        .collect();
    let default_baud_index = bauds.iter().position(|b| *b == 115200);

    let baud = match picker::pick(terminal, "Select baud rate", &baud_items, default_baud_index)? {
        Some(b) => b.parse::<u32>().context("parsing baud rate")?,
        None => return Ok(()),
    };

    session::run(terminal, &port, baud)
}
