//! The thread that owns a serial device.
//!
//! It opens the port, feeds received bytes into the console, writes queued
//! input, and reconnects on its own when the device disappears. Everything else
//! in smon reads and writes a console through the state in [`Console`], never
//! the device itself, which is what lets a TUI and an agent share one port.

use std::{
    io::{self, Error, ErrorKind, Read},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use serialport::SerialPort;

use crate::{
    console::{Console, Inject},
    probe,
};

const RECONNECT_EVERY: Duration = Duration::from_secs(1);
// How long the loop waits on the input queue before looking at everything else.
// It bounds how late a disconnect is noticed, and input is never delayed by it.
const TICK: Duration = Duration::from_millis(100);
// A device asserting flow control stalls writes briefly, which is normal. Only
// a stall this long with no progress at all means a dead port.
const WRITE_STALL_LIMIT: Duration = Duration::from_secs(5);

/// `resp` reports the result to whoever queued the input. Keyboard writes have
/// no listener.
struct WriteReq {
    bytes: Vec<u8>,
    resp:  Option<Reply>,
}

type Reply = tokio::sync::oneshot::Sender<Result<(), String>>;

/// Dropped and rebuilt on every disconnect.
struct Connection {
    writer_tx: Sender<WriteReq>,
    stop_tx:   Sender<()>,
    reader:    JoinHandle<()>,
    writer:    JoinHandle<()>,
}

pub struct Runner {
    stop:   Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl Runner {
    /// Take over the device named by `console` and keep it open.
    ///
    /// With `require_open` the first open must succeed and its failure is
    /// returned, which is what the TUI needs so a bad pick fails straight back
    /// to the picker. The daemon passes false, so a console whose adapter is
    /// unplugged at boot is retried instead of taking the service down.
    ///
    /// # Errors
    /// Returns an error only when `require_open` is set and the port cannot be
    /// opened.
    pub fn start(console: Arc<Console>, injects: Receiver<Inject>, require_open: bool) -> Result<Runner> {
        let (deaths_tx, deaths_rx) = mpsc::channel::<String>();
        let mut conn = None;
        if require_open {
            conn = Some(connect(&console, &deaths_tx)?);
            console.set_connected(true);
        }

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            run(&console, injects, &deaths_tx, &deaths_rx, conn, &thread_stop);
        });
        Ok(Runner { stop, thread })
    }

    /// Close the port and wait for the thread to finish.
    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        match self.thread.join() {
            Ok(()) => {}
            Err(_) => eprintln!("smon: console thread panicked"),
        }
    }
}

fn run(
    console: &Arc<Console>,
    injects: Receiver<Inject>,
    deaths_tx: &Sender<String>,
    deaths_rx: &Receiver<String>,
    mut conn: Option<Connection>,
    stop: &AtomicBool,
) {
    // None means nothing has been tried yet, so a console that starts
    // disconnected attempts its first open straight away.
    let mut last_attempt: Option<Instant> = None;
    let mut opened_before = conn.is_some();
    loop {
        match injects.recv_timeout(TICK) {
            Ok(inject) => write(console, &conn, inject),
            Err(RecvTimeoutError::Timeout) => {}
            // Every sender is gone, so nothing can ever queue input again.
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }

        // Some other program was handed the device. Let go of it and stay off
        // until it is handed back.
        if console.released() {
            if let Some(c) = conn.take() {
                teardown(c);
                console.set_connected(false);
                console.push_system("released, another program has the device");
            }
            continue;
        }

        // Both port threads report fatal errors here. Only the first report
        // tears the connection down.
        while let Ok(reason) = deaths_rx.try_recv() {
            if let Some(c) = conn.take() {
                teardown(c);
                console.set_connected(false);
                console.push_system(&format!("{reason}, reconnecting"));
                last_attempt = Some(Instant::now());
            }
        }

        if conn.is_none()
            && !console.released()
            && last_attempt.is_none_or(|at| at.elapsed() >= RECONNECT_EVERY)
        {
            last_attempt = Some(Instant::now());
            match connect(console, deaths_tx) {
                Ok(c) => {
                    conn = Some(c);
                    console.set_connected(true);
                    console.push_system(if opened_before { "reconnected" } else { "connected" });
                    opened_before = true;
                }
                // Logging every failed attempt would add a line each second for
                // as long as the device stays unplugged, so retries are silent.
                Err(_) => continue,
            }
        }
    }

    if let Some(c) = conn {
        teardown(c);
        console.set_connected(false);
    }
}

