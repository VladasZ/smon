//! The interactive session: keyboard in, console events out to the screen.
//!
//! The session owns no device. It watches a console and queues input on it, the
//! same way an agent does, so what a person types and what a client injects
//! both appear here in the order the port saw them.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::DefaultTerminal;

use crate::{
    attached::{Attached, Polled},
    clipboard::Clipboard,
    config::Config,
    console::ConsoleEvent,
    control::Control,
    draw::draw,
    ui::Ui,
};

/// # Errors
/// Returns an error if the screen cannot be drawn.
pub fn run(terminal: &mut DefaultTerminal, attached: &mut dyn Attached, control: &Control) -> Result<()> {
    let mut config = Config::load();
    let mut ui = Ui {
        history: config.history.clone(),
        ..Default::default()
    };
    // The console has been running since before this session attached, so start
    // from what it already heard rather than an empty screen.
    ui.push_rx(attached.backlog().as_bytes());
    let title = attached.title();
    let mut clipboard = Clipboard::new();
    let mut dirty = true;

    loop {
        // An update replaced the binary, so this session is the last thing
        // still running the old one.
        if control.stopping() {
            return Ok(());
        }

        loop {
            match attached.poll() {
                Polled::Event(ConsoleEvent::Rx(bytes)) => ui.push_rx(&bytes),
                Polled::Event(ConsoleEvent::Echo { origin, text }) => ui.push_echo(origin, &text),
                Polled::Event(ConsoleEvent::System(text)) => ui.push_system(&text),
                // The title carries the state, so the redraw below is the whole
                // response to a connection change.
                Polled::Event(ConsoleEvent::Connected) => {}
                Polled::Idle => break,
                Polled::Closed => return Ok(()),
            }
            dirty = true;
        }

        if ui.flash_expired() {
            dirty = true;
        }

        // Redraw only when state changed. An unconditional draw on every 16ms
        // tick re-wrapped the scrollback at 60 fps and burned most of a core.
        if dirty {
            draw(terminal, &mut ui, &title, attached.connected())?;
            dirty = false;
        }

        if !event::poll(POLL)? {
            continue;
        }
        let key = match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => key,
            Event::Mouse(mouse) => {
                match mouse.kind {
                    MouseEventKind::ScrollUp => ui.scroll_up(WHEEL_LINES),
                    MouseEventKind::ScrollDown => ui.scroll_down(WHEEL_LINES),
                    MouseEventKind::Down(MouseButton::Left) => {
                        if let Some(text) = ui.click(mouse.column, mouse.row)
                            && let Err(e) = clipboard.copy(&text)
                        {
                            ui.push_system(&e);
                        }
                    }
                    _ => continue,
                }
                dirty = true;
                continue;
            }
            // a resize invalidates the frame even though no state changed.
            Event::Resize(_, _) => {
                dirty = true;
                continue;
            }
            _ => continue,
        };
        if is_quit(&key) {
            return Ok(());
        }
        dirty = true;

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Ctrl+F belongs to the search, so it is the one control combo that
        // never reaches the device.
        if ctrl && matches!(key.code, KeyCode::Char('f') | KeyCode::Char('F')) {
            if ui.search.is_some() {
                ui.close_search();
            } else {
                ui.open_search();
            }
            continue;
        }
        if ui.search.is_some() {
            search_key(&mut ui, &key);
            continue;
        }
        match key.code {
            KeyCode::Char(c) if ctrl => {
                if let Err(e) = attached.send_ctrl(c) {
                    ui.push_system(&e);
                }
            }
            KeyCode::Char(c) => ui.insert(c),
            KeyCode::Tab => ui.accept_suggestion(),
            KeyCode::Enter => {
                let text = ui.take_input();
                if !text.is_empty() {
                    config.record_command(&text);
                    // A failed save is worth a note, not the end of the session.
                    if let Err(e) = config.save() {
                        ui.push_system(&format!("config save failed: {e}"));
                    }
                    ui.history = config.history.clone();
                }
                if let Err(e) = attached.send_line(&text) {
                    ui.push_system(&e);
                }
            }
            KeyCode::Backspace => ui.backspace(),
            KeyCode::Delete => ui.delete(),
            KeyCode::Left => ui.cursor = ui.cursor.saturating_sub(1),
            KeyCode::Right if ui.cursor == ui.input.len() && ui.suggestion.is_some() => {
                ui.accept_suggestion()
            }
            KeyCode::Right => ui.cursor = (ui.cursor + 1).min(ui.input.len()),
            KeyCode::Home => ui.cursor = 0,
            KeyCode::End => ui.cursor = ui.input.len(),
            KeyCode::Up => ui.history_prev(),
            KeyCode::Down => ui.history_next(),
            KeyCode::Esc => ui.clear_input(),
            _ => {}
        }
    }
}

const POLL: Duration = Duration::from_millis(16);
// One wheel notch moves this many lines, matching common terminal behavior.
const WHEEL_LINES: usize = 3;

fn is_quit(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
}

// Keys while the search prompt is open. Control combos are deliberately inert
// here, so a stray Ctrl+C cannot reach the device mid-search.
fn search_key(ui: &mut Ui, key: &KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return;
    }
    match key.code {
        KeyCode::Esc => ui.close_search(),
        KeyCode::Enter | KeyCode::Up => ui.search_older(),
        KeyCode::Down => ui.search_newer(),
        KeyCode::Backspace => ui.search_backspace(),
        KeyCode::Char(c) => ui.search_insert(c),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyEvent;

    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn ctrl_q_quits() {
        assert!(is_quit(&key(KeyCode::Char('q'), KeyModifiers::CONTROL)));
        assert!(is_quit(&key(KeyCode::Char('Q'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn plain_q_does_not_quit() {
        assert!(!is_quit(&key(KeyCode::Char('q'), KeyModifiers::NONE)));
    }

    #[test]
    fn other_control_keys_pass_through_not_quit() {
        assert!(!is_quit(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        assert!(!is_quit(&key(KeyCode::Char(']'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn search_keys_edit_the_query_and_esc_closes() {
        let mut ui = Ui::default();
        ui.open_search();
        search_key(&mut ui, &key(KeyCode::Char('o'), KeyModifiers::NONE));
        search_key(&mut ui, &key(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(ui.search_query().as_deref(), Some("ok"));
        search_key(&mut ui, &key(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(ui.search_query().as_deref(), Some("o"));
        search_key(&mut ui, &key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(ui.search.is_none());
    }

    // A control combo typed mid-search must not land in the query and must not
    // go anywhere else either.
    #[test]
    fn control_combos_are_inert_while_searching() {
        let mut ui = Ui::default();
        ui.open_search();
        search_key(&mut ui, &key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(ui.search_query(), None);
        assert!(ui.search.is_some());
    }
}
