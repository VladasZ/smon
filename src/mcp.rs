//! The MCP side of smon: shared serial-console state plus a Streamable HTTP MCP
//! server that exposes generic serial tools. It knows nothing about any board.
//!
//! The serial session feeds received bytes into a rolling buffer here, and MCP
//! tools read that buffer and queue input back through an inject channel that
//! the session drains onto the port. All board-specific behaviour belongs in a
//! separate MCP server that uses these generic tools as a client.

use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender as ReadySender,
    },
    thread,
    time::{Duration, Instant},
};

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::wrapper::{Json, Parameters},
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpService, session::local::LocalSessionManager,
        tower::StreamableHttpServerConfig,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, mpsc::UnboundedSender, oneshot};

// Rolling buffer cap. At 115200 baud the console fills slowly, so a few hundred
// KB keeps enough scrollback that expect() can still find a line that scrolled
// past a moment ago.
const RING_CAP: usize = 512 * 1024;

// Hard ceiling on a single expect() wait, so one call cannot block a client
// forever. Longer waits are done by the client re-calling.
const MAX_EXPECT_MS: u64 = 120_000;

/// Input queued by an MCP tool for the session to write to the port. `echo` is
/// the display form shown in the TUI; `resp` reports the write result back.
pub struct Inject {
    pub bytes: Vec<u8>,
    pub echo:  String,
    pub resp:  oneshot::Sender<Result<(), String>>,
}

/// The result of an [`Shared::expect`] wait.
pub struct Expect {
    pub matched:   bool,
    pub data:      String,
    pub cursor:    u64,
    pub timed_out: bool,
}

/// A rolling window of recently received bytes, addressed by an absolute offset
/// so a client can page through with a cursor even after old bytes are dropped.
struct Ring {
    buf:  Vec<u8>,
    base: u64, // absolute offset of buf[0]
}

impl Ring {
    fn new() -> Self {
        Self {
            buf:  Vec::new(),
            base: 0,
        }
    }

    fn total(&self) -> u64 {
        self.base + self.buf.len() as u64
    }

    fn append(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        if self.buf.len() > RING_CAP {
            let drop = self.buf.len() - RING_CAP;
            self.buf.drain(..drop);
            self.base += drop as u64;
        }
    }

    // Bytes from `cursor` to the end, plus the absolute offset the slice starts
    // at. `cursor` is clamped into the retained window, so a cursor pointing at
    // bytes already dropped simply starts at the oldest retained byte.
    fn slice_from(&self, cursor: u64) -> (u64, &[u8]) {
        let start = cursor.clamp(self.base, self.total());
        let idx = (start - self.base) as usize;
        (start, &self.buf[idx..])
    }
}

// Either a plain substring or a compiled regex, matched against raw bytes so the
// offsets it returns line up with the ring.
enum Matcher {
    Substr(Vec<u8>),
    Regex(regex::bytes::Regex),
}

impl Matcher {
    fn build(pattern: &str, regex: bool) -> Result<Self, String> {
        if regex {
            regex::bytes::Regex::new(pattern)
                .map(Matcher::Regex)
                .map_err(|e| e.to_string())
        } else {
            Ok(Matcher::Substr(pattern.as_bytes().to_vec()))
        }
    }

    // Offset just past the first match in `hay`, or None.
    fn find_end(&self, hay: &[u8]) -> Option<usize> {
        match self {
            Matcher::Substr(needle) => find_sub(hay, needle).map(|i| i + needle.len()),
            Matcher::Regex(re) => re.find(hay).map(|m| m.end()),
        }
    }
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

// The control byte for Ctrl+<c>, matching the mapping the TUI uses for keystrokes.
fn ctrl_byte(c: char) -> Option<u8> {
    let lower = c.to_ascii_lowercase();
    Some(match lower {
        'a'..='z' => (lower as u8) - b'a' + 1,
        '@' => 0,
        '[' => 0x1b,
        '\\' => 0x1c,
        ']' => 0x1d,
        '^' => 0x1e,
        '_' => 0x1f,
        _ => return None,
    })
}

/// Shared serial-console state. Cheap to clone via `Arc`. The reader thread
/// appends received bytes; MCP tools read the buffer and queue input.
pub struct Shared {
    ring:      Mutex<Ring>,
    notify:    Notify,
    inject:    UnboundedSender<Inject>,
    eol:       Vec<u8>,
    port:      String,
    baud:      u32,
    connected: AtomicBool,
}

impl Shared {
    pub fn new(port: String, baud: u32, eol: Vec<u8>, inject: UnboundedSender<Inject>) -> Arc<Self> {
        Arc::new(Self {
            ring: Mutex::new(Ring::new()),
            notify: Notify::new(),
            inject,
            eol,
            port,
            baud,
            connected: AtomicBool::new(true),
        })
    }

