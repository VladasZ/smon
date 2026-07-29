//! One-shot CLI client for the /call side door of a running smon.
//! Plain blocking HTTP over a socket, no client dependencies, so `smon call`
//! and `smon list` work anywhere the binary does.

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};

use crate::mcp::{PORT_HUNT_RANGE, StatusResult};

// serial_expect can legitimately wait 120 s, everything else answers instantly.
const READ_TIMEOUT: Duration = Duration::from_secs(125);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

/// Call one tool on the smon serving `addr` and return the response body, a
/// JSON document.
///
/// # Errors
/// Returns an error if nothing is listening, the reply is not HTTP, or the tool
/// reports a failure.
pub fn call(addr: SocketAddr, tool: &str, args: &str) -> Result<String> {
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .with_context(|| format!("no smon on {addr}, is the daemon running"))?;
    request(stream, addr, tool, args)
}

/// Print one line per console the smon at `addr` owns.
///
/// # Errors
/// Returns an error if the endpoint cannot be reached or its reply is not the
/// expected shape.
pub fn list(addr: SocketAddr) -> Result<()> {
    let body = call(addr, "console_list", "{}")?;
    let consoles: Vec<StatusResult> =
        serde_json::from_str(&body).with_context(|| format!("bad console_list reply: {body}"))?;
    if consoles.is_empty() {
        println!("smon on {addr} owns no consoles");
        return Ok(());
    }
    for console in consoles {
        let label = console.label.as_deref().unwrap_or("-");
        let state = if console.connected { "connected" } else { "disconnected" };
        println!("{label}  {}  {}  {state}", console.port, console.baud);
    }
    Ok(())
}

/// A console some smon on this machine already holds.
pub struct Console {
    pub name:      String,
    pub connected: bool,
}

/// A reachable smon and what it owns.
pub struct Daemon {
    pub addr:     SocketAddr,
    pub consoles: Vec<Console>,
}

/// Find an smon on this machine, starting at `bind` and walking the hunt range.
///
/// Used by the TUI before it opens anything: a console another instance already
/// holds cannot be opened again, so it is attached to instead.
pub fn find_daemon(bind: SocketAddr) -> Option<Daemon> {
    let end = bind.port().saturating_add(PORT_HUNT_RANGE - 1);
    for port in bind.port()..=end {
        let addr = SocketAddr::new(bind.ip(), port);
        let Ok(body) = call(addr, "console_list", "{}") else {
            continue;
        };
        let Ok(listed) = serde_json::from_str::<Vec<StatusResult>>(&body) else {
            continue;
        };
        let consoles: Vec<Console> = listed
            .into_iter()
            .map(|c| Console {
                // The label is how the console is addressed when it has one,
                // and the device path is the name when it does not.
                name:      c.label.unwrap_or(c.port),
                connected: c.connected,
            })
            .collect();
        if !consoles.is_empty() {
            return Some(Daemon { addr, consoles });
        }
    }
    None
}

fn request(mut stream: TcpStream, addr: SocketAddr, tool: &str, args: &str) -> Result<String> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    let request = format!(
        "POST /call/{tool} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{args}",
        args.len(),
    );
    stream.write_all(request.as_bytes())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("malformed HTTP response from {addr}"))?;
    let status_line = head.lines().next().unwrap_or_default();
    if !status_line.contains(" 200 ") {
        bail!("{tool} failed, {status_line}: {body}");
    }
    Ok(body.to_string())
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        sync::{Arc, mpsc},
        thread::sleep,
    };


    use super::*;
    use crate::{console::{Console, ConsoleSpec}, control::{Control, Role}, log::ConsoleLog, mcp, registry::Registry, ring::DEFAULT_RING_CAP};

    #[test]
    fn call_roundtrip_via_http_side_door() {
        let dir = env::temp_dir().join("smon-client-test");
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        let log = ConsoleLog::open_in(dir.clone(), "COMTEST", 0, None).unwrap();
        let console = Console::new(
            ConsoleSpec {
                device:   "COMTEST".to_string(),
                label:    Some("under-test".to_string()),
                baud:     9600,
                eol:      b"\r\n".to_vec(),
                ring_cap: DEFAULT_RING_CAP,
                bridge:   None,
            },
            log,
            mpsc::channel().0,
        );
        let registry = Registry::new(vec![Arc::clone(&console)], 0);

        let (ready_tx, ready_rx) = mpsc::channel();
        let control = Arc::new(Control::new(Role::Tui));
        let server = mcp::spawn("127.0.0.1:0".parse().unwrap(), registry, Arc::clone(&control), ready_tx);

        let addr = ready_rx.recv().unwrap().unwrap();
        let body = call(addr, "serial_status", "").unwrap();
        assert!(body.contains("COMTEST"), "unexpected body: {body}");
        assert!(body.contains("under-test"), "label missing: {body}");

        // The name is how a client addresses a console, so it has to work over
        // the wire and not only in the registry's own tests.
        let by_label = call(addr, "serial_snapshot", r#"{"console":"under-test"}"#).unwrap();
        assert_eq!(by_label, "\"\"");
        let missing = call(addr, "serial_status", r#"{"console":"nope"}"#).unwrap_err().to_string();
        assert!(missing.contains("400"), "unexpected error: {missing}");

        let error = call(addr, "no_such_tool", "{}").unwrap_err().to_string();
        assert!(error.contains("404"), "unexpected error: {error}");

        assert!(control.release(), "server thread ended early");
        // The server is detached on purpose, so give it a moment to unbind
        // rather than joining a thread a client could be holding open.
        sleep(Duration::from_millis(50));
        drop(server);
        fs::remove_dir_all(&dir).unwrap();
    }
}
