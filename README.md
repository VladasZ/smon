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

smon lists the available serial ports and lets you pick one with an fzf-style
filter. Type to filter, arrow keys to move, Enter to select. The list refreshes
about once a second. A port already opened by another program or another smon is
shown dimmed and marked `busy`, and cannot be picked. Busy detection currently
works on Windows. After you pick a port smon asks for a baud rate and connects.

If you type a path that does not match any detected port, Enter accepts it as a
raw device path. This is useful for virtual PTYs and adapters that do not
enumerate.

Run `smon --help` for the full flag list.

### Sending and receiving

The bottom box is an input line. Type a command and press Enter to send it
followed by the line ending. Device output scrolls in the pane above, with a
scrollbar when there is more than one screen. Your sent lines are echoed in the
pane in cyan with a `>` prefix.

- Enter sends the current line plus the line ending.
- Up and Down recall previously sent commands.
- Tab, or Right at the end of the line, accepts the ghost autocomplete suggestion.
- Ctrl key combos such as Ctrl+C pass straight through to the device.
- Ctrl+Q quits.

If the device disappears mid-session, for example the board reboots or the
adapter is replugged, smon keeps the scrollback, marks the session as
disconnected in the title, and reconnects on its own as soon as the port is
back.

The line ending is chosen once at launch with `--eol` and defaults to `crlf`:

```
smon --eol crlf   # \r\n, the default
smon --eol cr     # \r
smon --eol lf     # \n
smon --eol none   # send nothing extra
```

### Autocomplete

Sent commands are saved and offered back as you type. The best fuzzy match from
your history is shown dimmed at the right of the input box with a Tab hint. Press
Tab, or Right at the end of the line, to accept it. The history is global, stored
in the config file, de-duplicated, and capped at the 200 most recent commands.

### Remembered baud

The baud rate is remembered per port and preselected the next time you open that
port. It is stored in the config file. Pressing Esc on the baud picker returns to
port selection.

### Session logs

Every session is written to its own log file in real time. Each entry has a
timestamp and a direction marker: incoming bytes are tagged `RX`, lines you send
are tagged `TX`. Control bytes are escaped so the file stays readable plain text.
The file name holds the port and the start time, for example
`smon-COM3-20260629-143205.log`.

Logs are stored in:

- `$XDG_STATE_HOME/smon/logs/` on Linux and macOS, or `~/.local/state/smon/logs/`
- `%LOCALAPPDATA%\smon\logs\` on Windows

While a session is running its log file is held open, so on Windows the size and
last write time shown by a directory listing are stale. Windows does not flush
them to the directory entry until the file is closed. `dir`, `ls` and
`Get-ChildItem` can report the active log as 0 bytes or with an old timestamp
even while bytes are being written to it. Do not decide a log is empty or
unchanged from its listed size or time. Read the file contents.

### Config file

The baud per port and the command history live in `config.json`, found in:

- `$XDG_CONFIG_HOME/smon/` or `~/.config/smon/` on Linux and macOS
- `%APPDATA%\smon\` on Windows

## MCP server

smon also serves a small [Model Context Protocol](https://modelcontextprotocol.io)
endpoint, so an agent or any MCP client can drive the serial console the same way
you can at the TUI. It exposes generic serial tools only, such as `serial_send`,
`serial_read`, and `serial_expect`, with no knowledge of any device.

It is always on and listens on `http://127.0.0.1:4123/mcp` over Streamable HTTP,
localhost only. Change the bind with `--mcp`:

```
smon --mcp 127.0.0.1:5000
```

See [docs/mcp.md](docs/mcp.md) for the tool list and how to connect a client.

## Testing with a fake device

A `Makefile` target spawns a virtual serial pair via [socat](https://www.dest-unreach.org/socat/) so you can try smon without real hardware:

```
make device
```

This symlinks one end at `/tmp/smon-fake` and runs the other end interactively in your terminal. In another terminal, run `cargo run`, type `/tmp/smon-fake` at the port picker. Lines you type in the `make device` terminal show up in smon, and lines you send from smon appear there. Ctrl+C stops the device.

Requires socat (`brew install socat` on macOS).

## WSL2

On WSL2 the Windows COM ports are not directly visible. Forward the USB adapter into WSL with [usbipd-win](https://github.com/dorssel/usbipd-win) and smon will list attachable devices in the port picker and attach them for you. See [docs/wsl.md](docs/wsl.md) for the full setup.

## License

Dual-licensed under MIT or Apache-2.0.