    /// Append received bytes and wake any waiting `expect` calls. Called from
    /// the serial reader thread.
    pub fn push_rx(&self, bytes: &[u8]) {
        self.ring.lock().unwrap().append(bytes);
        self.notify.notify_waiters();
    }

    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::Relaxed);
    }

    fn total(&self) -> u64 {
        self.ring.lock().unwrap().total()
    }

    /// Bytes received since `cursor` (or the whole retained buffer if `None`),
    /// with the new cursor to pass next time.
    pub fn read(&self, cursor: Option<u64>) -> (String, u64) {
        let ring = self.ring.lock().unwrap();
        let start = cursor.unwrap_or(ring.base);
        let (abs, hay) = ring.slice_from(start);
        (
            String::from_utf8_lossy(hay).into_owned(),
            abs + hay.len() as u64,
        )
    }

    /// The last `lines` complete lines currently in the buffer.
    pub fn snapshot(&self, lines: usize) -> String {
        let ring = self.ring.lock().unwrap();
        let text = String::from_utf8_lossy(&ring.buf);
        let all: Vec<&str> = text.lines().collect();
        let start = all.len().saturating_sub(lines);
        all[start..].join("\n")
    }

    /// Port name, baud, whether the port is still connected, and the current
    /// read cursor.
    pub fn status(&self) -> (String, u32, bool, u64) {
        (
            self.port.clone(),
            self.baud,
            self.connected.load(Ordering::Relaxed),
            self.total(),
        )
    }

    /// Write `text` to the port, appending the session end-of-line when
    /// `newline`. Returns the cursor just before the write, so a following
    /// read/expect captures the echo and reply.
    pub async fn send(&self, text: String, newline: bool) -> Result<u64, String> {
        let cursor = self.total();
        let echo = text.clone();
        let mut bytes = text.into_bytes();
        if newline {
            bytes.extend_from_slice(&self.eol);
        }
        self.inject_and_wait(bytes, echo).await?;
        Ok(cursor)
    }

    /// Send a single Ctrl+<char> control byte. Returns the cursor before the write.
    pub async fn send_ctrl(&self, ctrl: char) -> Result<u64, String> {
        let byte = ctrl_byte(ctrl).ok_or_else(|| format!("no control byte for '{ctrl}'"))?;
        let cursor = self.total();
        let echo = format!("Ctrl+{}", ctrl.to_ascii_uppercase());
        self.inject_and_wait(vec![byte], echo).await?;
        Ok(cursor)
    }

    async fn inject_and_wait(&self, bytes: Vec<u8>, echo: String) -> Result<(), String> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.inject
            .send(Inject {
                bytes,
                echo,
                resp: resp_tx,
            })
            .map_err(|_| "serial session closed".to_string())?;
        match resp_rx.await {
            Ok(result) => result,
            Err(_) => Err("serial session closed".to_string()),
        }
    }

    /// Wait until `pattern` appears in received data, or `timeout_ms` elapses.
    /// Scans from `cursor` if given, else from the current end (new data only).
    /// Returns the text from the start point up to and including the match, or
    /// up to the end on timeout, plus the cursor to continue from.
    pub async fn expect(
        &self,
        pattern: &str,
        timeout_ms: u64,
        regex: bool,
        cursor: Option<u64>,
    ) -> Result<Expect, String> {
        let matcher = Matcher::build(pattern, regex)?;
        let start = cursor.unwrap_or_else(|| self.total());
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(MAX_EXPECT_MS));

        loop {
            // Register interest before scanning so a byte that arrives between
            // the scan and the wait cannot be missed.
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            {
                let ring = self.ring.lock().unwrap();
                let (abs, hay) = ring.slice_from(start);
                if let Some(end) = matcher.find_end(hay) {
                    return Ok(Expect {
                        matched:   true,
                        data:      String::from_utf8_lossy(&hay[..end]).into_owned(),
                        cursor:    abs + end as u64,
                        timed_out: false,
                    });
                }
            }

            let now = Instant::now();
            if now >= deadline {
                let ring = self.ring.lock().unwrap();
                let (abs, hay) = ring.slice_from(start);
                return Ok(Expect {
                    matched:   false,
                    data:      String::from_utf8_lossy(hay).into_owned(),
                    cursor:    abs + hay.len() as u64,
                    timed_out: true,
                });
            }

            tokio::select! {
                () = &mut notified => {}
                () = tokio::time::sleep(deadline - now) => {}
            }
        }
    }
}

