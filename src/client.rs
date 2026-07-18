//! One-shot CLI client for the /call side door of a running smon instance.
//! Plain blocking HTTP over a localhost socket, no client dependencies, so
//! `smon call` and `smon list` work anywhere the binary does.

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::{DEFAULT_MCP, mcp::PORT_HUNT_RANGE};

// serial_expect can legitimately wait 120 s, everything else answers instantly.
const READ_TIMEOUT: Duration = Duration::from_secs(125);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

/// Call one tool on the smon instance serving the given local port and return
/// the response body, a JSON document.
pub fn call(port: u16, tool: &str, args: &str) -> Result<String> {
    let stream = connect(port, CONNECT_TIMEOUT)
        .with_context(|| format!("no smon MCP on port {port}, try smon list"))?;
    request(stream, port, tool, args)
}

/// Probe the port hunt range on localhost and print one line per running
/// instance: MCP port, serial port, baud, connection state.
pub fn list() -> Result<()> {
    let base = DEFAULT_MCP.parse::<SocketAddr>()?.port();
    let mut found = 0;
    for port in base..base.saturating_add(PORT_HUNT_RANGE) {
        let Ok(stream) = connect(port, Duration::from_millis(200)) else {
            continue;
        };
        let body = request(stream, port, "serial_status", "{}")?;
        let status: Value = serde_json::from_str(&body)
            .with_context(|| format!("bad serial_status reply from port {port}"))?;
        let serial = status["port"].as_str().unwrap_or("?");
        let baud = status["baud"].as_u64().unwrap_or(0);
        let connected = if status["connected"].as_bool().unwrap_or(false) {
            "connected"
        } else {
            "disconnected"
        };
        println!("{port}  {serial}  {baud}  {connected}");
        found += 1;
    }
    if found == 0 {
        println!("no running smon instances found on ports {base}..{}", base + PORT_HUNT_RANGE - 1);
    }
    Ok(())
}

fn connect(port: u16, timeout: Duration) -> Result<TcpStream> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    Ok(TcpStream::connect_timeout(&addr, timeout)?)
}

fn request(mut stream: TcpStream, port: u16, tool: &str, args: &str) -> Result<String> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    let request = format!(
        "POST /call/{tool} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{args}",
        args.len(),
    );
    stream.write_all(request.as_bytes())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("malformed HTTP response from port {port}"))?;
    let status_line = head.lines().next().unwrap_or_default();
    if !status_line.contains(" 200 ") {
        bail!("{tool} failed, {status_line}: {body}");
    }
    Ok(body.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, mpsc};

    use tokio::sync::{mpsc::unbounded_channel, oneshot};

    use super::*;
    use crate::mcp::{self, Shared};

    #[test]
    fn call_roundtrip_via_http_side_door() {
        let inject = unbounded_channel().0;
        let state = Shared::new("COMTEST".to_string(), 9600, b"\r\n".to_vec(), inject);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = mcp::spawn("127.0.0.1:0".parse().unwrap(), Arc::clone(&state), ready_tx, shutdown_rx);

        let addr = ready_rx.recv().unwrap().unwrap();
        let body = call(addr.port(), "serial_status", "").unwrap();
        assert!(body.contains("COMTEST"), "unexpected body: {body}");

        let error = call(addr.port(), "no_such_tool", "{}").unwrap_err().to_string();
        assert!(error.contains("404"), "unexpected error: {error}");

        shutdown_tx.send(()).expect("server thread ended early");
        server.join().expect("server thread panicked");
    }
}
