//! What the session drives: a console it is attached to, local or remote.
//!
//! The session never opens a device and never speaks a protocol. It polls for
//! events and queues input, so the same loop serves a console this process owns
//! and one held by a daemon on another machine.

use std::sync::Arc;

use tokio::sync::broadcast::{Receiver, error::TryRecvError};

use crate::console::{Console, ConsoleEvent};

pub enum Polled {
    Event(ConsoleEvent),
    Idle,
    /// The console is gone and will send nothing more.
    Closed,
}

pub trait Attached {
    /// Shown in the title bar.
    fn title(&self) -> String;
    fn connected(&self) -> bool;
    /// What the console said before this viewer arrived.
    fn backlog(&self) -> String;
    fn poll(&mut self) -> Polled;
    /// # Errors
    /// Returns a message to show when the input cannot be queued.
    fn send_line(&mut self, text: &str) -> Result<(), String>;
    /// # Errors
    /// Returns a message to show when the input cannot be queued.
    fn send_ctrl(&mut self, ctrl: char) -> Result<(), String>;
}

/// A console this process owns.
pub struct Local {
    console: Arc<Console>,
    events:  Receiver<ConsoleEvent>,
    backlog: String,
}

impl Local {
    pub fn new(console: Arc<Console>, backlog_lines: usize) -> Local {
        // Subscribed before the backlog is taken, so anything arriving in
        // between is queued rather than lost in the gap.
        let events = console.subscribe();
        let backlog = console.snapshot(backlog_lines);
        Local {
            console,
            events,
            backlog,
        }
    }
}

impl Attached for Local {
    fn title(&self) -> String {
        format!("{} @ {}", self.console.name(), self.console.baud())
    }

    fn connected(&self) -> bool {
        self.console.connected()
    }

    fn backlog(&self) -> String {
        self.backlog.clone()
    }

    fn poll(&mut self) -> Polled {
        match self.events.try_recv() {
            Ok(event) => Polled::Event(event),
            Err(TryRecvError::Empty) => Polled::Idle,
            // A burst faster than this screen could take. Saying so is better
            // than a silent hole in the scrollback.
            Err(TryRecvError::Lagged(missed)) => Polled::Event(ConsoleEvent::System(format!(
                "{missed} lines dropped, screen fell behind"
            ))),
            Err(TryRecvError::Closed) => Polled::Closed,
        }
    }

    fn send_line(&mut self, text: &str) -> Result<(), String> {
        self.console.queue_line(text)
    }

    fn send_ctrl(&mut self, ctrl: char) -> Result<(), String> {
        self.console.queue_ctrl(ctrl)
    }
}
