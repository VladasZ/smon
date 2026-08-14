//! Rendering the session screen.

use anyhow::Result;
use ratatui::{
    Terminal,
    backend::Backend,
    layout::{Constraint, Direction, Layout, Position},
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};

use crate::ui::{OutLine, Source, Ui, smart_find};

pub fn draw<B: Backend>(terminal: &mut Terminal<B>, ui: &mut Ui, title: &str, connected: bool) -> Result<()>
where B::Error: std::error::Error + Send + Sync + 'static {
    let border = Style::new().fg(Color::DarkGray);
    let title = if connected {
        format!(" {title} ")
    } else {
        format!(" {title}  --  disconnected, retrying ")
    };
    let query = ui.search_query();
    let search = ui
        .search
        .as_ref()
        .map(|s| (s.query.iter().collect::<String>(), ui.search_count()));

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
            .title(Line::from(" ctrl+f: search  ctrl+q: quit ").right_aligned());
        let out_inner = out_block.inner(out_area);
        frame.render_widget(out_block, out_area);

        let height = out_inner.height as usize;

        // Build and wrap only the lines that can reach the viewport. The view
        // sits ui.scroll lines above the live bottom, so nothing outside that
        // window can show, and wrapping the whole capped history on every frame
        // used to burn most of a core.
        let mut scroll = ui.scroll.min(ui.lines.len().saturating_sub(1));
        let mut tail: Vec<Line> = Vec::new();
        let mut rows = 0usize;
        if scroll == 0 && !ui.rx_partial.is_empty() {
            let line = Line::from(ui.rx_partial.clone());
            rows += wrapped_rows(&line, out_inner.width);
            tail.push(line);
        }
        for line in ui.lines.iter().rev().skip(scroll) {
            if rows >= height {
                break;
            }
            let styled = style_line(line, query.as_deref());
            rows += wrapped_rows(&styled, out_inner.width);
            tail.push(styled);
        }
        // Scrolled past the top: pull lines back in from below the window until
        // the pane is full again, so the view stops at the oldest full screen
        // instead of draining off the top.
        while rows < height && scroll > 0 {
            scroll -= 1;
            let styled = style_line(&ui.lines[ui.lines.len() - 1 - scroll], query.as_deref());
            rows += wrapped_rows(&styled, out_inner.width);
            tail.insert(0, styled);
            if scroll == 0 && rows < height && !ui.rx_partial.is_empty() {
                let line = Line::from(ui.rx_partial.clone());
                rows += wrapped_rows(&line, out_inner.width);
                tail.insert(0, line);
            }
        }
        ui.scroll = scroll;
        let shown = tail.len();
        tail.reverse();

        let paragraph = Paragraph::new(Text::from(tail)).wrap(Wrap { trim: false });
        let para_scroll = rows.saturating_sub(height).min(u16::MAX as usize) as u16;
        frame.render_widget(paragraph.scroll((para_scroll, 0)), out_inner);

        // Lines left out of the tail wrap to at least one row each, which bounds the
        // total row count from below without wrapping them, so the thumb size
        // and position are approximate. content_length is the number of scroll
        // positions, not the total row count. The thumb reaches the bottom only
        // when position == content_length, which is where the bottom-pinned
        // view sits.
        let total = ui.lines.len() + usize::from(!ui.rx_partial.is_empty());
        let total_rows = rows + (total - shown);
        if total_rows > height {
            let below = scroll + usize::from(scroll > 0 && !ui.rx_partial.is_empty());
            let mut sb_state = ScrollbarState::new(total_rows - height)
                .viewport_content_length(height)
                .position((total_rows - height).saturating_sub(below));
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(None)
                    .end_symbol(None),
                out_area,
                &mut sb_state,
            );
        }

        let in_area = chunks[1];
        // While the search prompt is open it replaces the input line, and the
        // yellow border says keystrokes are going to the search, not the device.
        if let Some((typed, count)) = &search {
            let in_block = Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(Color::Yellow))
                .title(Line::from(" search  --  enter: older  down: newer  esc: close "))
                .title(Line::from(format!(" {count} lines ")).right_aligned());
            let in_inner = in_block.inner(in_area);
            frame.render_widget(in_block, in_area);
            let avail = in_inner.width.max(1) as usize;
            let len = typed.chars().count();
            let scroll_x = len.saturating_sub(avail.saturating_sub(1));
            frame.render_widget(
                Paragraph::new(typed.clone()).scroll((0, scroll_x as u16)),
                in_inner,
            );
            frame.set_cursor_position(Position {
                x: in_inner.x + (len - scroll_x) as u16,
                y: in_inner.y,
            });
            return;
        }
        let in_block = Block::bordered().border_type(BorderType::Rounded).border_style(border);
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

fn style_line(line: &OutLine, query: Option<&str>) -> Line<'static> {
    let (prefix, style) = match line.source {
        Source::Rx => ("", Style::default()),
        Source::Local => ("> ", Style::new().fg(Color::Cyan)),
        Source::Agent => (">> ", Style::new().fg(Color::Magenta)),
        Source::System => ("-- ", Style::new().fg(Color::DarkGray)),
    };
    let ranges = query.map_or_else(Vec::new, |q| smart_find(&line.text, q));
    if ranges.is_empty() {
        return Line::styled(format!("{prefix}{}", line.text), style);
    }
    let highlight = Style::new().fg(Color::Black).bg(Color::Yellow);
    let mut spans = Vec::new();
    if !prefix.is_empty() {
        spans.push(Span::styled(prefix, style));
    }
    let mut pos = 0;
    for (start, end) in ranges {
        if start > pos {
            spans.push(Span::styled(line.text[pos..start].to_string(), style));
        }
        spans.push(Span::styled(line.text[start..end].to_string(), highlight));
        pos = end;
    }
    if pos < line.text.len() {
        spans.push(Span::styled(line.text[pos..].to_string(), style));
    }
    Line::from(spans)
}

