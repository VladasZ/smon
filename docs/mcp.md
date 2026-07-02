# MCP server

smon serves a small [Model Context Protocol](https://modelcontextprotocol.io)
endpoint so an agent, or any MCP client, can drive the serial console the same
way a person can at the TUI. It exposes generic serial tools only. It has no
knowledge of any board or firmware. Board-specific helpers belong in a separate
MCP server that uses these tools as a client.

## Endpoint

- Transport: Streamable HTTP.
- URL: `http://127.0.0.1:4123/mcp`.
- Always on. It starts with the serial session, no flag needed.
- Localhost only. Pass `--mcp <host:port>` to change the bind, for example
  `smon --mcp 127.0.0.1:5000`.

If the address is already in use, smon writes a `mcp disabled: ...` note to the
session log and the serial monitor keeps running. The MCP server never takes
the monitor down.

The bound endpoint is recorded in the session log file as `mcp serving ...`.
The TUI itself does not show it.

## Connecting

The client and smon must run on the same machine. With Claude Code:

```
claude mcp add --transport http smon http://127.0.0.1:4123/mcp
```

Any MCP client that speaks Streamable HTTP works the same way, by pointing it at
the URL.

## Tools

All tools are generic. Every read returns a `cursor`, an absolute byte offset
into the received stream. Pass it back to read or wait for only what arrived
since.

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

1. `serial_send { text: "startProcess 1" }` -> returns `cursor`.
2. `serial_expect { pattern: "-> ", timeout_ms: 5000, cursor }` -> returns the
   output the board printed in reply, up to the next shell prompt.

`serial_expect` matches on the raw byte stream, so it finds prompts like `-> `
that have no trailing newline. Set `regex: true` to match a pattern instead of a
literal substring. Without a `cursor` it waits for new output only. A single
call waits at most 120 seconds; for longer waits the client calls again.

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
clients can connect at once; each keeps its own read cursor.
