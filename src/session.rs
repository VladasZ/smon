use std::{
    io::{Read, Write},
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    DefaultTerminal,
    layout::{Constraint, Direction, Layout},
    text::Line,
    widgets::{Paragraph, Wrap},
};
use serialport::SerialPort;

pub fn run(terminal: &mut DefaultTerminal, port_name: &str, baud: u32) -> Result<()> {
    let mut port = serialport::new(port_name, baud)
        .timeout(Duration::from_millis(50))
        .open()
        .with_context(|| format!("opening {port_name} @ {baud}"))?;

    let reader = port
        .try_clone()
        .context("cloning serial port for reader thread")?;

    let (rx_tx, rx_rx) = mpsc::channel::<Vec<u8>>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let reader_thread = thread::spawn(move || reader_loop(reader, rx_tx, stop_rx));

    let mut buffer = String::new();
    let mut error: Option<String> = None;

    loop {
        loop {
            match rx_rx.try_recv() {
                Ok(bytes) => buffer.push_str(&String::from_utf8_lossy(&bytes)),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    error = Some("reader thread disconnected".to_string());
                    break;
                }
            }
        }

        terminal.draw(|frame| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(0), Constraint::Length(1)])
                .split(frame.area());

            let pane_height = chunks[0].height as usize;
            let line_count = buffer.matches('\n').count() + 1;
            let scroll = line_count.saturating_sub(pane_height) as u16;

            let text = Paragraph::new(buffer.as_str())
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0));
            frame.render_widget(text, chunks[0]);

            let status = Line::from(format!(" {port_name} @ {baud}  |  Ctrl+Q to quit"));
            frame.render_widget(Paragraph::new(status), chunks[1]);
        })?;

        if error.is_some() {
            break;
        }

        if !event::poll(Duration::from_millis(16))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if is_quit(&key) {
            break;
        }

        if let Some(bytes) = key_to_bytes(key.code, key.modifiers)
            && let Err(e) = port.write_all(&bytes)
        {
            error = Some(format!("write error: {e}"));
            break;
        }
    }

    let _ = stop_tx.send(());
    let _ = reader_thread.join();

    if let Some(e) = error {
        anyhow::bail!(e);
    }
    Ok(())
}

fn reader_loop(mut port: Box<dyn SerialPort>, tx: Sender<Vec<u8>>, stop_rx: Receiver<()>) {
    let mut buf = [0u8; 4096];
    loop {
        if matches!(
            stop_rx.try_recv(),
            Ok(()) | Err(mpsc::TryRecvError::Disconnected)
        ) {
            return;
        }
        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => return,
        }
    }
}

fn is_quit(key: &KeyEvent) -> bool {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }
    // Ctrl+] emits 0x1D, which crossterm reports as Char('5'). That key is missing on some
    // keyboard layouts, so Ctrl+Q is the layout-independent quit.
    matches!(
        key.code,
        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Char(']') | KeyCode::Char('5')
    )
}

fn key_to_bytes(code: KeyCode, mods: KeyModifiers) -> Option<Vec<u8>> {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    Some(match code {
        KeyCode::Char(c) if ctrl => {
            let lower = c.to_ascii_lowercase();
            match lower {
                'a'..='z' => vec![(lower as u8) - b'a' + 1],
                '@' => vec![0],
                '[' => vec![0x1b],
                '\\' => vec![0x1c],
                '^' => vec![0x1e],
                '_' => vec![0x1f],
                _ => return None,
            }
        }
        KeyCode::Char(c) => {
            let mut buf = [0u8; 4];
            c.encode_utf8(&mut buf).as_bytes().to_vec()
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => vec![0x1b, b'[', b'A'],
        KeyCode::Down => vec![0x1b, b'[', b'B'],
        KeyCode::Right => vec![0x1b, b'[', b'C'],
        KeyCode::Left => vec![0x1b, b'[', b'D'],
        KeyCode::Home => vec![0x1b, b'[', b'H'],
        KeyCode::End => vec![0x1b, b'[', b'F'],
        KeyCode::PageUp => vec![0x1b, b'[', b'5', b'~'],
        KeyCode::PageDown => vec![0x1b, b'[', b'6', b'~'],
        KeyCode::Delete => vec![0x1b, b'[', b'3', b'~'],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
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
    fn ctrl_bracket_and_its_control_byte_quit() {
        assert!(is_quit(&key(KeyCode::Char(']'), KeyModifiers::CONTROL)));
        assert!(is_quit(&key(KeyCode::Char('5'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn plain_q_does_not_quit() {
        assert!(!is_quit(&key(KeyCode::Char('q'), KeyModifiers::NONE)));
    }

    #[test]
    fn unrelated_control_keys_do_not_quit() {
        assert!(!is_quit(&key(KeyCode::Char('a'), KeyModifiers::CONTROL)));
        assert!(!is_quit(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)));
    }
}