// While disconnected the input is dropped rather than piling up for a dead
// port. A caller waiting on the result is told, and a keystroke gets a note in
// the scrollback instead.
fn write(console: &Arc<Console>, conn: &Option<Connection>, inject: Inject) {
    let Inject {
        bytes,
        echo,
        origin,
        resp,
    } = inject;

    let Some(c) = conn else {
        if !reply(resp, Err("port disconnected".to_string())) {
            console.push_system("port disconnected, input not sent");
        }
        return;
    };

    console.push_echo(origin, &bytes, &echo);
    if let Err(back) = c.writer_tx.send(WriteReq { bytes, resp }) {
        let WriteReq { resp, .. } = back.0;
        if !reply(resp, Err("port disconnected".to_string())) {
            console.push_system("port disconnected, input not sent");
        }
    }
}

// Whether anyone was waiting to hear the outcome.
fn reply(resp: Option<Reply>, outcome: Result<(), String>) -> bool {
    match resp {
        Some(tx) => tx.send(outcome).is_ok(),
        None => false,
    }
}

fn connect(console: &Arc<Console>, deaths: &Sender<String>) -> Result<Connection> {
    let name = console.device();
    let baud = console.baud();
    // The probe lock is held across the open, so another instance's port probe
    // can't briefly grab the port at the same instant and make this open fail
    // with access-denied.
    let opened = probe::hold(|| serialport::new(name, baud).timeout(Duration::from_millis(50)).open());
    let port = opened.with_context(|| format!("opening {name} @ {baud}"))?;
    let reader_port = port.try_clone().context("cloning serial port for reader thread")?;

    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (writer_tx, writer_rx) = mpsc::channel::<WriteReq>();

    let reader_console = Arc::clone(console);
    let reader_deaths = deaths.clone();
    let reader = thread::spawn(move || reader_loop(reader_port, &reader_console, &stop_rx, &reader_deaths));

    let writer_deaths = deaths.clone();
    let writer = thread::spawn(move || writer_loop(port, &writer_rx, &writer_deaths));

    Ok(Connection {
        writer_tx,
        stop_tx,
        reader,
        writer,
    })
}

// Stop both port threads and wait for them. The reader notices the stop signal
// within its read timeout. The writer exits when its queue sender is dropped.
fn teardown(conn: Connection) {
    let sent = conn.stop_tx.send(());
    drop(conn.writer_tx);
    let reader = conn.reader.join();
    let writer = conn.writer.join();
    if sent.is_err() || reader.is_err() || writer.is_err() {
        eprintln!("smon: a port thread ended badly");
    }
}

fn reader_loop(
    mut port: Box<dyn SerialPort>,
    console: &Arc<Console>,
    stop_rx: &Receiver<()>,
    deaths: &Sender<String>,
) {
    let mut buf = [0u8; 4096];
    loop {
        if matches!(stop_rx.try_recv(), Ok(()) | Err(mpsc::TryRecvError::Disconnected)) {
            return;
        }
        match port.read(&mut buf) {
            Ok(0) => {}
            // Straight into the console from here, so an expect() sees bytes
            // without waiting on any loop tick.
            Ok(n) => console.push_rx(&buf[..n]),
            Err(e) if e.kind() == ErrorKind::TimedOut => {}
            Err(e) => {
                report(deaths, format!("read error: {e}"));
                return;
            }
        }
    }
}

fn writer_loop(mut port: Box<dyn SerialPort>, reqs: &Receiver<WriteReq>, deaths: &Sender<String>) {
    while let Ok(req) = reqs.recv() {
        match write_with_retry(port.as_mut(), &req.bytes) {
            Ok(()) => {
                reply(req.resp, Ok(()));
            }
            Err(e) => {
                let msg = format!("write error: {e}");
                reply(req.resp, Err(msg.clone()));
                report(deaths, msg);
                return;
            }
        }
    }
}

// Whether the runner was still there to hear it. It is gone only when the whole
// console is being torn down, and then there is nothing left to tell.
fn report(deaths: &Sender<String>, reason: String) -> bool {
    deaths.send(reason).is_ok()
}

// Retry timed-out writes until the stall limit, resetting the clock whenever
// any bytes go through, so slow trickling progress is not mistaken for a dead
// port.
fn write_with_retry(port: &mut dyn SerialPort, bytes: &[u8]) -> io::Result<()> {
    let mut written = 0;
    let mut deadline = Instant::now() + WRITE_STALL_LIMIT;
    while written < bytes.len() {
        match port.write(&bytes[written..]) {
            Ok(0) => return Err(Error::new(ErrorKind::WriteZero, "wrote zero bytes")),
            Ok(n) => {
                written += n;
                deadline = Instant::now() + WRITE_STALL_LIMIT;
            }
            Err(e) if e.kind() == ErrorKind::TimedOut => {
                if Instant::now() >= deadline {
                    return Err(Error::new(ErrorKind::TimedOut, "write stalled"));
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
