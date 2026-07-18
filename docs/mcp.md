# MCP server

smon serves a small [Model Context Protocol](https://modelcontextprotocol.io)
endpoint so an agent, or any MCP client, can drive the serial console the same
way a person can at the TUI.

## Endpoint

- Transport: Streamable HTTP.
- URL: `http://127.0.0.1:4123/mcp`.
- Always on. It starts with the serial session, no flag needed.
- Localhost only. Pass `--mcp <host:port>` to change the bind, for example
  `smon --mcp 127.0.0.1:5000`.

When the requested port is taken, by another smon instance for example, smon
hunts upward through the next ports, 16 in total, and serves on the first free
one. Every running instance therefore has its own endpoint. A client finds the
instance for a given serial port by probing the ports in order and checking
`serial_status`, which reports the COM port each instance monitors. When the
whole range is taken smon exits with `mcp bind failed`, a running smon without
a reachable MCP endpoint is not allowed.

The bound endpoint is recorded in the session log file as `mcp serving ...`.
The TUI itself does not show it.

## One-shot calls from a shell

The smon binary is its own client. `smon list` probes the port range and
prints every running instance, MCP port, serial port, baud, connection state.
`smon call <port> <tool> [json-args]` calls one tool and prints the JSON
result, string results print raw.

```
smon list
smon call 4123 serial_status
smon call 4124 serial_send '{"text":"reboot","newline":true}'
smon call 4124 serial_expect '{"pattern":"ready> ","timeout_ms":10000,"cursor":0}'
```

Under the hood this uses a plain HTTP side door next to /mcp on the same bind:
`POST /call/<tool>` with the JSON arguments as the body, no MCP session
needed, so curl works too: `curl -d '{}' http://127.0.0.1:4123/call/serial_status`.

Note for agents: do not write temp wrapper scripts around the endpoint, use
`smon call`. If something is still too clunky, propose an smon improvement so
the friction gets fixed in the tool once rather than re-scripted every session.

## Connecting

The client and smon must run on the same machine. With Claude Code:

```
claude mcp add --transport http smon http://127.0.0.1:4123/mcp
```

Any MCP client that speaks Streamable HTTP works the same way, by pointing it at
the URL.

## Tools

Every read returns a `cursor`, an absolute byte offset into the received stream.
Pass it back to read or wait for only what arrived since.

| Tool | Parameters | Returns |
| --- | --- | --- |
| `serial_send` | `text`, `newline` (default true) | `cursor` before the write |
| `serial_send_ctrl` | `ctrl` (one char, e.g. `c` for Ctrl+C) | `cursor` before the write |
| `serial_read` | `cursor` (optional) | `data`, next `cursor` |
| `serial_expect` | `pattern`, `timeout_ms`, `regex` (default false), `cursor` (optional) | `matched`, `data`, `cursor`, `timed_out` |
| `serial_snapshot` | `lines` (default 40) | the last N lines as text |
| `serial_status` | none | `port`, `baud`, `connected`, `cursor` |

### The cursor and expect model

`serial_send` returns the cursor from just before it wrote, so the usual pattern
is send then expect:

1. `serial_send { text: "version" }` -> returns `cursor`.
2. `serial_expect { pattern: "ready> ", timeout_ms: 5000, cursor }` -> returns the
   output the device printed in reply, up to the next shell prompt.

`serial_expect` matches on the raw byte stream, so it finds prompts like `ready> `
that have no trailing newline. Set `regex: true` to match a pattern instead of a
literal substring. Without a `cursor` it waits for new output only. A single
call waits at most 120 seconds. For longer waits the client calls again.

`serial_read` with no `cursor` returns the whole retained buffer. The buffer
keeps at least the most recent 512 KB, so a cursor pointing at bytes older than
that simply starts at the oldest retained byte.

### Disconnects

If the device disappears mid-session, `serial_send` and `serial_send_ctrl`
return an error and `serial_status` reports `connected: false`. smon retries
the port every second and reconnects on its own, after which `connected` flips
back to true. Cursors stay valid across a disconnect.

## Sharing with the TUI

The person at the TUI and MCP clients share one console. Input an MCP client
sends is written to the port from the same place as keystrokes, echoed in the
scrollback as a magenta `>>` line, and recorded in the log as `[mcp]`. Several
clients can connect at once, each keeping its own read cursor.
