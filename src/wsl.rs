use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

pub struct Usbipd {
    exe: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    NotShared,
    Shared,
    Attached,
}

pub struct Device {
    pub busid: String,
    description: String,
    state: State,
}

impl Device {
    pub fn attach_label(&self) -> String {
        match self.state {
            State::Shared => format!("attach  {}  {}  [usbipd]", self.busid, self.description),
            State::NotShared => format!(
                "attach  {}  {}  [usbipd: needs one-time admin bind]",
                self.busid, self.description
            ),
            State::Attached => format!("{}  {}  [usbipd: attached]", self.busid, self.description),
        }
    }
}

pub fn detect() -> Option<Usbipd> {
    if !is_wsl() {
        return None;
    }
    find_usbipd().map(|exe| Usbipd { exe })
}

impl Usbipd {
    pub fn serial_devices(&self) -> Vec<Device> {
        let list = self.run(&["list"]).unwrap_or_default();
        parse_connected(&list)
            .into_iter()
            .filter(|d| d.state != State::Attached && is_serial(&d.description))
            .collect()
    }

    pub fn attach(&self, busid: &str) -> Result<()> {
        let output = Command::new(&self.exe)
            .args(["attach", "--wsl", "--busid", busid])
            .output()
            .context("running usbipd attach")?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lower = stderr.to_ascii_lowercase();
        if lower.contains("not shared") || lower.contains("bind") {
            bail!("{busid} not shared - run once in admin PowerShell: usbipd bind --busid {busid}");
        }
        let detail = stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("usbipd attach failed");
        bail!("{detail}");
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let output = Command::new(&self.exe)
            .args(args)
            .output()
            .context("running usbipd")?;
        if !output.status.success() {
            bail!("usbipd {args:?} failed");
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

fn is_wsl() -> bool {
    if std::env::var_os("WSL_DISTRO_NAME").is_some() {
        return true;
    }
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| {
            let s = s.to_ascii_lowercase();
            s.contains("microsoft") || s.contains("wsl")
        })
        .unwrap_or(false)
}

fn find_usbipd() -> Option<PathBuf> {
    [
        PathBuf::from("usbipd.exe"),
        PathBuf::from("/mnt/c/Program Files/usbipd-win/usbipd.exe"),
    ]
    .into_iter()
    .find(|p| runs(p))
}

fn runs(exe: &Path) -> bool {
    Command::new(exe)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn parse_connected(list: &str) -> Vec<Device> {
    let mut devices = Vec::new();
    let mut in_section = false;
    for line in list.lines() {
        let line = line.trim_end();
        if line.starts_with("Connected:") {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        if line.trim().is_empty() || line.starts_with("Persisted:") {
            break;
        }
        if line.trim_start().starts_with("BUSID") {
            continue;
        }
        if let Some(device) = parse_device_line(line) {
            devices.push(device);
        }
    }
    devices
}

fn parse_device_line(line: &str) -> Option<Device> {
    let (state, rest) = strip_state(line)?;
    let mut tokens = rest.split_whitespace();
    let busid = tokens.next()?.to_string();
    tokens.next()?; // VID:PID column, positional
    let description = tokens.collect::<Vec<_>>().join(" ");
    if description.is_empty() {
        return None;
    }
    Some(Device {
        busid,
        description,
        state,
    })
}

fn strip_state(line: &str) -> Option<(State, &str)> {
    let line = line.trim_end();
    let candidates = [
        ("Not shared", State::NotShared),
        ("Shared (forced)", State::Shared),
        ("Shared", State::Shared),
        ("Attached", State::Attached),
    ];
    for (suffix, state) in candidates {
        if let Some(rest) = line.strip_suffix(suffix) {
            return Some((state, rest));
        }
    }
    None
}

fn is_serial(description: &str) -> bool {
    let d = description.to_ascii_lowercase();
    [
        "serial", "uart", "com port", "ftdi", "cp210", "ch340", "ch341", "prolific", "arduino",
    ]
    .iter()
    .any(|keyword| d.contains(keyword))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r"Connected:
BUSID  VID:PID    DEVICE                                          STATE
1-1    aaaa:0001  Example USB Serial Converter A, Converter B     Shared
2-2    aaaa:0002  Example CP210x USB to UART Bridge               Not shared
3-3    aaaa:0003  Example Generic UART Adapter                    Shared (forced)
4-4    aaaa:0004  Example USB Serial Port                         Attached
5-5    aaaa:0005  Example Integrated Webcam                       Not shared
6-6    aaaa:0006  Example Ethernet Controller                     Attached

Persisted:
GUID                                  DEVICE
00000000  Example persisted device
";

    #[test]
    fn parses_busid_state_and_multiword_description() {
        let devices = parse_connected(SAMPLE);
        assert_eq!(devices.len(), 6);

        let first = &devices[0];
        assert_eq!(first.busid, "1-1");
        assert_eq!(first.state, State::Shared);
        assert_eq!(
            first.description,
            "Example USB Serial Converter A, Converter B"
        );

        assert_eq!(devices[2].state, State::Shared); // "Shared (forced)"
        assert_eq!(devices[3].state, State::Attached);
    }

    #[test]
    fn keeps_attachable_serial_devices_and_drops_attached_or_non_serial() {
        let busids: Vec<String> = parse_connected(SAMPLE)
            .into_iter()
            .filter(|d| d.state != State::Attached && is_serial(&d.description))
            .map(|d| d.busid)
            .collect();
        // 4-4 dropped (attached), 5-5/6-6 dropped (not serial / attached)
        assert_eq!(busids, ["1-1", "2-2", "3-3"]);
    }

    #[test]
    fn is_serial_matches_common_adapters_only() {
        assert!(is_serial("USB Serial Converter A, USB Serial Converter B"));
        assert!(is_serial("Silicon Labs CP210x USB to UART Bridge"));
        assert!(is_serial("USB-SERIAL CH340"));
        assert!(!is_serial("Integrated Camera"));
        assert!(!is_serial("Realtek Gaming USB 2.5GbE Family Controller"));
    }

    #[test]
    fn not_shared_label_points_to_bind() {
        let device = Device {
            busid: "1-1".to_string(),
            description: "USB Serial Converter A".to_string(),
            state: State::NotShared,
        };
        assert!(device.attach_label().contains("admin"));
    }
}
