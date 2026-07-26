//! The websocket a viewer attaches to.
//!
//! One socket per viewer, carrying console output out and input back. Several
//! viewers can hold the same console at once and each sees the others' input,
//! because everything goes through the console's own event stream.

use std::sync::Arc;

use axum::{
    extract::{
        Path as UrlPath, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use tokio::sync::broadcast::error::RecvError;

use crate::{
    console::{Console, ConsoleEvent},
    frame::{ToConsole, ToViewer},
    registry::Registry,
};

// What a viewer is shown of the past when it attaches. Enough to fill any
// terminal without replaying a whole day.
const BACKLOG_LINES: usize = 2000;

pub async fn attach(
    upgrade: WebSocketUpgrade,
    UrlPath(name): UrlPath<String>,
    State(registry): State<Arc<Registry>>,
) -> Response {
    match registry.resolve(Some(&name)) {
        Ok(console) => upgrade.on_upgrade(move |socket| pump(socket, console)),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

async fn pump(mut socket: WebSocket, console: Arc<Console>) {
    // Subscribed before the backlog is taken, so anything arriving in between
    // is queued rather than lost in the gap.
    let mut events = console.subscribe();
    let hello = ToViewer::Hello {
        console:   console.name().to_string(),
        baud:      console.baud(),
        connected: console.connected(),
        backlog:   console.snapshot(BACKLOG_LINES),
    };
    if !send(&mut socket, &hello).await {
        return;
    }

    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => {
                    if !send(&mut socket, &frame_of(&console, event)).await {
                        return;
                    }
                }
                // The viewer could not keep up. Saying so beats a silent hole.
                Err(RecvError::Lagged(missed)) => {
                    let note = ToViewer::System {
                        text: format!("{missed} lines dropped, viewer fell behind"),
                    };
                    if !send(&mut socket, &note).await {
                        return;
                    }
                }
                Err(RecvError::Closed) => return,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if !apply(&console, &text) {
                        return;
                    }
                }
                Some(Ok(Message::Close(_))) | None => return,
                Some(Ok(_)) => {}
                Some(Err(_)) => return,
            },
        }
    }
}

// Whether the console is still there to take input. A bad frame is reported to
// the viewer and the socket stays up, since one malformed message is no reason
// to drop a console someone is watching.
fn apply(console: &Arc<Console>, text: &str) -> bool {
    let outcome = match serde_json::from_str::<ToConsole>(text) {
        Ok(ToConsole::Line { text }) => console.queue_line(&text),
        Ok(ToConsole::Ctrl { ctrl }) => console.queue_ctrl(ctrl),
        Err(e) => {
            console.push_system(&format!("viewer sent a frame that made no sense: {e}"));
            return true;
        }
    };
    match outcome {
        Ok(()) => true,
        Err(e) => {
            console.push_system(&e);
            true
        }
    }
}

// The console carries the connection state, the event only says it changed, so
// the frame is filled in from the console itself.
fn frame_of(console: &Arc<Console>, event: ConsoleEvent) -> ToViewer {
    match event {
        ConsoleEvent::Rx(bytes) => ToViewer::Rx {
            data: String::from_utf8_lossy(&bytes).into_owned(),
        },
        ConsoleEvent::Echo { origin, text } => ToViewer::Echo { origin, text },
        ConsoleEvent::System(text) => ToViewer::System { text },
        ConsoleEvent::Connected => ToViewer::Connected {
            connected: console.connected(),
        },
    }
}

// Whether the viewer is still connected.
async fn send(socket: &mut WebSocket, frame: &ToViewer) -> bool {
    let Ok(text) = serde_json::to_string(frame) else {
        return true;
    };
    socket.send(Message::Text(text.into())).await.is_ok()
}
