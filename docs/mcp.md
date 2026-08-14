# MCP server

smon serves a small [Model Context Protocol](https://modelcontextprotocol.io)
endpoint so an agent, or any MCP client, can drive the serial console the same
way a person can at the TUI.

## Endpoint

- Transport: Streamable HTTP.
- URL: `http://127.0.0.1:4123/mcp`.
- Always on. The daemon serves it, and so does a standalone TUI, no flag needed.
- Loopback only. Pass `--mcp <host:port>` to change the bind, for example
  `smon --mcp 127.0.0.1:5000`. To reach another machine use `--host`, which
  tunnels over ssh rather than putting a console on the network.

The daemon binds exactly what it was told and fails loudly when that port is
taken, because a service that quietly moves is worse than one that stops. A
standalone TUI does hunt upward through the next ports, 16 in total, so several
of them on one machine each get an endpoint. When the whole range is taken it
exits with `mcp bind failed`, a running smon without a reachable endpoint is not
allowed.

The bound endpoint is recorded in the session log file as `mcp serving ...`.
The TUI itself does not show it.

## One-shot calls from a shell

The smon binary is its own client. `smon list` prints every console a running
smon owns. `smon call <tool> [json-args]` calls one tool and prints the JSON
result, string results print raw. Both take `--host <ssh target>` to reach an
smon on another machine, and open the ssh tunnel themselves.

```
smon list
smon call console_list
smon call serial_status '{"console":"left"}'
smon call serial_send '{"console":"left","text":"reboot","newline":true}'
smon call serial_expect '{"console":"left","pattern":"ready> ","timeout_ms":10000,"cursor":0}'
smon list --host pi                      # the same, against another machine over ssh
```

Under the hood this uses a plain HTTP side door next to /mcp on the same bind:
`POST /call/<tool>` with the JSON arguments as the body, no MCP session
needed, so curl works too: `curl -d '{}' http://127.0.0.1:4123/call/serial_status`.

Two calls live on that side door only and are absent from the MCP tool list,
`smon_info` and `smon_restart`. They are what `smon update` uses to find running
smon processes and stand them down. Standing a process down drops every console
and every viewer on it, and an agent cannot tell that someone is watching one in
another terminal, so this is not a decision to hand to a model.

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

## Naming a console

One smon can hold several consoles, so every tool takes an optional `console`,
either a label or a device path. With one console open it can be left out. With
several it is required: picking a board for the caller is the one thing this
must never guess at, so a call without a name is an error listing what there is.

`console_list` takes no arguments and answers the same either way, so it is what
a client uses to find out what exists.

## Tools

Every read returns a `cursor`, an absolute byte offset into the received stream.
Pass it back to read or wait for only what arrived since.

| Tool | Parameters | Returns |
| --- | --- | --- |
| `console_list` | none | one entry per console, as `serial_status` |
| `serial_send` | `console`, `text`, `newline` (default true) | `cursor` before the write |
| `serial_send_ctrl` | `console`, `ctrl` (one char, e.g. `c` for Ctrl+C) | `cursor` before the write |
| `serial_read` | `console`, `cursor` (optional) | `data`, next `cursor` |
| `serial_expect` | `console`, `pattern`, `timeout_ms`, `regex` (default false), `cursor` (optional) | `matched`, `data`, `cursor`, `timed_out` |
| `serial_snapshot` | `console`, `lines` (default 40) | the last N lines as text |
| `serial_status` | `console` | `port`, `label`, `baud`, `connected`, `cursor`, `log`, `released`, `bridge` |
| `log_roll` | `console`, `tag` (optional) | `path`, `started` of the new log segment |
| `log_info` | `console` | `path`, `started` of the current segment |
| `log_search` | `console`, `pattern`, `regex` (default false), `max_results` (default 100), `context` (default 0), `days` (optional) | `matches` newest first, `truncated`, `files_searched` |
| `console_release` | `console` | status, once the device is really closed |
| `console_hold` | `console` | status, after taking the device back |
| `console_adopt` | `device`, `label`, `baud`, `eol`, `ring_kb` | status of the new console |

### Taking over a device later

`console_adopt` hands a device that is not open yet to a running smon, which is
what the TUI does when you pick a port the daemon does not already hold. The
device has to exist and be free, and a failure to open it is reported rather
than leaving behind a console that never connects. A console adopted this way
cannot have a `bridge_port`, since the bridge listeners start with the server,
so put a console that needs one in the config file.

### Giving a run its own log

`log_roll` closes the current log segment and starts a new one, returning where
it went. A run that calls it first can read its own output back from exactly
that path instead of searching a day-sized file for its own lines. `tag` is
added to the file name, a ticket id for example, and smon gives it no meaning.

### Searching the logs

`log_search` searches a console's log files on disk, so it reaches everything
still retained under `log_retention_days`, not just the in-memory buffer that
`serial_read` sees. The pattern is a substring by default, a regular expression
with `regex: true`, and it must not be empty.

Matches come newest first. Each carries the `file` it was found in, its 1-based
`line` number there, and the matching log line as `text`, timestamp and
direction marker included. `max_results` caps the reply keeping the newest
matches, and `truncated` reports whether more existed beyond the cap. `context`
returns that many surrounding lines with each match, as `before` and `after`.
`days` narrows the search to segments started within that many days, where 1
means today, and leaving it out searches everything retained.

The search is line-based over the log records. One record holds one received
chunk, and a device line can split across two records, so a substring that
straddles a chunk boundary can be missed. When a search for a line that should
exist comes back empty, search for a shorter piece of it.

### Handing the device over

`console_release` makes smon let go of the serial device so another program can
open it, and it does not return until the port is actually closed, so the caller
can open it the moment the call comes back. The console keeps its buffer, its log
and its viewers throughout. `console_hold` takes the device back.

Usually nothing needs this, because a console with a `bridge_port` is already
reachable as a raw byte stream, see the daemon section of the README.

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

A console with a stable `/dev/serial/by-id/...` path also survives a replug that
hands the adapter a different `ttyUSB` number, because the path it reopens is the
one that does not move.

## Sharing a console

Everyone watching a console sees the same thing. Input from an MCP client is
written to the port from the same place as a keystroke, shown in every attached
TUI as a magenta `>>` line, and recorded in the log as `[mcp]`. Input through a
raw bridge is recorded as `[bridge]`. Several clients can connect at once, each
keeping its own read cursor.
