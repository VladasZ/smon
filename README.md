# smon

Minimalistic TUI serial monitor.

## Install

```
cargo install smon
```

## Usage

```
smon
```

smon enumerates available serial ports, lets you pick one with an fzf-style filter (type to filter, arrow keys to move, Enter to select), then prompts for a baud rate. Once connected, it works like a shell: keystrokes go to the device, incoming bytes scroll in the pane.

If you type a path that doesn't match any detected port, Enter accepts it as a raw device path (useful for virtual PTYs and adapters that don't enumerate).

The last baud rate you pick is remembered (stored in `$XDG_CONFIG_HOME/smon/config.json`, or `~/.config/smon/config.json`) and preselected next time. Pressing `Esc` on the baud picker returns to port selection.

Press `Ctrl+Q` to quit. `Ctrl+]` also quits, but only on keyboard layouts where `]` is a direct key.

## Testing with a fake device

A `Makefile` target spawns a virtual serial pair via [socat](https://www.dest-unreach.org/socat/) so you can try smon without real hardware:

```
make device
```

This symlinks one end at `/tmp/smon-fake` and runs the other end interactively in your terminal. In another terminal, run `cargo run`, type `/tmp/smon-fake` at the port picker. Lines you type in the `make device` terminal show up in smon; keystrokes you send from smon appear there. Ctrl+C stops the device.

Requires socat (`brew install socat` on macOS).

## WSL2

On WSL2 the Windows COM ports are not directly visible. Forward the USB adapter into WSL with [usbipd-win](https://github.com/dorssel/usbipd-win) and smon will list attachable devices in the port picker and attach them for you. See [docs/wsl.md](docs/wsl.md) for the full setup.

## License

Dual-licensed under MIT or Apache-2.0.
