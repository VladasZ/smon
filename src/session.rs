use std::{
    io::{Read, Write},
    mem::take,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use nucleo_matcher::{Config as FuzzyConfig, Matcher, Utf32String};
use ratatui::{
    DefaultTerminal, Terminal,
    backend::Backend,
    layout::{Constraint, Direction, Layout, Position},
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};
use serialport::SerialPort;

use crate::{config, log::SessionLog, probe::Lock};

const MAX_LINES: usize = 5000;

enum Origin {
    Rx,
    Tx,
}

struct OutLine {
    origin: Origin,
    text: String,
}

#[derive(Default)]
struct Ui {
    lines: Vec<OutLine>,
    rx_partial: String,
    input: Vec<char>,
    cursor: usize,
    history: Vec<String>,
    hist_pos: Option<usize>,
    // best fuzzy match from history for the current input, shown as ghost text.
    suggestion: Option<String>,
}

impl Ui {
    fn push_rx(&mut self, bytes: &[u8]) {
        for ch in String::from_utf8_lossy(bytes).chars() {
            match ch {
                '\n' => self.end_line(),
                // carriage returns and other control bytes would corrupt the on-screen lines;
                // the faithful escaped form is kept in the log file, not here.
                '\r' => {}
                c if c.is_control() && c != '\t' => {}
                c => self.rx_partial.push(c),
            }
        }
        self.cap_lines();
    }

    fn end_line(&mut self) {
        self.lines.push(OutLine {
            origin: Origin::Rx,
            text: take(&mut self.rx_partial),
        });
    }

    fn echo_tx(&mut self, text: &str) {
        if !self.rx_partial.is_empty() {
            self.end_line();
        }
        self.lines.push(OutLine {
            origin: Origin::Tx,
            text: text.to_string(),
        });
        self.cap_lines();
    }

    fn cap_lines(&mut self) {
        if self.lines.len() > MAX_LINES {
            let drop = self.lines.len() - MAX_LINES;
            self.lines.drain(..drop);
        }
    }

    fn take_input(&mut self) -> String {
        self.cursor = 0;
        self.hist_pos = None;
        let text: String = take(&mut self.input).into_iter().collect();
        self.update_suggestion();
        text
    }

    fn insert(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += 1;
        self.update_suggestion();
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.input.remove(self.cursor);
            self.update_suggestion();
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.input.len() {
            self.input.remove(self.cursor);
            self.update_suggestion();
        }
    }

    fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
        self.hist_pos = None;
        self.update_suggestion();
    }

    fn set_input(&mut self, text: &str) {
        self.input = text.chars().collect();
        self.cursor = self.input.len();
        self.update_suggestion();
    }

    fn accept_suggestion(&mut self) {
        if let Some(suggestion) = self.suggestion.clone() {
            self.set_input(&suggestion);
        }
    }

    // The freshest history entry that fuzzy-matches the input and isn't already exactly it.
    fn update_suggestion(&mut self) {
        let input: String = self.input.iter().collect();
        if input.is_empty() {
            self.suggestion = None;
            return;
        }
        let mut matcher = Matcher::new(FuzzyConfig::DEFAULT);
        let needle = Utf32String::from(input.as_str());
        let mut best: Option<(u16, &String)> = None;
        for cmd in &self.history {
            if *cmd == input {
                continue;
            }
            let hay = Utf32String::from(cmd.as_str());
            if let Some(score) = matcher.fuzzy_match(hay.slice(..), needle.slice(..))
                && best.is_none_or(|(b, _)| score >= b)
            {
                best = Some((score, cmd));
            }
        }
        self.suggestion = best.map(|(_, cmd)| cmd.clone());
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let pos = match self.hist_pos {
            None => self.history.len() - 1,
            Some(p) => p.saturating_sub(1),
        };
        let text = self.history[pos].clone();
        self.set_input(&text);
        self.hist_pos = Some(pos);
    }

    fn history_next(&mut self) {
        let Some(pos) = self.hist_pos else {
            return;
        };
        if pos + 1 < self.history.len() {
            let text = self.history[pos + 1].clone();
            self.set_input(&text);
            self.hist_pos = Some(pos + 1);
        } else {
            self.clear_input();
        }
    }
}

