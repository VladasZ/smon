//! The frames a viewer and a console exchange over the attach socket.
//!
//! One connection carries both directions. Everything the console hears goes
//! out as a frame, and input comes back the same way, so a viewer on another
//! machine sees exactly what a viewer on this one does.

use serde::{Deserialize, Serialize};

use crate::console::Origin;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToViewer {
    /// Always first. `backlog` is what the console already heard, so a viewer
    /// that attaches to a quiet board still sees where it got to.
    Hello {
        console:   String,
        baud:      u32,
        connected: bool,
        backlog:   String,
    },
    Rx {
        data: String,
    },
    Echo {
        origin: Origin,
        text:   String,
    },
    System {
        text: String,
    },
    Connected {
        connected: bool,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToConsole {
    Line { text: String },
    Ctrl { ctrl: char },
}

#[cfg(test)]
mod tests {
    use super::*;

    // The two ends are built from this one file, but they are separate
    // processes and can be different builds, so the tags are a contract.
    #[test]
    fn frames_carry_a_stable_tag() {
        let hello = serde_json::to_string(&ToViewer::Hello {
            console:   "left".to_string(),
            baud:      115_200,
            connected: true,
            backlog:   "boot ok".to_string(),
        })
        .unwrap();
        assert!(hello.contains(r#""type":"hello""#), "{hello}");

        let echo = serde_json::to_string(&ToViewer::Echo {
            origin: Origin::Agent,
            text:   "reboot".to_string(),
        })
        .unwrap();
        assert!(echo.contains(r#""type":"echo""#), "{echo}");
        assert!(echo.contains(r#""origin":"agent""#), "{echo}");

        let line = serde_json::to_string(&ToConsole::Line {
            text: "version".to_string(),
        })
        .unwrap();
        assert!(line.contains(r#""type":"line""#), "{line}");
    }

    #[test]
    fn frames_round_trip() {
        let sent = serde_json::to_string(&ToConsole::Ctrl { ctrl: 'c' }).unwrap();
        let back: ToConsole = serde_json::from_str(&sent).unwrap();
        assert!(matches!(back, ToConsole::Ctrl { ctrl: 'c' }));
    }
}