// ----- MCP server ----------------------------------------------------------

fn default_true() -> bool {
    true
}

fn default_lines() -> usize {
    40
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SendReq {
    /// Text to write to the serial port.
    text: String,
    /// Append the session end-of-line after the text. Default true.
    #[serde(default = "default_true")]
    newline: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SendCtrlReq {
    /// A single letter or symbol, e.g. "c" for Ctrl+C.
    ctrl: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ReadReq {
    /// Return output received after this cursor. Omit for the whole retained buffer.
    #[serde(default)]
    cursor: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ExpectReq {
    /// Text to wait for, or a regular expression when `regex` is true.
    pattern: String,
    /// Give up after this many milliseconds. Capped at 120000.
    timeout_ms: u64,
    /// Treat `pattern` as a regular expression. Default false.
    #[serde(default)]
    regex: bool,
    /// Scan from this cursor. Omit to wait for new output only.
    #[serde(default)]
    cursor: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SnapshotReq {
    /// How many trailing lines to return. Default 40.
    #[serde(default = "default_lines")]
    lines: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
struct Cursor {
    /// Cursor just before the write; read or expect from here to capture the reply.
    cursor: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ReadResult {
    /// Output text.
    data: String,
    /// Cursor to pass next time to continue where this left off.
    cursor: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ExpectResult {
    /// Whether the pattern was found before the timeout.
    matched: bool,
    /// Output from the start point up to the match, or up to the end on timeout.
    data: String,
    /// Cursor at the end of `data`.
    cursor: u64,
    /// Whether the wait timed out.
    timed_out: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
struct StatusResult {
    port:      String,
    baud:      u32,
    connected: bool,
    cursor:    u64,
}

/// The MCP server, exposing generic serial tools over the shared console state.
/// Cloned per session by the transport; every clone shares the same `Shared`.
#[derive(Clone)]
struct Server {
    state: Arc<Shared>,
}

#[tool_router]
impl Server {
    #[tool(description = "Write text to the serial port. Returns a cursor to read the reply from.")]
    async fn serial_send(&self, Parameters(req): Parameters<SendReq>) -> Result<Json<Cursor>, McpError> {
        let cursor = self
            .state
            .send(req.text, req.newline)
            .await
            .map_err(|e| McpError::internal_error(e, None))?;
        Ok(Json(Cursor { cursor }))
    }

    #[tool(description = "Send a single Ctrl+<char> control byte, e.g. ctrl \"c\" for Ctrl+C.")]
    async fn serial_send_ctrl(
        &self,
        Parameters(req): Parameters<SendCtrlReq>,
    ) -> Result<Json<Cursor>, McpError> {
        let ch = req
            .ctrl
            .chars()
            .next()
            .ok_or_else(|| McpError::invalid_params("ctrl must be one character", None))?;
        let cursor = self
            .state
            .send_ctrl(ch)
            .await
            .map_err(|e| McpError::internal_error(e, None))?;
        Ok(Json(Cursor { cursor }))
    }

    #[tool(description = "Read serial output received since a cursor. Omit cursor for the whole buffer.")]
    async fn serial_read(&self, Parameters(req): Parameters<ReadReq>) -> Json<ReadResult> {
        let (data, cursor) = self.state.read(req.cursor);
        Json(ReadResult { data, cursor })
    }

    #[tool(
        description = "Wait until a pattern appears in serial output, or until timeout. Substring by default, regex optional."
    )]
    async fn serial_expect(
        &self,
        Parameters(req): Parameters<ExpectReq>,
    ) -> Result<Json<ExpectResult>, McpError> {
        let out = self
            .state
            .expect(&req.pattern, req.timeout_ms, req.regex, req.cursor)
            .await
            .map_err(|e| McpError::invalid_params(e, None))?;
        Ok(Json(ExpectResult {
            matched:   out.matched,
            data:      out.data,
            cursor:    out.cursor,
            timed_out: out.timed_out,
        }))
    }

    #[tool(description = "Return the last N lines currently in the serial buffer.")]
    async fn serial_snapshot(&self, Parameters(req): Parameters<SnapshotReq>) -> String {
        self.state.snapshot(req.lines)
    }

    #[tool(description = "Report the serial port, baud, whether it is connected, and the current cursor.")]
    async fn serial_status(&self) -> Json<StatusResult> {
        let (port, baud, connected, cursor) = self.state.status();
        Json(StatusResult {
            port,
            baud,
            connected,
            cursor,
        })
    }
}

#[tool_handler]
impl ServerHandler for Server {}

/// Start the MCP server on its own thread with its own tokio runtime. The bind
/// result is reported once through `ready`, so a bind failure is a warning
/// rather than fatal. The server stops when `shutdown` fires or the process
/// exits.
pub fn spawn(
    bind: SocketAddr,
    state: Arc<Shared>,
    ready: ReadySender<Result<SocketAddr, String>>,
    shutdown: oneshot::Receiver<()>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(e) => {
                let _ = ready.send(Err(format!("tokio runtime: {e}")));
                return;
            }
        };
        runtime.block_on(serve(bind, state, ready, shutdown));
    })
}

async fn serve(
    bind: SocketAddr,
    state: Arc<Shared>,
    ready: ReadySender<Result<SocketAddr, String>>,
    shutdown: oneshot::Receiver<()>,
) {
    let listener = match tokio::net::TcpListener::bind(bind).await {
        Ok(listener) => listener,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let addr = listener.local_addr().unwrap_or(bind);
    let _ = ready.send(Ok(addr));

    let service = StreamableHttpService::new(
        move || Ok(Server { state: Arc::clone(&state) }),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let app = axum::Router::new().route_service("/mcp", service);

    let graceful = async move {
        let _ = shutdown.await;
    };
    let _ = axum::serve(listener, app)
        .with_graceful_shutdown(graceful)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_drops_oldest_and_tracks_offset() {
        let mut ring = Ring::new();
        ring.append(b"hello ");
        ring.append(b"world");
        assert_eq!(ring.total(), 11);
        let (abs, slice) = ring.slice_from(6);
        assert_eq!(abs, 6);
        assert_eq!(slice, b"world");
    }

    #[test]
    fn ring_cursor_before_window_clamps_to_base() {
        let mut ring = Ring { buf: Vec::new(), base: 100 };
        ring.append(b"abc");
        let (abs, slice) = ring.slice_from(0);
        assert_eq!(abs, 100);
        assert_eq!(slice, b"abc");
    }

    #[test]
    fn substr_match_returns_offset_past_match() {
        let m = Matcher::build("-> ", false).unwrap();
        // "-> " sits at bytes 11..14, so the offset just past it is 14.
        assert_eq!(m.find_end(b"value = 1\r\n-> "), Some(14));
        assert_eq!(m.find_end(b"still running"), None);
    }

    #[test]
    fn regex_match_finds_process_tag() {
        let m = Matcher::build(r"\(P\d\)", true).unwrap();
        // "(P1)" sits at bytes 13..17, so the offset just past it is 17.
        assert_eq!(m.find_end(b"No AliveFlag (P1) 61 sec"), Some(17));
        assert_eq!(m.find_end(b"No AliveFlag yet"), None);
    }

    #[test]
    fn ctrl_byte_maps_letters_and_symbols() {
        assert_eq!(ctrl_byte('c'), Some(3));
        assert_eq!(ctrl_byte('C'), Some(3));
        assert_eq!(ctrl_byte('['), Some(0x1b));
        assert_eq!(ctrl_byte('1'), None);
    }
}
