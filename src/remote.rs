//! Attaching to a console held by a daemon, over the attach socket.
//!
//! The socket is driven on its own thread with its own runtime. The session
//! stays synchronous and talks to it through plain channels, so nothing in the
//! TUI has to know a network is involved.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, TryRecvError, channel},
    },
    thread,
    time::Duration,
};

use anyhow::{Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_tungstenite::tungstenite::Message;

use crate::{
    attached::{Attached, Polled},
    console::ConsoleEvent,
    frame::{ToConsole, ToViewer},
};

// The first frame carries the console's name and its backlog, so the session
// has something to show before waiting on the board.
const HELLO_WAIT: Duration = Duration::from_secs(10);

pub struct Remote {
    title:     String,
    backlog:   String,
    connected: Arc<AtomicBool>,
    events:    Receiver<ConsoleEvent>,
    outgoing:  UnboundedSender<ToConsole>,
    closed:    bool,
}

/// Attach to `console` on the smon serving `addr`.
///
/// # Errors
/// Returns an error if the socket cannot be opened, the console is not there,
/// or the first frame does not arrive.
pub fn attach(addr: &str, console: &str) -> Result<Remote> {
    let url = format!("ws://{addr}/attach/{console}");
    let (events_tx, events_rx) = channel::<ConsoleEvent>();
    let (hello_tx, hello_rx) = channel::<Result<Hello, String>>();
    let (outgoing_tx, outgoing_rx) = unbounded_channel::<ToConsole>();
    let connected = Arc::new(AtomicBool::new(false));

    let thread_connected = Arc::clone(&connected);
    thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(e) => {
                report(&hello_tx, Err(format!("tokio runtime: {e}")));
                return;
            }
        };
        runtime.block_on(drive(
            url,
            &events_tx,
            &hello_tx,
            outgoing_rx,
            &thread_connected,
        ));
    });

    match hello_rx.recv_timeout(HELLO_WAIT) {
        Ok(Ok(hello)) => Ok(Remote {
            title: format!("{} @ {}", hello.console, hello.baud),
            backlog: hello.backlog,
            connected,
            events: events_rx,
            outgoing: outgoing_tx,
            closed: false,
        }),
        Ok(Err(e)) => Err(anyhow!("{e}")),
        Err(_) => Err(anyhow!("no answer from smon at {addr} within 10s")),
    }
}

struct Hello {
    console: String,
    baud:    u32,
    backlog: String,
}

// Whether anyone was still waiting for the first frame.
fn report(hello: &Sender<Result<Hello, String>>, outcome: Result<Hello, String>) -> bool {
    hello.send(outcome).is_ok()
}

async fn drive(
    url: String,
    events: &Sender<ConsoleEvent>,
    hello: &Sender<Result<Hello, String>>,
    mut outgoing: tokio::sync::mpsc::UnboundedReceiver<ToConsole>,
    connected: &AtomicBool,
) {
    let socket = match tokio_tungstenite::connect_async(&url).await {
        Ok((socket, _)) => socket,
        Err(e) => {
            report(hello, Err(format!("cannot attach at {url}: {e}")));
            return;
        }
    };
    let (mut sink, mut stream) = socket.split();
    let mut greeted = false;

    loop {
        tokio::select! {
            incoming = stream.next() => {
                let Some(Ok(Message::Text(text))) = incoming else {
                    // Either the socket closed or it carried something this
                    // does not speak. Both end the attachment.
                    if !greeted {
                        report(hello, Err("the console closed before saying hello".to_string()));
                    }
                    return;
                };
                let Ok(frame) = serde_json::from_str::<ToViewer>(&text) else {
                    continue;
                };
                match frame {
                    ToViewer::Hello { console, baud, connected: up, backlog } => {
                        connected.store(up, Ordering::Relaxed);
                        greeted = true;
                        if !report(hello, Ok(Hello { console, baud, backlog })) {
                            return;
                        }
                    }
                    ToViewer::Rx { data } => {
                        if events.send(ConsoleEvent::Rx(data.into_bytes())).is_err() {
                            return;
                        }
                    }
                    ToViewer::Echo { origin, text } => {
                        if events.send(ConsoleEvent::Echo { origin, text }).is_err() {
                            return;
                        }
                    }
                    ToViewer::System { text } => {
                        if events.send(ConsoleEvent::System(text)).is_err() {
                            return;
                        }
                    }
                    ToViewer::Connected { connected: up } => {
                        connected.store(up, Ordering::Relaxed);
                        if events.send(ConsoleEvent::Connected).is_err() {
                            return;
                        }
                    }
                }
            }
            queued = outgoing.recv() => {
                let Some(frame) = queued else {
                    return;
                };
                let Ok(text) = serde_json::to_string(&frame) else {
                    continue;
                };
                if sink.send(Message::Text(text.into())).await.is_err() {
                    return;
                }
            }
        }
    }
}