pub fn run(
    terminal: &mut DefaultTerminal,
    port_name: &str,
    baud: u32,
    eol: &[u8],
) -> Result<()> {
    // Hold the probe lock across the open so another instance's port probe can't briefly grab
    // the port at the same instant and make this open fail with access-denied.
    let opened = {
        let lock = Lock::acquire();
        let result = serialport::new(port_name, baud)
            .timeout(Duration::from_millis(50))
            .open();
        drop(lock);
        result
    };
    let mut port = opened.with_context(|| format!("opening {port_name} @ {baud}"))?;

    let mut log = SessionLog::create(port_name)?;

    let reader = port
        .try_clone()
        .context("cloning serial port for reader thread")?;

    let (rx_tx, rx_rx) = mpsc::channel::<Vec<u8>>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let reader_thread = thread::spawn(move || reader_loop(reader, rx_tx, stop_rx));

    let mut config = config::Config::load();
    let mut ui = Ui {
        history: config.history.clone(),
        ..Default::default()
    };
    let mut error: Option<String> = None;

    loop {
        loop {
            match rx_rx.try_recv() {
                Ok(bytes) => {
                    log.rx(&bytes)?;
                    ui.push_rx(&bytes);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    error = Some("reader thread disconnected".to_string());
                    break;
                }
            }
        }

        draw(terminal, &ui, port_name, baud)?;

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

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char(c) if ctrl => {
                if let Some(bytes) = ctrl_bytes(c) {
                    log.tx_key(&format!("Ctrl+{}", c.to_ascii_uppercase()))?;
                    if let Err(e) = port.write_all(&bytes) {
                        error = Some(format!("write error: {e}"));
                        break;
                    }
                }
            }
            KeyCode::Char(c) => ui.insert(c),
            KeyCode::Tab => ui.accept_suggestion(),
            KeyCode::Enter => {
                let text = ui.take_input();
                if !text.is_empty() {
                    config.record_command(&text);
                    config.save()?;
                    ui.history = config.history.clone();
                }
                ui.echo_tx(&text);
                let mut bytes = text.into_bytes();
                bytes.extend_from_slice(eol);
                log.tx_line(&bytes)?;
                if let Err(e) = port.write_all(&bytes) {
                    error = Some(format!("write error: {e}"));
                    break;
                }
            }
            KeyCode::Backspace => ui.backspace(),
            KeyCode::Delete => ui.delete(),
            KeyCode::Left => ui.cursor = ui.cursor.saturating_sub(1),
            // at the end of the line, Right accepts the ghost suggestion; otherwise it moves.
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

    let _ = stop_tx.send(());
    let _ = reader_thread.join();

    if let Some(e) = error {
        anyhow::bail!(e);
    }
    Ok(())
}

fn draw<B: Backend>(terminal: &mut Terminal<B>, ui: &Ui, port_name: &str, baud: u32) -> Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let border = Style::new().fg(Color::DarkGray);

    terminal.draw(|frame| {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(frame.area());

        let out_area = chunks[0];
        let out_block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border)
            .title(Line::from(format!(" {port_name} @ {baud} ")))
            .title(Line::from(" ctrl+q: quit ").right_aligned());
        let out_inner = out_block.inner(out_area);
        frame.render_widget(out_block, out_area);

        let height = out_inner.height as usize;

        let mut rendered: Vec<Line> = Vec::with_capacity(ui.lines.len() + 1);
        for line in &ui.lines {
            let (text, style) = match line.origin {
                Origin::Rx => (line.text.clone(), Style::default()),
                Origin::Tx => (format!("> {}", line.text), Style::new().fg(Color::Cyan)),
            };
            rendered.push(Line::styled(text, style));
        }
        if !ui.rx_partial.is_empty() {
            rendered.push(Line::from(ui.rx_partial.clone()));
        }

        // Ask the paragraph itself how many rows it wraps to at this width. A hand-rolled
        // ceil(chars / width) estimate drifts from the word-wrapper, which drops whitespace at
        // wrap boundaries, so on wide terminals it over-counts and the scroll overshoots the
        // real bottom, leaving the newest lines stranded at the top with blank space below.
        let paragraph = Paragraph::new(Text::from(rendered)).wrap(Wrap { trim: false });
        let rows = paragraph.line_count(out_inner.width);
        let scroll = rows.saturating_sub(height).min(u16::MAX as usize) as u16;
        frame.render_widget(paragraph.scroll((scroll, 0)), out_inner);

        // Only show the scrollbar when content actually overflows. content_length is the number
        // of scroll positions (rows - viewport), NOT the total row count: the thumb reaches the
        // bottom only when position == content_length, and a paragraph's scroll maxes out at
        // rows - viewport. viewport_content_length sizes the thumb to the visible fraction.
        if rows > height {
            let mut sb_state = ScrollbarState::new(rows - height)
                .viewport_content_length(height)
                .position(scroll as usize);
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(None)
                    .end_symbol(None),
                out_area,
                &mut sb_state,
            );
        }

        let in_area = chunks[1];
        let in_block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border);
        let in_inner = in_block.inner(in_area);
        frame.render_widget(in_block, in_area);

        let avail = in_inner.width.max(1) as usize;
        let typed: String = ui.input.iter().collect();
        let typed_width = ui.input.len();

        let ghost = ui
            .suggestion
            .as_ref()
            .map(|s| format!("{s} ⇥"))
            .filter(|g| typed_width + 1 + g.chars().count() <= avail);

        if let Some(ghost) = ghost {
            let pad = avail - typed_width - ghost.chars().count();
            let line = Line::from(vec![
                Span::raw(typed),
                Span::raw(" ".repeat(pad)),
                Span::styled(ghost, Style::new().fg(Color::DarkGray)),
            ]);
            frame.render_widget(Paragraph::new(line), in_inner);
            frame.set_cursor_position(Position {
                x: in_inner.x + ui.cursor as u16,
                y: in_inner.y,
            });
        } else {
            let scroll_x = ui.cursor.saturating_sub(avail.saturating_sub(1));
            frame.render_widget(Paragraph::new(typed).scroll((0, scroll_x as u16)), in_inner);
            frame.set_cursor_position(Position {
                x: in_inner.x + (ui.cursor - scroll_x) as u16,
                y: in_inner.y,
            });
        }
    })?;
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
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
}

