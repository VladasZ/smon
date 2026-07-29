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

use crate::ui::{OutLine, Source, Ui};

pub fn draw<B: Backend>(terminal: &mut Terminal<B>, ui: &Ui, title: &str, connected: bool) -> Result<()>
where B::Error: std::error::Error + Send + Sync + 'static {
    let border = Style::new().fg(Color::DarkGray);
    let title = if connected {
        format!(" {title} ")
    } else {
        format!(" {title}  --  disconnected, retrying ")
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

        // Build and wrap only the newest lines that can reach the viewport. The view is
        // always pinned to the bottom, so anything older can never show, and
        // wrapping the whole capped history on every frame used to burn most of
        // a core.
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

        // Lines left out of the tail wrap to at least one row each, which bounds the
        // total row count from below without wrapping them, so the thumb size
        // is approximate. content_length is the number of scroll positions, not
        // the total row count. The thumb reaches the bottom only when position
        // == content_length, and the bottom-pinned view is always at that
        // maximum.
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

fn style_line(line: &OutLine) -> Line<'static> {
    let (text, style) = match line.source {
        Source::Rx => (line.text.clone(), Style::default()),
        Source::Local => (format!("> {}", line.text), Style::new().fg(Color::Cyan)),
        Source::Agent => (format!(">> {}", line.text), Style::new().fg(Color::Magenta)),
        Source::System => (format!("-- {}", line.text), Style::new().fg(Color::DarkGray)),
    };
    Line::styled(text, style)
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
        draw(&mut terminal, &ui, "COM11 @ 115200", true).unwrap();
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
}
