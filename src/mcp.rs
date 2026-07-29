//! The MCP side of smon: a Streamable HTTP server exposing the serial tools
//! over the consoles a daemon owns.
//!
//! Every tool takes an optional `console`. With one console open the name can be
//! left out, and with several it is required, because picking a board for the
//! caller is not something this may guess at.

use std::{
    io::ErrorKind,
    net::SocketAddr,
    sync::{Arc, mpsc::Sender as ReadySender},
    thread,
};

use anyhow::{Result, anyhow};
use axum::{
    Router,
    extract::FromRef,
    routing::{get, post},
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
use tokio::{net::TcpListener, runtime::Builder, sync::oneshot};

use crate::{
    attach::attach,
    console::Console,
    control::Control,
    http::call_http,
    registry::{Adopt, Registry},
};

/// The consoles a request can reach, and the process serving them. Handlers take
/// whichever half they need, so the ones that only touch consoles are unchanged.
#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Registry>,
    pub control:  Arc<Control>,
}

impl FromRef<AppState> for Arc<Registry> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.registry)
    }
}

impl FromRef<AppState> for Arc<Control> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.control)
    }
}

// How many consecutive ports the TUI tries from the requested one, so several
// standalone instances on one machine each get an endpoint. The daemon does not
// hunt, a service that quietly moves its port is worse than one that fails.
pub(crate) const PORT_HUNT_RANGE: u16 = 16;

fn default_true() -> bool {
    true
}

fn default_lines() -> usize {
    40
}

fn default_baud() -> u32 {
    115_200
}

fn default_eol() -> String {
    "crlf".to_string()
}

fn default_ring_kb() -> usize {
    crate::ring::DEFAULT_RING_CAP / 1024
}

