use std::{
    collections::VecDeque,
    io::{self, Error, ErrorKind, Read},
    mem::take,
    net::SocketAddr,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
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
use tokio::sync::{mpsc::unbounded_channel, oneshot};

use crate::{
    config,
    log::SessionLog,
    mcp::{self, Inject, Shared, ctrl_byte},
    probe::Lock,
};

const MAX_LINES: usize = 5000;
const RECONNECT_EVERY: Duration = Duration::from_secs(1);
// A board asserting flow control can stall writes briefly, which is normal. Only
// a stall this long with no progress at all is treated as a dead port.
const WRITE_STALL_LIMIT: Duration = Duration::from_secs(5);

enum Origin {
    Rx,
    Tx,
    Agent,
    System,
}

struct OutLine {
    origin: Origin,
    text: String,
}

#[derive(Default)]
struct Ui {
    lines: VecDeque<OutLine>,
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
        self.lines.push_back(OutLine {
            origin: Origin::Rx,
            text: take(&mut self.rx_partial),
        });
    }

    fn echo_tx(&mut self, text: &str) {
        self.push_out(Origin::Tx, text);
    }

    // Injected input joins the same scrollback, so the person at the TUI sees
    // exactly what an MCP client did, in order with everything else.
    fn echo_agent(&mut self, text: &str) {
        self.push_out(Origin::Agent, text);
    }

    fn push_system(&mut self, text: &str) {
        self.push_out(Origin::System, text);
    }

    fn push_out(&mut self, origin: Origin, text: &str) {
        if !self.rx_partial.is_empty() {
            self.end_line();
        }
        self.lines.push_back(OutLine {
            origin,
            text: text.to_string(),
        });
        self.cap_lines();
    }

    fn cap_lines(&mut self) {
        while self.lines.len() > MAX_LINES {
            self.lines.pop_front();
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

/// Everything the session hears back from the port threads.
enum PortEvent {
    Rx(Vec<u8>),
    Dead(String),
}

/// A write queued for the writer thread. `resp` reports the result back to an
/// MCP client. Keyboard writes have no listener.
struct WriteReq {
    bytes: Vec<u8>,
    resp:  Option<oneshot::Sender<Result<(), String>>>,
}

/// One open port and its reader and writer threads. Dropped and rebuilt on
/// every disconnect.
struct Connection {
    writer_tx: Sender<WriteReq>,
    stop_tx:   Sender<()>,
    reader:    JoinHandle<()>,
    writer:    JoinHandle<()>,
}

fn connect(
    port_name: &str,
    baud: u32,
    events: &Sender<PortEvent>,
    state: &Arc<Shared>,
) -> Result<Connection> {
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
    let port = opened.with_context(|| format!("opening {port_name} @ {baud}"))?;
    let reader_port = port
        .try_clone()
        .context("cloning serial port for reader thread")?;

    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (writer_tx, writer_rx) = mpsc::channel::<WriteReq>();

    let reader_events = events.clone();
    let reader_state = Arc::clone(state);
    let reader =
        thread::spawn(move || reader_loop(reader_port, reader_events, stop_rx, reader_state));

    let writer_events = events.clone();
    let writer = thread::spawn(move || writer_loop(port, writer_rx, writer_events));

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
    let _ = conn.stop_tx.send(());
    drop(conn.writer_tx);
    let _ = conn.reader.join();
    let _ = conn.writer.join();
}

// Queue keyboard bytes for the writer thread. While the port is disconnected
// the input is dropped with a note instead of piling up for a dead port.
fn queue_write(
    conn: &Option<Connection>,
    ui: &mut Ui,
    log: &mut SessionLog,
    bytes: Vec<u8>,
) -> Result<()> {
    if let Some(c) = conn
        && c.writer_tx.send(WriteReq { bytes, resp: None }).is_ok()
    {
        return Ok(());
    }
    let note = "port disconnected, input not sent";
    ui.push_system(note);
    log.system(note)?;
    Ok(())
}

pub fn run(
    terminal: &mut DefaultTerminal,
    port_name: &str,
    baud: u32,
    eol: &[u8],
    mcp_bind: SocketAddr,
) -> Result<()> {
    let (event_tx, event_rx) = mpsc::channel::<PortEvent>();

    // Shared serial-console state the MCP server reads and writes through. The
    // reader thread feeds it received bytes. MCP tools queue input on the inject
    // channel, which this loop forwards to the writer thread, so the port is
    // only ever written from one place.
    let (inject_tx, mut inject_rx) = unbounded_channel::<Inject>();
    let state = Shared::new(port_name.to_string(), baud, eol.to_vec(), inject_tx);

    // The first open must succeed so a bad pick fails fast back at the picker.
    // Failures after that go through the reconnect loop instead of ending the
    // session and losing the scrollback.
    let mut conn = Some(connect(port_name, baud, &event_tx, &state)?);

    let mut log = SessionLog::create(port_name)?;

    let (ready_tx, ready_rx) = mpsc::channel::<Result<SocketAddr, String>>();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server_thread = mcp::spawn(mcp_bind, Arc::clone(&state), ready_tx, shutdown_rx);

    let mut config = config::Config::load();
    let mut ui = Ui {
        history: config.history.clone(),
        ..Default::default()
    };
    let mut last_attempt = Instant::now();
    let mut dirty = true;

    loop {
        while let Ok(event) = event_rx.try_recv() {
            match event {
                PortEvent::Rx(bytes) => {
                    log.rx(&bytes)?;
                    ui.push_rx(&bytes);
                    dirty = true;
                }
                PortEvent::Dead(reason) => {
                    // Both port threads report fatal errors here. Only the first
                    // report tears the connection down.
                    if let Some(c) = conn.take() {
                        teardown(c);
                        state.set_connected(false);
                        let note = format!("{reason}, reconnecting");
                        ui.push_system(&note);
                        log.system(&note)?;
                        last_attempt = Instant::now();
                        dirty = true;
                    }
                }
            }
        }

        // Log the MCP bind result once it arrives. The MCP endpoint is for agents,
        // so it goes to the session log only and is never shown in the TUI.
        if let Ok(result) = ready_rx.try_recv() {
            let note = match result {
                Ok(addr) => format!("mcp serving http://{addr}/mcp"),
                Err(e) => format!("mcp disabled: {e}"),
            };
            log.system(&note)?;
        }

        // Forward input queued by MCP clients, echoing and logging it like a
        // keystroke. While disconnected the client gets an error instead.
        while let Ok(inject) = inject_rx.try_recv() {
            let Some(c) = &conn else {
                let _ = inject.resp.send(Err("port disconnected".to_string()));
                continue;
            };
            ui.echo_agent(&inject.echo);
            log.tx_agent(&inject.bytes)?;
            dirty = true;
            let req = WriteReq {
                bytes: inject.bytes,
                resp:  Some(inject.resp),
            };
            if let Err(back) = c.writer_tx.send(req)
                && let Some(resp) = back.0.resp
            {
                let _ = resp.send(Err("port disconnected".to_string()));
            }
        }

        if conn.is_none() && last_attempt.elapsed() >= RECONNECT_EVERY {
            match connect(port_name, baud, &event_tx, &state) {
                Ok(c) => {
                    conn = Some(c);
                    state.set_connected(true);
                    ui.push_system("reconnected");
                    log.system("reconnected")?;
                    dirty = true;
                }
                // Logging every failed attempt would add a line each second for
                // as long as the device stays unplugged, so retries are silent.
                Err(_) => last_attempt = Instant::now(),
            }
        }

        // Redraw only when state changed. An unconditional draw on every 16ms
        // tick re-wrapped the scrollback at 60 fps and burned most of a core.
        if dirty {
            draw(terminal, &ui, port_name, baud, conn.is_some())?;
            dirty = false;
        }

        if !event::poll(Duration::from_millis(16))? {
            continue;
        }
        let key = match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => key,
            // a resize invalidates the frame even though no state changed.
            Event::Resize(_, _) => {
                dirty = true;
                continue;
            }
            _ => continue,
        };
        if is_quit(&key) {
            break;
        }
        dirty = true;

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char(c) if ctrl => {
                if let Some(byte) = ctrl_byte(c) {
                    log.tx_key(&format!("Ctrl+{}", c.to_ascii_uppercase()))?;
                    queue_write(&conn, &mut ui, &mut log, vec![byte])?;
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
                        let note = format!("config save failed: {e}");
                        ui.push_system(&note);
                        log.system(&note)?;
                    }
                    ui.history = config.history.clone();
                }
                ui.echo_tx(&text);
                let mut bytes = text.into_bytes();
                bytes.extend_from_slice(eol);
                log.tx_line(&bytes)?;
                queue_write(&conn, &mut ui, &mut log, bytes)?;
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

    let _ = shutdown_tx.send(());
    if let Some(c) = conn.take() {
        teardown(c);
    }
    // The MCP server thread is detached, not joined: a client holding an SSE
    // stream open can keep graceful shutdown from returning, so we signal it and
    // let process exit reap it rather than block quit on a client.
    drop(server_thread);

    Ok(())
}

fn draw<B: Backend>(
    terminal: &mut Terminal<B>,
    ui: &Ui,
    port_name: &str,
    baud: u32,
    connected: bool,
) -> Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let border = Style::new().fg(Color::DarkGray);
    let title = if connected {
        format!(" {port_name} @ {baud} ")
    } else {
        format!(" {port_name} @ {baud}  --  disconnected, retrying ")
    };

    terminal.draw(|frame| {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(frame.area());

        let out_area = chunks[0];
        let out_block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border)
            .title(Line::from(title))
            .title(Line::from(" ctrl+q: quit ").right_aligned());
        let out_inner = out_block.inner(out_area);
        frame.render_widget(out_block, out_area);

        let height = out_inner.height as usize;

        // Build and wrap only the newest lines that can reach the viewport. The view is always
        // pinned to the bottom, so anything older can never show, and wrapping the whole capped
        // history on every frame used to burn most of a core.
        let mut tail: Vec<Line> = Vec::new();
        let mut rows = 0usize;
        if !ui.rx_partial.is_empty() {
            let line = Line::from(ui.rx_partial.clone());
            rows += wrapped_rows(&line, out_inner.width);
            tail.push(line);
        }
        for line in ui.lines.iter().rev() {
            if rows >= height {
                break;
            }
            let styled = style_line(line);
            rows += wrapped_rows(&styled, out_inner.width);
            tail.push(styled);
        }
        let shown = tail.len();
        tail.reverse();

        let paragraph = Paragraph::new(Text::from(tail)).wrap(Wrap { trim: false });
        let scroll = rows.saturating_sub(height).min(u16::MAX as usize) as u16;
        frame.render_widget(paragraph.scroll((scroll, 0)), out_inner);

        // Only show the scrollbar when content actually overflows. Lines left out of the tail
        // wrap to at least one row each, which bounds the total row count from below without
        // wrapping them. The thumb stays pinned to the bottom, only its size is approximate.
        // content_length is the number of scroll positions, NOT the total row count: the thumb
        // reaches the bottom only when position == content_length, and the bottom-pinned view
        // is always at that maximum. viewport_content_length sizes the thumb to the visible
        // fraction.
        let total = ui.lines.len() + usize::from(!ui.rx_partial.is_empty());
        let total_rows = rows + (total - shown);
        if total_rows > height {
            let mut sb_state = ScrollbarState::new(total_rows - height)
                .viewport_content_length(height)
                .position(total_rows - height);
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

fn style_line(line: &OutLine) -> Line<'static> {
    let (text, style) = match line.origin {
        Origin::Rx => (line.text.clone(), Style::default()),
        Origin::Tx => (format!("> {}", line.text), Style::new().fg(Color::Cyan)),
        Origin::Agent => (format!(">> {}", line.text), Style::new().fg(Color::Magenta)),
        Origin::System => (format!("-- {}", line.text), Style::new().fg(Color::DarkGray)),
    };
    Line::styled(text, style)
}

// Ask the paragraph itself how many rows the line wraps to at this width. A hand-rolled
// ceil of chars over width drifts from the word-wrapper, which drops whitespace at wrap
// boundaries, so on wide terminals it over-counts and the scroll overshoots the real
// bottom, leaving the newest lines stranded at the top with blank space below.
fn wrapped_rows(line: &Line, width: u16) -> usize {
    Paragraph::new(line.clone())
        .wrap(Wrap { trim: false })
        .line_count(width)
}

fn reader_loop(
    mut port: Box<dyn SerialPort>,
    events: Sender<PortEvent>,
    stop_rx: Receiver<()>,
    state: Arc<Shared>,
) {
    let mut buf = [0u8; 4096];
    loop {
        if matches!(
            stop_rx.try_recv(),
            Ok(()) | Err(TryRecvError::Disconnected)
        ) {
            return;
        }
        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                // Feed the MCP buffer straight from the reader so expect() sees bytes without
                // waiting on the ~16ms UI tick.
                state.push_rx(&buf[..n]);
                if events.send(PortEvent::Rx(buf[..n].to_vec())).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == ErrorKind::TimedOut => {}
            Err(e) => {
                let _ = events.send(PortEvent::Dead(format!("read error: {e}")));
                return;
            }
        }
    }
}

fn writer_loop(mut port: Box<dyn SerialPort>, reqs: Receiver<WriteReq>, events: Sender<PortEvent>) {
    while let Ok(req) = reqs.recv() {
        match write_with_retry(port.as_mut(), &req.bytes) {
            Ok(()) => {
                if let Some(resp) = req.resp {
                    let _ = resp.send(Ok(()));
                }
            }
            Err(e) => {
                let msg = format!("write error: {e}");
                if let Some(resp) = req.resp {
                    let _ = resp.send(Err(msg.clone()));
                }
                let _ = events.send(PortEvent::Dead(msg));
                return;
            }
        }
    }
}

// Retry timed-out writes until the stall limit, resetting the clock whenever any
// bytes go through, so slow trickling progress is not mistaken for a dead port.
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

fn is_quit(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
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
    fn history_recall_walks_back_and_forward() {
        let mut ui = Ui {
            history: vec!["first".into(), "second".into()],
            ..Default::default()
        };

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
    fn scrollback_is_capped_from_the_front() {
        let mut ui = Ui::default();
        for i in 0..(MAX_LINES + 10) {
            ui.push_rx(format!("line {i}\n").as_bytes());
        }
        assert_eq!(ui.lines.len(), MAX_LINES);
        assert_eq!(ui.lines[0].text, "line 10");
    }

    #[test]
    fn suggestion_fuzzy_matches_history() {
        let mut ui = Ui {
            history: vec!["get_status".into(), "reboot".into(), "get_temp".into()],
            ..Default::default()
        };
        ui.set_input("gst");
        assert_eq!(ui.suggestion.as_deref(), Some("get_status"));
    }

    #[test]
    fn suggestion_skips_exact_input_and_empty() {
        let mut ui = Ui {
            history: vec!["reboot".into()],
            ..Default::default()
        };
        ui.set_input("reboot");
        assert_eq!(ui.suggestion, None);
        ui.clear_input();
        assert_eq!(ui.suggestion, None);
    }

    // Two real captured device lines. The word-wrapper drops the whitespace that lands on a
    // wrap boundary, so it packs each into fewer rows than a naive ceil(chars / width) count:
    // the first over-counts on a narrow terminal, the second on a wide one. Feeding many of
    // these used to make the naive row total overshoot the real wrapped height, so the scroll
    // ran past the bottom and stranded the newest lines at the top of the pane. Keep the
    // trailing spaces, they are what tips ceil() over while the wrapper still fits the line.
    const NARROW_OVERCOUNT: &str = "D I P1 0x00F 0x00000 26-07-01~05:40:42.222+00:00~#  PR03 S30 RestartManager ErrorlogReceiverRepositoryElement.cpp Line 77 : ErrorlogReceiverRepositoryElement::initializeAfterAllRepositoryElements() called, initializing errorlogReceiver~ ";
    const WIDE_OVERCOUNT: &str = "D I P1 0x00F 0x00000 26-07-01~05:40:45.039+00:00~#  PR03 S30 RestartManager RemoteLoggingRepositoryElement.cpp Line 180 : RemoteLoggingRepositoryElement::initializeAfterAllRepositoryElements() called, initializing BranchHandler~ ";

    #[test]
    fn narrow_terminal_pins_log_to_bottom_when_overflowing() {
        assert_bottom_filled(NARROW_OVERCOUNT, 237, 120, 24);
    }

    #[test]
    fn wide_terminal_pins_log_to_bottom_when_overflowing() {
        assert_bottom_filled(WIDE_OVERCOUNT, 229, 230, 20);
    }

    // Fill the pane with copies of an over-counting line and check the last text row is not
    // blank. The precondition asserts the line really does over-count at this width, so the
    // test can never silently pass on data that stopped triggering the bug (a lost trailing
    // space, or a change in ratatui's wrapping).
    fn assert_bottom_filled(line: &str, expected_len: usize, width: u16, height: u16) {
        use ratatui::{Terminal, backend::TestBackend};

        assert_eq!(line.chars().count(), expected_len, "test line length changed");
        let inner = width - 2;
        let naive = line.chars().count().div_ceil(inner as usize);
        let wrapped = Paragraph::new(line).wrap(Wrap { trim: false }).line_count(inner);
        assert!(
            naive > wrapped,
            "line must over-count at width {width}: naive={naive} wrapped={wrapped}"
        );

        let mut ui = Ui::default();
        for _ in 0..80 {
            ui.push_rx(line.as_bytes());
            ui.push_rx(b"\n");
        }

        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        draw(&mut terminal, &ui, "COM11", 115200, true).unwrap();
        let buf = terminal.backend().buffer();

        // The 3-row input box sits at the bottom, with the output box's own bottom border just
        // above it, so the last text row is height - 3 - 1 - 1 (0-indexed).
        let last_text_row = height - 3 - 1 - 1;
        let row: String = (1..width - 1)
            .map(|x| buf.cell((x, last_text_row)).unwrap().symbol())
            .collect();
        assert!(
            !row.trim().is_empty(),
            "bottom log row is blank at width {width} -- content over-scrolled off the top"
        );
    }
}
