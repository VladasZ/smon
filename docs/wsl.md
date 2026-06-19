# Using smon under WSL2

WSL2 is a real VM and does not bridge Windows COM ports. The `/dev/ttyS0..N`
nodes inside WSL are placeholders, not your Windows COM ports - opening them
will not reach a device. To use a serial device, forward its USB adapter into
WSL2 with usbipd-win; it then appears as `/dev/ttyUSB*` (USB-UART) or
`/dev/ttyACM*` (USB CDC).

## Requirements

- Windows 11 (build 22000+) or Windows 10 via the Microsoft Store WSL.
- WSL kernel >= 5.10.60.1 (`uname -r`). Update with `wsl --update`.
- usbipd-win 5.x on Windows: `winget install --interactive --exact dorssel.usbipd-win`
- Build dep for smon's port enumeration: `sudo apt install -y pkg-config libudev-dev`

Common USB-serial drivers (`ftdi_sio`, `cp210x`, `ch341`, `cdc_acm`, `pl2303`)
ship built into recent WSL kernels - no custom kernel needed for typical adapters.

## Commands

All `usbipd` commands run in a **Windows** PowerShell window (it is a Windows
program). Only `bind` needs Administrator.

| command | where | admin |
| --- | --- | --- |
| `usbipd list` | Windows PowerShell | no |
| `usbipd bind --busid <id>` | Windows PowerShell | yes (one-time) |
| `usbipd attach --wsl --busid <id>` | Windows PowerShell | no |
| `usbipd detach --busid <id>` | Windows PowerShell | no |

Typical flow:

```powershell
usbipd list                       # find the BUSID, e.g. 1-1
usbipd bind   --busid 1-1         # one-time, as admin
usbipd attach --wsl --busid 1-1   # forward into WSL2
```

In WSL:

```bash
lsusb                             # device appears (sudo apt install usbutils)
ls -l /dev/ttyUSB* /dev/ttyACM*   # the new node(s)
dmesg | tail                      # which driver bound, which node
```

## Persistence

- `bind` is persistent: run once, survives reboots and replugs.
- `attach` is NOT persistent: re-run after a Windows reboot, a `wsl --shutdown`,
  or unplugging/replugging the device. No admin needed.
- Auto re-attach on replug (holds a terminal open):
  `usbipd attach --wsl --auto-attach --busid 1-1`

## Multi-port devices

usbipd forwards a whole USB device by BUSID, not per port. A single adapter that
exposes several UARTs (e.g. FTDI FT2232/FT4232, CP2105, dual-CDC boards) has one
BUSID; one `attach` brings in all of them as separate nodes
(`/dev/ttyUSB0` + `/dev/ttyUSB1`, etc.).

## Permissions

Opening a port requires membership in the `dialout` group:

```bash
sudo usermod -aG dialout $USER    # then `wsl --shutdown` from Windows and reopen
```

## smon's built-in WSL support

When smon detects WSL it locates `usbipd.exe` (PATH or
`/mnt/c/Program Files/usbipd-win/usbipd.exe`), runs `usbipd list`, and adds
attachable serial devices to the port picker:

- A bound-but-detached device shows as `attach <busid> <name> [usbipd]`.
  Selecting it runs `usbipd attach --wsl`, waits for the new `/dev/ttyUSB*`
  node(s), then returns to the picker so you choose the actual port.
- An unbound device is labeled `[usbipd: needs one-time admin bind]`; selecting
  it surfaces the exact `usbipd bind --busid <id>` command to run as admin.

This automates everything except the one-time admin `bind`.

## COM port note

While a device is attached to WSL it is removed from the Windows USB stack, so
Windows shows no COM port for it (`[System.IO.Ports.SerialPort]::GetPortNames()`
returns empty). The Windows `COMx` identity exists only while the device is on
the Windows side; inside WSL it is `/dev/ttyUSB*`.

## Quitting smon

`Ctrl+Q` quits (works on any keyboard layout). `Ctrl+]` also quits but only on
layouts where `]` is a direct key.