/// Named by every tool. Left out when only one console is open.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct Which {
    /// Console label or device path. Optional when only one console is open.
    #[serde(default)]
    pub console: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SendReq {
    /// Console label or device path. Optional when only one console is open.
    #[serde(default)]
    pub console: Option<String>,
    /// Text to write to the serial port.
    pub text:    String,
    /// Append the console end-of-line after the text. Default true.
    #[serde(default = "default_true")]
    pub newline: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SendCtrlReq {
    /// Console label or device path. Optional when only one console is open.
    #[serde(default)]
    pub console: Option<String>,
    /// A single letter or symbol, e.g. "c" for Ctrl+C.
    pub ctrl:    String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ReadReq {
    /// Console label or device path. Optional when only one console is open.
    #[serde(default)]
    pub console: Option<String>,
    /// Return output received after this cursor. Omit for the whole retained buffer.
    #[serde(default)]
    pub cursor:  Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ExpectReq {
    /// Console label or device path. Optional when only one console is open.
    #[serde(default)]
    pub console:    Option<String>,
    /// Text to wait for, or a regular expression when `regex` is true.
    pub pattern:    String,
    /// Give up after this many milliseconds. Capped at 120000.
    pub timeout_ms: u64,
    /// Treat `pattern` as a regular expression. Default false.
    #[serde(default)]
    pub regex:      bool,
    /// Scan from this cursor. Omit to wait for new output only.
    #[serde(default)]
    pub cursor:     Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SnapshotReq {
    /// Console label or device path. Optional when only one console is open.
    #[serde(default)]
    pub console: Option<String>,
    /// How many trailing lines to return. Default 40.
    #[serde(default = "default_lines")]
    pub lines:   usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RollReq {
    /// Console label or device path. Optional when only one console is open.
    #[serde(default)]
    pub console: Option<String>,
    /// Free-form text added to the file name, such as a ticket id. smon gives
    /// it no meaning.
    #[serde(default)]
    pub tag:     Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AdoptReq {
    /// The serial device to take over, for example /dev/ttyUSB1 or COM7.
    pub device:  String,
    /// A name for it. smon gives it no meaning, it only makes the console
    /// addressable by something shorter than its device path.
    #[serde(default)]
    pub label:   Option<String>,
    #[serde(default = "default_baud")]
    pub baud:    u32,
    #[serde(default = "default_eol")]
    pub eol:     String,
    #[serde(default = "default_ring_kb")]
    pub ring_kb: usize,
}

impl From<AdoptReq> for Adopt {
    fn from(req: AdoptReq) -> Adopt {
        Adopt {
            device:  req.device,
            label:   req.label,
            baud:    req.baud,
            eol:     req.eol,
            ring_kb: req.ring_kb,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct Cursor {
    /// Cursor just before the write. Read or expect from here to capture the reply.
    pub cursor: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ReadResult {
    /// Output text.
    pub data:   String,
    /// Cursor to pass next time to continue where this left off.
    pub cursor: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ExpectResult {
    /// Whether the pattern was found before the timeout.
    pub matched:   bool,
    /// Output from the start point up to the match, or up to the end on timeout.
    pub data:      String,
    /// Cursor at the end of `data`.
    pub cursor:    u64,
    /// Whether the wait timed out.
    pub timed_out: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub(crate) struct StatusResult {
    /// The device path this console holds.
    pub port:      String,
    /// The console's label, when it has one.
    pub label:     Option<String>,
    pub baud:      u32,
    pub connected: bool,
    pub cursor:    u64,
    /// The log segment currently being written.
    pub log:       String,
    /// True while another program has been handed the device.
    #[serde(default)]
    pub released:  bool,
    /// The loopback port this console is offered as raw bytes on, when it is.
    #[serde(default)]
    pub bridge:    Option<u16>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct LogResult {
    /// The log segment now being written.
    pub path:    String,
    /// When this segment was started, in local time.
    pub started: String,
}

pub(crate) fn status_of(console: &Arc<Console>) -> StatusResult {
    StatusResult {
        port:      console.device().to_string(),
        label:     console.label().map(str::to_string),
        baud:      console.baud(),
        connected: console.connected(),
        cursor:    console.total(),
        log:       console.log_info().path.display().to_string(),
        released:  console.released(),
        bridge:    console.bridge_port(),
    }
}

pub(crate) fn log_result(info: &crate::log::LogInfo) -> LogResult {
    LogResult {
        path:    info.path.display().to_string(),
        started: info.started.format("%Y-%m-%d %H:%M:%S").to_string(),
    }
}

/// Cloned per session by the transport. Every clone shares the same registry.
#[derive(Clone)]
struct Server {
    registry: Arc<Registry>,
}

impl Server {
    fn console(&self, name: Option<&str>) -> Result<Arc<Console>, McpError> {
        self.registry
            .resolve(name)
            .map_err(|e| McpError::invalid_params(e, None))
    }
}

#[tool_router]
impl Server {
    #[tool(description = "List the consoles this smon owns, with label, device, baud and state.")]
    async fn console_list(&self) -> Json<Vec<StatusResult>> {
        Json(self.registry.all().iter().map(status_of).collect())
    }

    #[tool(description = "Write text to a serial console. Returns a cursor to read the reply from.")]
    async fn serial_send(
        &self,
        Parameters(req): Parameters<SendReq>,
    ) -> Result<Json<Cursor>, McpError> {
        let cursor = self
            .console(req.console.as_deref())?
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
            .console(req.console.as_deref())?
            .send_ctrl(ch)
            .await
            .map_err(|e| McpError::internal_error(e, None))?;
        Ok(Json(Cursor { cursor }))
    }

    #[tool(description = "Read serial output received since a cursor. Omit cursor for the whole buffer.")]
    async fn serial_read(
        &self,
        Parameters(req): Parameters<ReadReq>,
    ) -> Result<Json<ReadResult>, McpError> {
        let (data, cursor) = self.console(req.console.as_deref())?.read(req.cursor);
        Ok(Json(ReadResult { data, cursor }))
    }

    #[tool(
        description = "Wait until a pattern appears in serial output, or until timeout. Substring by default, regex optional."
    )]
    async fn serial_expect(
        &self,
        Parameters(req): Parameters<ExpectReq>,
    ) -> Result<Json<ExpectResult>, McpError> {
        let out = self
            .console(req.console.as_deref())?
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

    #[tool(description = "Return the last N lines currently in a console's buffer.")]
    async fn serial_snapshot(
        &self,
        Parameters(req): Parameters<SnapshotReq>,
    ) -> Result<String, McpError> {
        Ok(self.console(req.console.as_deref())?.snapshot(req.lines))
    }

    #[tool(description = "Report a console's device, baud, connection state, cursor and log file.")]
    async fn serial_status(
        &self,
        Parameters(req): Parameters<Which>,
    ) -> Result<Json<StatusResult>, McpError> {
        Ok(Json(status_of(&self.console(req.console.as_deref())?)))
    }

    #[tool(
        description = "Start a new log file for a console and return its path. Call this at the start of a run so its output lands in a file of its own."
    )]
    async fn log_roll(
        &self,
        Parameters(req): Parameters<RollReq>,
    ) -> Result<Json<LogResult>, McpError> {
        let info = self
            .console(req.console.as_deref())?
            .log_roll(req.tag.as_deref())
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(Json(log_result(&info)))
    }

    #[tool(
        description = "Take over a serial device that is not open here yet and start logging it. The device must exist and be free."
    )]
    async fn console_adopt(
        &self,
        Parameters(req): Parameters<AdoptReq>,
    ) -> Result<Json<StatusResult>, McpError> {
        let console = self
            .registry
            .adopt(req.into())
            .map_err(|e| McpError::invalid_params(e, None))?;
        Ok(Json(status_of(&console)))
    }

    #[tool(
        description = "Let go of a console's device so another program can open it. The console keeps its buffer and log. Call console_hold to take it back."
    )]
    async fn console_release(
        &self,
        Parameters(req): Parameters<Which>,
    ) -> Result<Json<StatusResult>, McpError> {
        let console = self.console(req.console.as_deref())?;
        if !console.release().await {
            return Err(McpError::internal_error(
                format!("{} did not let go of its device", console.name()),
                None,
            ));
        }
        Ok(Json(status_of(&console)))
    }

    #[tool(description = "Take a released console's device back and reopen it.")]
    async fn console_hold(
        &self,
        Parameters(req): Parameters<Which>,
    ) -> Result<Json<StatusResult>, McpError> {
        let console = self.console(req.console.as_deref())?;
        console.hold();
        Ok(Json(status_of(&console)))
    }

    #[tool(description = "Report the log file a console is writing to now, and when it was started.")]
    async fn log_info(
        &self,
        Parameters(req): Parameters<Which>,
    ) -> Result<Json<LogResult>, McpError> {
        let info = self.console(req.console.as_deref())?.log_info();
        Ok(Json(log_result(&info)))
    }
}

#[tool_handler]
impl ServerHandler for Server {}

/// Serve on `bind` and block until the process ends. Used by the daemon, which
/// binds exactly what it was told and fails loudly when that port is taken.
///
/// # Errors
/// Returns an error if the runtime cannot start or the bind fails.
pub fn run(bind: SocketAddr, registry: Arc<Registry>, control: Arc<Control>) -> Result<()> {
    let runtime = Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async move {
        let listener = TcpListener::bind(bind)
            .await
            .map_err(|e| anyhow!("binding {bind}: {e}"))?;
        let addr = listener.local_addr().unwrap_or(bind);
        println!("smon: serving http://{addr}/mcp");
        // The daemon serves until the process ends, or until an update asks it
        // to let the port go so its replacement can bind.
        let (release, stopped) = oneshot::channel();
        control.arm(release);
        serve(listener, AppState { registry, control }, stopped).await;
        Ok(())
    })
}

/// Start the server on its own thread with its own runtime, hunting for a free
/// port. The bind result is reported once through `ready`, and the server stops
/// when `control` releases it or the process exits. Used by the TUI.
pub fn spawn(
    bind: SocketAddr,
    registry: Arc<Registry>,
    control: Arc<Control>,
    ready: ReadySender<Result<SocketAddr, String>>,
) -> thread::JoinHandle<()> {
    let (release, shutdown) = oneshot::channel();
    control.arm(release);
    thread::spawn(move || {
        // One local client at a time talks to this server. A single-threaded
        // runtime is enough and avoids spawning a worker thread per core.
        let runtime = match Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(e) => {
                report(&ready, Err(format!("tokio runtime: {e}")));
                return;
            }
        };
        runtime.block_on(async move {
            let listener = match bind_hunting(bind).await {
                Ok(listener) => listener,
                Err(e) => {
                    report(&ready, Err(e));
                    return;
                }
            };
            let addr = listener.local_addr().unwrap_or(bind);
            report(&ready, Ok(addr));
            serve(listener, AppState { registry, control }, shutdown).await;
        });
    })
}

// Whether the session was still waiting to hear where the server bound.
fn report(ready: &ReadySender<Result<SocketAddr, String>>, outcome: Result<SocketAddr, String>) -> bool {
    ready.send(outcome).is_ok()
}

/// Bind the requested address. When the port is taken by another instance, hunt
/// upward through the next ports so every instance gets its own endpoint.
async fn bind_hunting(bind: SocketAddr) -> Result<TcpListener, String> {
    for offset in 0..PORT_HUNT_RANGE {
        let Some(port) = bind.port().checked_add(offset) else {
            break;
        };
        let addr = SocketAddr::new(bind.ip(), port);
        match TcpListener::bind(addr).await {
            Ok(listener) => return Ok(listener),
            Err(e) if e.kind() == ErrorKind::AddrInUse => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Err(format!(
        "ports {}..{} all in use",
        bind.port(),
        bind.port().saturating_add(PORT_HUNT_RANGE - 1)
    ))
}

async fn serve(listener: TcpListener, state: AppState, shutdown: oneshot::Receiver<()>) {
    for console in state.registry.all() {
        if let Some(port) = console.bridge_port() {
            tokio::spawn(crate::bridge::serve(console, port));
        }
    }

    let mcp_registry = Arc::clone(&state.registry);
    let service = StreamableHttpService::new(
        move || {
            Ok(Server {
                registry: Arc::clone(&mcp_registry),
            })
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let app = Router::new()
        .route_service("/mcp", service)
        .route("/call/{tool}", post(call_http))
        .route("/attach/{console}", get(attach))
        .with_state(state);

    // A stop signal, or the sender going away without sending one. Both end the
    // server, so the two cases are deliberately the same.
    let graceful = async move { shutdown.await.unwrap_or(()) };
    match axum::serve(listener, app)
        .with_graceful_shutdown(graceful)
        .await
    {
        Ok(()) => {}
        Err(e) => eprintln!("smon: http server stopped: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Role;

    // Two standalone instances must not fight over one port, the second hunts on.
    #[test]
    fn bind_hunting_skips_taken_port() {
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let requested = taken.local_addr().unwrap();
            let hunted = bind_hunting(requested).await.unwrap();
            let port = hunted.local_addr().unwrap().port();
            assert!(port > requested.port());
            assert!(port < requested.port() + PORT_HUNT_RANGE);
        });
    }

    // The daemon must not quietly move to another port. A taken bind is a
    // misconfiguration and has to be visible.
    #[test]
    fn the_daemon_bind_does_not_hunt() {
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        let taken = runtime.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
        let addr = taken.local_addr().unwrap();
        let control = Arc::new(Control::new(Role::Daemon));
        let error = run(addr, Registry::new(Vec::new(), 0), control).unwrap_err().to_string();
        assert!(error.contains(&addr.to_string()), "{error}");
    }
}
