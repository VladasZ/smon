# smon

Minimalistic TUI serial monitor.

## Install

```
cargo install smon
```

## Usage

Run with no arguments. smon enumerates available serial ports, lets you pick one with an fzf-style filter (type to filter, arrow keys to move, Enter to select), then prompts for a baud rate. Once connected, it works like a shell: keystrokes go to the device, incoming bytes scroll in the pane.

```
smon
```

Press `Ctrl+]` to quit.

## License

Dual-licensed under MIT or Apache-2.0.
