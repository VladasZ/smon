//! Reaching an smon on another machine, over ssh.
//!
//! A daemon binds loopback only, so a console is never exposed to the network.
//! To attach from elsewhere the client forwards a local port over ssh and talks
//! to that, which means it inherits whatever already guards ssh to that host and
//! opens nothing new.

use std::{
    net::{SocketAddr, TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

const READY_TIMEOUT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(100);

pub struct Tunnel {
    child: Child,
    addr:  SocketAddr,
}

impl Tunnel {
    /// Where to talk to the far end, on this machine.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        if let Err(e) = self.child.kill() {
            eprintln!("smon: could not stop the ssh tunnel: {e}");
        }
        if let Err(e) = self.child.wait() {
            eprintln!("smon: could not reap the ssh tunnel: {e}");
        }
    }
}

/// Forward a local port to `remote` on `host` over ssh and wait for it to answer.
///
/// # Errors
/// Returns an error if ssh cannot start, or the forwarded port does not accept
/// a connection in time, which is what a refused login or a missing daemon on
/// the far side both look like from here.
pub fn open(host: &str, remote: SocketAddr) -> Result<Tunnel> {
    let local = free_port().context("finding a local port for the ssh tunnel")?;
    let forward = format!("{local}:127.0.0.1:{}", remote.port());
    let child = Command::new("ssh")
        .args([
            "-N",
            // Fail the tunnel rather than sit there uselessly when the port
            // cannot be forwarded.
            "-o",
            "ExitOnForwardFailure=yes",
            "-L",
            &forward,
            host,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .with_context(|| format!("starting ssh to {host}"))?;

    let addr = SocketAddr::from(([127, 0, 0, 1], local));
    let mut tunnel = Tunnel { child, addr };
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        // An early exit means ssh itself failed, and its own message is already
        // on the terminal, so there is nothing to add.
        if let Some(status) = tunnel.child.try_wait()? {
            bail!("ssh to {host} exited with {status}");
        }
        if TcpStream::connect_timeout(&addr, POLL).is_ok() {
            return Ok(tunnel);
        }
        sleep(POLL);
    }
    bail!("no answer through the ssh tunnel to {host} within 15s, is smon running there")
}

// A port nothing is on right now. The listener is dropped before ssh is asked
// for it, so there is a small window where something else could take it. Losing
// that race shows up as a clear ssh forward failure, not as a silent wrong
// connection.
fn free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}