// Ask the paragraph itself how many rows the line wraps to at this width. A
// hand-rolled ceil of chars over width drifts from the word-wrapper, which
// drops whitespace at wrap boundaries, so on wide terminals it over-counts and
// the scroll overshoots the real bottom, leaving the newest lines stranded at
// the top with blank space below.
fn wrapped_rows(line: &Line, width: u16) -> usize {
    Paragraph::new(line.clone()).wrap(Wrap { trim: false }).line_count(width)
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;

    use super::*;

    // Two words of exactly the pane width, plus the space between them and a
    // trailing one. The wrapper fits each word on its own row and drops the
    // space at the break, so the line takes 2 rows while ceil(chars / width)
    // counts the spaces and asks for 3. Feeding many of these used to make the
    // naive row total overshoot the real wrapped height, so the scroll ran past
    // the bottom and stranded the newest lines at the top of the pane.
    fn overcounting_line(inner: u16) -> String {
        format!("{} {} ", "a".repeat(inner as usize), "b".repeat(inner as usize))
    }

    #[test]
    fn narrow_terminal_pins_log_to_bottom_when_overflowing() {
        assert_bottom_filled(120, 24);
    }

    #[test]
    fn wide_terminal_pins_log_to_bottom_when_overflowing() {
        assert_bottom_filled(230, 20);
    }

    // Fill the pane with copies of an over-counting line and check the last text
    // row is not blank. The precondition asserts the line really does
    // over-count at this width, so the test can never silently pass on data
    // that stopped triggering the bug, for example after a change in ratatui's
    // wrapping.
    fn assert_bottom_filled(width: u16, height: u16) {
        let inner = width - 2;
        let line = overcounting_line(inner);
        let naive = line.chars().count().div_ceil(inner as usize);
        let wrapped = Paragraph::new(line.as_str()).wrap(Wrap { trim: false }).line_count(inner);
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
        draw(&mut terminal, &mut ui, "COM11 @ 115200", true).unwrap();
        let buf = terminal.backend().buffer();

        // The 3-row input box sits at the bottom, with the output box's bottom border
        // just above it, so the last 0-indexed text row is height - 3 - 1 - 1.
        let last_text_row = height - 3 - 1 - 1;
        let row: String = (1..width - 1).map(|x| buf.cell((x, last_text_row)).unwrap().symbol()).collect();
        assert!(
            !row.trim().is_empty(),
            "bottom log row is blank at width {width} -- content over-scrolled off the top"
        );
    }

    fn text_row(terminal: &Terminal<TestBackend>, width: u16, y: u16) -> String {
        let buf = terminal.backend().buffer();
        (1..width - 1).map(|x| buf.cell((x, y)).unwrap().symbol()).collect()
    }

    fn numbered_ui(count: usize) -> Ui {
        let mut ui = Ui::default();
        for i in 0..count {
            ui.push_rx(format!("line {i}\n").as_bytes());
        }
        ui
    }

    #[test]
    fn scrolled_view_shows_older_lines_at_the_bottom() {
        let (width, height) = (40u16, 12u16);
        let mut ui = numbered_ui(40);
        ui.scroll = 10;

        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        draw(&mut terminal, &mut ui, "COM11 @ 115200", true).unwrap();

        let last_text_row = height - 3 - 1 - 1;
        let row = text_row(&terminal, width, last_text_row);
        assert!(
            row.contains("line 29"),
            "expected line 29 at the bottom, got: {row}"
        );
        assert_eq!(ui.scroll, 10);
    }

    // The person must see where their keystrokes go, so the search prompt takes
    // over the bottom box, and the matches must be visibly marked in the pane.
    #[test]
    fn search_prompt_replaces_the_input_line_and_highlights_matches() {
        let (width, height) = (60u16, 12u16);
        let mut ui = numbered_ui(5);
        ui.push_rx(b"an error happened\n");
        ui.open_search();
        for c in "error".chars() {
            ui.search_insert(c);
        }

        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        draw(&mut terminal, &mut ui, "COM11 @ 115200", true).unwrap();

        let input_row = text_row(&terminal, width, height - 2);
        assert!(input_row.contains("error"), "query not shown: {input_row}");

        let buf = terminal.backend().buffer();
        let mut highlighted = false;
        for y in 1..height - 4 {
            let row = text_row(&terminal, width, y);
            if row.contains("happened") {
                let idx = row.find("error").unwrap();
                let cell = buf.cell((1 + idx as u16, y)).unwrap();
                assert_eq!(cell.bg, Color::Yellow, "match not highlighted in: {row}");
                highlighted = true;
            }
        }
        assert!(highlighted, "the matching line never rendered");
    }

    // Scrolling far past the top must stop at the oldest full screen, not leave
    // the pane mostly blank, and the offset must be pinned there so scrolling
    // back down starts moving immediately.
    #[test]
    fn over_scroll_clamps_to_the_oldest_full_screen() {
        let (width, height) = (40u16, 12u16);
        let mut ui = numbered_ui(40);
        ui.scroll = 1000;

        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        draw(&mut terminal, &mut ui, "COM11 @ 115200", true).unwrap();

        let top = text_row(&terminal, width, 1);
        assert!(top.contains("line 0"), "expected line 0 at the top, got: {top}");
        let pane_rows = usize::from(height) - 3 - 2;
        assert_eq!(ui.scroll, 40 - pane_rows);
    }
}
