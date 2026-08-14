//! One serial console: its rolling buffer, its log, and everything a client can
//! ask of it.
//!
//! A console does not own the device. The runner does, and it feeds received
//! bytes in here. Input goes the other way, queued on the inject channel for
//! the runner to write, so the port is only ever written from one place.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    time::{Duration, Instant},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, broadcast, oneshot};

use crate::{
    log::{ConsoleLog, LogInfo},
    ring::{Matcher, Ring},
};

// How many events a viewer can fall behind before it is told it missed some.
// A viewer redraws every 16ms, so this is generous.
const EVENT_QUEUE: usize = 1024;

// Hard ceiling on a single expect() wait, so one call cannot block a client
// forever. Longer waits are done by the client re-calling.
const MAX_EXPECT_MS: u64 = 120_000;

// How long a release waits for the runner to actually close the port. The
// runner notices within its own tick, so this is generous.
const RELEASE_POLL: Duration = Duration::from_millis(50);
const RELEASE_TICKS: usize = 40;

/// Who produced a piece of input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Typed,
    Key,
    Agent,
    /// Input from a raw byte bridge, where the writer is some other program
    /// speaking to the board directly.
    Bridge,
}

/// What a viewer attached to this console sees.
#[derive(Clone)]
pub enum ConsoleEvent {
    Rx(Vec<u8>),
    Echo {
        origin: Origin,
        text:   String,
    },
    System(String),
    /// The port came or went. A viewer reads the new state off the console, so
    /// this only says that it changed.
    Connected,
}

/// Input queued for the runner to write to the port. `echo` is the display form
/// shown to viewers, and `resp` reports the write result back when a caller is
/// waiting on it.
pub struct Inject {
    pub bytes:  Vec<u8>,
    pub echo:   String,
    pub origin: Origin,
    pub resp:   Option<oneshot::Sender<Result<(), String>>>,
}

/// The result of an [`Console::expect`] wait.
pub struct Expect {
    pub matched:   bool,
    pub data:      String,
    pub cursor:    u64,
    pub timed_out: bool,
}

/// Everything a console is made of, apart from its log and its input queue.
pub struct ConsoleSpec {
    pub device:   String,
    pub label:    Option<String>,
    pub baud:     u32,
    pub eol:      Vec<u8>,
    pub ring_cap: usize,
    /// Where to offer this console as raw bytes, when it is offered at all.
    pub bridge:   Option<u16>,
}

pub struct Console {
    device:    String,
    label:     Option<String>,
    baud:      u32,
    eol:       Vec<u8>,
    ring:      Mutex<Ring>,
    notify:    Notify,
    // A std channel, not a tokio one. Sending never blocks, so async callers and
    // the keyboard both queue input without a runtime, and the runner thread can
    // block on it instead of polling.
    inject:    Sender<Inject>,
    events:    broadcast::Sender<ConsoleEvent>,
    log:       Mutex<ConsoleLog>,
    connected: AtomicBool,
    // Set while some other program needs the device node itself. The runner
    // closes the port and stays off it until this clears.
    released:  AtomicBool,
    // Where this console is offered as a raw byte stream, when it is.
    bridge:    Option<u16>,
}