impl Attached for Remote {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    fn backlog(&self) -> String {
        self.backlog.clone()
    }

    fn poll(&mut self) -> Polled {
        match self.events.try_recv() {
            Ok(event) => Polled::Event(event),
            Err(TryRecvError::Empty) => Polled::Idle,
            Err(TryRecvError::Disconnected) => {
                // Said once. After that the session sees a plain closed console
                // rather than the same line on every tick.
                if self.closed {
                    return Polled::Closed;
                }
                self.closed = true;
                Polled::Event(ConsoleEvent::System(
                    "the console went away, press ctrl+q".to_string(),
                ))
            }
        }
    }

    fn send_line(&mut self, text: &str) -> Result<(), String> {
        self.queue(ToConsole::Line {
            text: text.to_string(),
        })
    }

    fn send_ctrl(&mut self, ctrl: char) -> Result<(), String> {
        self.queue(ToConsole::Ctrl { ctrl })
    }
}

impl Remote {
    fn queue(&self, frame: ToConsole) -> Result<(), String> {
        self.outgoing
            .send(frame)
            .map_err(|_| "not attached any more".to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::{env, fs, sync::mpsc, thread::sleep};


    use super::*;
    use crate::{console::{Console, ConsoleSpec}, control::{Control, Role}, log::ConsoleLog, mcp, registry::Registry, ring::DEFAULT_RING_CAP};

    // Both directions over a real socket against a real server. The console
    // here has no device, so the input it receives is read straight off the
    // inject queue the runner would normally drain.
    #[test]
    fn a_viewer_sees_output_and_its_input_reaches_the_console() {
        let dir = env::temp_dir().join("smon-remote-test");
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        let log = ConsoleLog::open_in(dir.clone(), "COMTEST", 0, None).unwrap();
        let (inject_tx, inject_rx) = std::sync::mpsc::channel();
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
            inject_tx,
        );
        console.set_connected(true);
        // Said before anyone attaches, so it can only arrive as backlog.
        console.push_rx(b"said before the viewer arrived\n");

        let (ready_tx, ready_rx) = mpsc::channel();
        let registry = Registry::new(vec![Arc::clone(&console)], 0);
        let control = Arc::new(Control::new(Role::Tui));
        let server = mcp::spawn("127.0.0.1:0".parse().unwrap(), registry, Arc::clone(&control), ready_tx);
        let addr = ready_rx.recv().unwrap().unwrap();

        let mut viewer = attach(&addr.to_string(), "under-test").unwrap();
        assert_eq!(viewer.title(), "under-test @ 9600");
        assert!(viewer.connected());
        assert!(
            viewer.backlog().contains("said before the viewer arrived"),
            "backlog was {:?}",
            viewer.backlog()
        );

        viewer.send_line("version").unwrap();
        let queued = inject_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(queued.bytes, b"version\r\n");
        assert_eq!(queued.echo, "version");

        viewer.send_ctrl('c').unwrap();
        let queued = inject_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(queued.bytes, vec![3]);

        // Output produced after the attach arrives as an event, not as backlog.
        console.push_rx(b"later output\n");
        assert!(
            wait_for_rx(&mut viewer, "later output"),
            "the viewer never saw output sent after it attached"
        );

        assert!(control.release(), "server thread ended early");
        sleep(Duration::from_millis(50));
        drop(server);
        fs::remove_dir_all(&dir).unwrap();
    }

    fn wait_for_rx(viewer: &mut Remote, wanted: &str) -> bool {
        for _ in 0..100 {
            match viewer.poll() {
                Polled::Event(ConsoleEvent::Rx(bytes)) => {
                    if String::from_utf8_lossy(&bytes).contains(wanted) {
                        return true;
                    }
                }
                Polled::Event(_) => {}
                Polled::Idle => sleep(Duration::from_millis(20)),
                Polled::Closed => return false,
            }
        }
        false
    }
}