fn ctrl_bytes(c: char) -> Option<Vec<u8>> {
    let lower = c.to_ascii_lowercase();
    Some(match lower {
        'a'..='z' => vec![(lower as u8) - b'a' + 1],
        '@' => vec![0],
        '[' => vec![0x1b],
        '\\' => vec![0x1c],
        ']' => vec![0x1d],
        '^' => vec![0x1e],
        '_' => vec![0x1f],
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
    fn plain_q_does_not_quit() {
        assert!(!is_quit(&key(KeyCode::Char('q'), KeyModifiers::NONE)));
    }

    #[test]
    fn other_control_keys_pass_through_not_quit() {
        assert!(!is_quit(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        assert!(!is_quit(&key(KeyCode::Char(']'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn ctrl_c_maps_to_interrupt_byte() {
        assert_eq!(ctrl_bytes('c'), Some(vec![3]));
        assert_eq!(ctrl_bytes('['), Some(vec![0x1b]));
        assert_eq!(ctrl_bytes('1'), None);
    }

    #[test]
    fn history_recall_walks_back_and_forward() {
        let mut ui = Ui::default();
        ui.history = vec!["first".into(), "second".into()];

        ui.history_prev();
        assert_eq!(ui.input.iter().collect::<String>(), "second");
        ui.history_prev();
        assert_eq!(ui.input.iter().collect::<String>(), "first");
        ui.history_next();
        assert_eq!(ui.input.iter().collect::<String>(), "second");
        ui.history_next();
        assert_eq!(ui.input.iter().collect::<String>(), "");
    }

    #[test]
    fn rx_splits_on_newline_and_strips_cr() {
        let mut ui = Ui::default();
        ui.push_rx(b"hello\r\nwor");
        assert_eq!(ui.lines.len(), 1);
        assert_eq!(ui.lines[0].text, "hello");
        assert_eq!(ui.rx_partial, "wor");
    }

    #[test]
    fn suggestion_fuzzy_matches_history() {
        let mut ui = Ui::default();
        ui.history = vec!["get_status".into(), "reboot".into(), "get_temp".into()];
        ui.set_input("gst");
        assert_eq!(ui.suggestion.as_deref(), Some("get_status"));
    }

    #[test]
    fn suggestion_skips_exact_input_and_empty() {
        let mut ui = Ui::default();
        ui.history = vec!["reboot".into()];
        ui.set_input("reboot");
        assert_eq!(ui.suggestion, None);
        ui.clear_input();
        assert_eq!(ui.suggestion, None);
    }

    // A real captured device line. At an inner width of 118 the word-wrapper packs it into 2
    // rows, but a naive ceil(chars / width) count says 3 because the 237th char is a trailing
    // space that the wrapper drops at the wrap boundary. Feeding many of these makes a naive
    // row total overshoot the real wrapped height, which used to push the log off the top of
    // the pane and leave the bottom blank. Keep the trailing space, it is the whole point.
    const OVERCOUNT_LINE: &str = "D I P1 0x00F 0x00000 26-07-01~05:40:42.222+00:00~#  PR03 S30 RestartManager ErrorlogReceiverRepositoryElement.cpp Line 77 : ErrorlogReceiverRepositoryElement::initializeAfterAllRepositoryElements() called, initializing errorlogReceiver~ ";

    #[test]
    fn newest_lines_pinned_to_bottom_when_overflowing() {
        use ratatui::{Terminal, backend::TestBackend};

        assert_eq!(OVERCOUNT_LINE.chars().count(), 237, "trailing space lost");

        let (width, height) = (120u16, 24u16);
        let mut ui = Ui::default();
        for _ in 0..50 {
            ui.push_rx(OVERCOUNT_LINE.as_bytes());
            ui.push_rx(b"\n");
        }

        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        draw(&mut terminal, &ui, "COM11", 115200).unwrap();
        let buf = terminal.backend().buffer();

        // The 3-row input box sits at the bottom, with the output box's own bottom border just
        // above it, so the last text row is height - 3 - 1 - 1 (0-indexed).
        let last_text_row = height - 3 - 1 - 1;
        let row: String = (1..width - 1)
            .map(|x| buf.cell((x, last_text_row)).unwrap().symbol())
            .collect();
        assert!(
            !row.trim().is_empty(),
            "bottom log row is blank -- content over-scrolled and left empty space below it"
        );
    }
}