impl Console {
    pub fn new(spec: ConsoleSpec, log: ConsoleLog, inject: Sender<Inject>) -> Arc<Self> {
        let ConsoleSpec {
            device,
            label,
            baud,
            eol,
            ring_cap,
            bridge,
        } = spec;
        Arc::new(Self {
            device,
            label,
            baud,
            eol,
            ring: Mutex::new(Ring::new(ring_cap)),
            notify: Notify::new(),
            inject,
            events: broadcast::Sender::new(EVENT_QUEUE),
            log: Mutex::new(log),
            connected: AtomicBool::new(false),
            released: AtomicBool::new(false),
            bridge,
        })
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// How this console is named to a person, its label when it has one.
    pub fn name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.device)
    }

    /// Does `query` address this console? A label or a device path both work,
    /// and so does the last element of the path, so `ttyUSB2` finds
    /// `/dev/ttyUSB2` without typing the whole thing.
    pub fn matches(&self, query: &str) -> bool {
        if self.label.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(query)) {
            return true;
        }
        if self.device.eq_ignore_ascii_case(query) {
            return true;
        }
        self.device
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(|file| file.eq_ignore_ascii_case(query))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ConsoleEvent> {
        self.events.subscribe()
    }

    /// Called from the runner when the port delivers bytes.
    pub fn push_rx(&self, bytes: &[u8]) {
        self.ring.lock().unwrap().append(bytes);
        self.record(self.log.lock().unwrap().rx(bytes));
        self.notify.notify_waiters();
        self.emit(ConsoleEvent::Rx(bytes.to_vec()));
    }

    /// Called from the runner when it takes input off the inject queue.
    pub fn push_echo(&self, origin: Origin, bytes: &[u8], echo: &str) {
        self.record(self.log.lock().unwrap().tx(origin, bytes, echo));
        self.emit(ConsoleEvent::Echo {
            origin,
            text: echo.to_string(),
        });
    }

    /// A line for the log only. Where the MCP endpoint bound is for agents, so
    /// it belongs in the log and not on the person's screen.
    pub fn note(&self, text: &str) {
        self.record(self.log.lock().unwrap().system(text));
    }

    pub fn push_system(&self, text: &str) {
        self.record(self.log.lock().unwrap().system(text));
        self.emit(ConsoleEvent::System(text.to_string()));
    }

    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::Relaxed);
        self.emit(ConsoleEvent::Connected);
    }

    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Let go of the device so another program can open it. The console keeps
    /// its buffer, its log and its viewers, it just stops holding the port.
    ///
    /// The runner does the closing, so this waits for it. Returning before the
    /// port was actually closed would hand the caller a device that is still
    /// held, which is the one thing this exists to avoid. Reports whether it
    /// let go in time.
    pub async fn release(&self) -> bool {
        self.released.store(true, Ordering::Relaxed);
        for _ in 0..RELEASE_TICKS {
            if !self.connected() {
                return true;
            }
            tokio::time::sleep(RELEASE_POLL).await;
        }
        false
    }

    /// Take the device back.
    pub fn hold(&self) {
        self.released.store(false, Ordering::Relaxed);
    }

    pub fn released(&self) -> bool {
        self.released.load(Ordering::Relaxed)
    }

    pub fn bridge_port(&self) -> Option<u16> {
        self.bridge
    }

    /// Queue bytes exactly as given, with no line ending added. This is the
    /// raw bridge path, where the client is already speaking the board's own
    /// protocol and nothing may be inserted into it.
    pub fn queue_raw(&self, bytes: Vec<u8>) -> Result<(), String> {
        let echo = String::from_utf8_lossy(&bytes).into_owned();
        self.queue(bytes, echo, Origin::Bridge)
    }

    pub fn baud(&self) -> u32 {
        self.baud
    }

    pub fn total(&self) -> u64 {
        self.ring.lock().unwrap().total()
    }

    /// Bytes received since `cursor`, or the whole retained buffer if `None`,
    /// with the new cursor to pass next time.
    pub fn read(&self, cursor: Option<u64>) -> (String, u64) {
        let ring = self.ring.lock().unwrap();
        let start = cursor.unwrap_or(ring.base());
        let (abs, hay) = ring.slice_from(start);
        (String::from_utf8_lossy(hay).into_owned(), abs + hay.len() as u64)
    }

    pub fn snapshot(&self, lines: usize) -> String {
        self.ring.lock().unwrap().tail_lines(lines)
    }

    pub fn log_info(&self) -> LogInfo {
        self.log.lock().unwrap().info()
    }

    /// Start a new log segment and report where it went.
    ///
    /// # Errors
    /// Returns an error if the new segment cannot be created.
    pub fn log_roll(&self, tag: Option<&str>) -> Result<LogInfo> {
        self.log.lock().unwrap().roll(tag)
    }

    /// This console's log segment files, newest first.
    ///
    /// # Errors
    /// Returns an error if the log directory cannot be read.
    pub fn log_segments(&self, days: Option<i64>) -> Result<Vec<PathBuf>> {
        self.log.lock().unwrap().segments(days)
    }

    /// Queue input without waiting for the write to land. This is the path a
    /// keystroke takes, where there is nobody to report a failure to.
    pub fn queue_line(&self, text: &str) -> Result<(), String> {
        let mut bytes = text.as_bytes().to_vec();
        bytes.extend_from_slice(&self.eol);
        self.queue(bytes, text.to_string(), Origin::Typed)
    }

    pub fn queue_ctrl(&self, ctrl: char) -> Result<(), String> {
        let byte = ctrl_byte(ctrl).ok_or_else(|| format!("no control byte for '{ctrl}'"))?;
        let echo = format!("Ctrl+{}", ctrl.to_ascii_uppercase());
        self.queue(vec![byte], echo, Origin::Key)
    }

    fn queue(&self, bytes: Vec<u8>, echo: String, origin: Origin) -> Result<(), String> {
        self.inject
            .send(Inject {
                bytes,
                echo,
                origin,
                resp: None,
            })
            .map_err(|_| "serial session closed".to_string())
    }

    /// Write `text` to the port, appending the console end-of-line when
    /// `newline`. Returns the cursor just before the write, so a following
    /// read or expect captures the echo and reply.
    ///
    /// # Errors
    /// Returns an error if the port is disconnected or the write fails.
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

    /// Returns the cursor before the write.
    ///
    /// # Errors
    /// Returns an error if `ctrl` has no control byte, or the write fails.
    pub async fn send_ctrl(&self, ctrl: char) -> Result<u64, String> {
        let byte = ctrl_byte(ctrl).ok_or_else(|| format!("no control byte for '{ctrl}'"))?;
        let cursor = self.total();
        let echo = format!("Ctrl+{}", ctrl.to_ascii_uppercase());
        self.inject_and_wait_as(vec![byte], echo, Origin::Key).await?;
        Ok(cursor)
    }

    async fn inject_and_wait(&self, bytes: Vec<u8>, echo: String) -> Result<(), String> {
        self.inject_and_wait_as(bytes, echo, Origin::Agent).await
    }

    async fn inject_and_wait_as(&self, bytes: Vec<u8>, echo: String, origin: Origin) -> Result<(), String> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.inject
            .send(Inject {
                bytes,
                echo,
                origin,
                resp: Some(resp_tx),
            })
            .map_err(|_| "serial session closed".to_string())?;
        match resp_rx.await {
            Ok(result) => result,
            Err(_) => Err("serial session closed".to_string()),
        }
    }

    /// Wait until `pattern` appears in received data, or `timeout_ms` elapses.
    /// Scans from `cursor` if given, else from the current end for new data
    /// only. Returns the text from the start point up to and including the
    /// match, or up to the end on timeout, plus the cursor to continue
    /// from.
    ///
    /// # Errors
    /// Returns an error if `pattern` is not a valid regular expression.
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
        // Each scan resumes near where the previous one ended instead of
        // rescanning everything from `start` for every received chunk.
        let mut scan_from = start;

        loop {
            // Register interest before scanning so a byte that arrives between
            // the scan and the wait cannot be missed.
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            {
                let ring = self.ring.lock().unwrap();
                let (scan_abs, scan_hay) = ring.slice_from(scan_from);
                if let Some(end) = matcher.find_end(scan_hay) {
                    let match_end = scan_abs + end as u64;
                    let (abs, hay) = ring.slice_from(start);
                    let data = &hay[..(match_end - abs) as usize];
                    return Ok(Expect {
                        matched:   true,
                        data:      String::from_utf8_lossy(data).into_owned(),
                        cursor:    match_end,
                        timed_out: false,
                    });
                }
                scan_from = matcher.resume_from(start, ring.total());
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

    // Returns how many viewers got the event. Zero is the normal state for a
    // console nobody is watching, so it is a count and not a failure.
    fn emit(&self, event: ConsoleEvent) -> usize {
        self.events.send(event).unwrap_or(0)
    }

    // A log write can fail on a full disk. That must not take the console down,
    // so it is reported and the console keeps running.
    fn record(&self, outcome: Result<()>) {
        if let Err(e) = outcome {
            eprintln!("smon: log write failed on {}: {e:#}", self.name());
        }
    }
}

/// The control byte for Ctrl+<c>. One shared mapping serves both the TUI
/// keystrokes and the MCP serial_send_ctrl tool, so they cannot drift apart.
pub fn ctrl_byte(c: char) -> Option<u8> {
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

#[cfg(test)]
mod tests {
    use std::{env, fs, sync::mpsc::channel};

    use super::*;
    use crate::ring::DEFAULT_RING_CAP;

    fn console(device: &str, label: Option<&str>) -> Arc<Console> {
        let dir = env::temp_dir().join(format!("smon-console-test-{}", sanitize_for_test(device)));
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        let log = ConsoleLog::open_in(dir, device, 30, None).unwrap();
        // The inject receiver is dropped, these tests never write to a port.
        Console::new(
            ConsoleSpec {
                device:   device.to_string(),
                label:    label.map(str::to_string),
                baud:     115_200,
                eol:      b"\r\n".to_vec(),
                ring_cap: DEFAULT_RING_CAP,
                bridge:   None,
            },
            log,
            channel().0,
        )
    }

    fn sanitize_for_test(s: &str) -> String {
        s.chars().filter(char::is_ascii_alphanumeric).collect()
    }

    #[test]
    fn snapshot_returns_last_lines() {
        let c = console("COM1", None);
        c.push_rx(b"one\r\ntwo\r\nthree\r\npartial");
        assert_eq!(c.snapshot(2), "three\npartial");
        assert_eq!(c.snapshot(10), "one\ntwo\nthree\npartial");
        assert_eq!(c.snapshot(0), "");
    }

    #[test]
    fn a_label_names_the_console_but_the_device_still_addresses_it() {
        let c = console("/dev/ttyUSB2", Some("board-a"));
        assert_eq!(c.name(), "board-a");
        assert!(c.matches("board-a"));
        assert!(c.matches("BOARD-A"));
        assert!(c.matches("/dev/ttyUSB2"));
        assert!(c.matches("ttyUSB2"));
        assert!(!c.matches("ttyUSB1"));
    }

    #[test]
    fn without_a_label_the_device_is_the_name() {
        let c = console("COM11", None);
        assert_eq!(c.name(), "COM11");
        assert!(c.matches("com11"));
        // COM1 must never address COM11.
        assert!(!c.matches("COM1"));
    }

    #[test]
    fn viewers_see_received_bytes_and_state_changes() {
        let c = console("COM7", None);
        let mut viewer = c.subscribe();
        c.push_rx(b"hello");
        c.set_connected(false);

        assert!(matches!(viewer.try_recv(), Ok(ConsoleEvent::Rx(b)) if b == b"hello"));
        assert!(matches!(viewer.try_recv(), Ok(ConsoleEvent::Connected)));
        assert!(!c.connected());
    }

    #[test]
    fn ctrl_byte_maps_letters_and_symbols() {
        assert_eq!(ctrl_byte('c'), Some(3));
        assert_eq!(ctrl_byte('C'), Some(3));
        assert_eq!(ctrl_byte('['), Some(0x1b));
        assert_eq!(ctrl_byte('1'), None);
    }
}
