use std::{
    cmp::Reverse,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use nucleo_matcher::{Config, Matcher, Utf32String};
use ratatui::{
    DefaultTerminal,
    layout::{Constraint, Direction, Layout, Position},
    style::{Color, Style},
    text::Line,
    widgets::{
        Block, BorderType, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState,
    },
};

#[derive(Clone)]
pub struct Item {
    pub value: String,
    pub label: String,
    pub busy: bool,
}

fn make_labels(items: &[Item]) -> Vec<Utf32String> {
    items
        .iter()
        .map(|i| Utf32String::from(i.label.as_str()))
        .collect()
}

// Indices into the item list matching `query`, best fuzzy score first.
fn filter_items(labels: &[Utf32String], query: &str, matcher: &mut Matcher) -> Vec<usize> {
    if query.is_empty() {
        return (0..labels.len()).collect();
    }
    let needle = Utf32String::from(query);
    let mut hits: Vec<(usize, u16)> = labels
        .iter()
        .enumerate()
        .filter_map(|(i, label)| {
            matcher
                .fuzzy_match(label.slice(..), needle.slice(..))
                .map(|score| (i, score))
        })
        .collect();
    hits.sort_by_key(|h| Reverse(h.1));
    hits.into_iter().map(|(i, _)| i).collect()
}

// `source` is re-called every second so the list (and each port's busy state) stays current
// while the picker is open.
pub fn pick(
    terminal: &mut DefaultTerminal,
    title: &str,
    mut source: impl FnMut() -> Vec<Item>,
    default_index: Option<usize>,
    allow_free_text: bool,
) -> Result<Option<String>> {
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut items = source();
    let mut labels = make_labels(&items);

    let mut query = String::new();
    let mut state = ListState::default();
    state.select(default_index.or(if items.is_empty() { None } else { Some(0) }));

    let mut last_refresh = Instant::now();
    let mut filtered: Vec<usize> = Vec::new();
    let mut dirty = true;

    loop {
        if last_refresh.elapsed() >= Duration::from_secs(1) {
            items = source();
            labels = make_labels(&items);
            last_refresh = Instant::now();
            dirty = true;
        }

        // Refilter and redraw only when the list, query, or selection changed,
        // not on every poll tick.
        if dirty {
            filtered = filter_items(&labels, &query, &mut matcher);
            if filtered.is_empty() {
                state.select(None);
            } else if state.selected().is_none_or(|s| s >= filtered.len()) {
                state.select(Some(0));
            }

            terminal.draw(|frame| {
                let border = Style::new().fg(Color::DarkGray);

                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(0), Constraint::Length(3)])
                    .split(frame.area());

                let list_area = chunks[0];
                let list_block = Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(border)
                    .title(Line::from(format!(" {title} ")))
                    .title(Line::from(" esc: cancel ").right_aligned());
                let list_inner = list_block.inner(list_area);
                frame.render_widget(list_block, list_area);

                let list_items: Vec<ListItem> = filtered
                    .iter()
                    .map(|&i| {
                        let item = &items[i];
                        if item.busy {
                            ListItem::new(format!("{}   busy", item.label))
                                .style(Style::new().fg(Color::DarkGray))
                        } else {
                            ListItem::new(item.label.as_str())
                        }
                    })
                    .collect();
                let list = List::new(list_items)
                    .highlight_style(Style::new().fg(Color::Cyan))
                    .highlight_symbol("> ");
                frame.render_stateful_widget(list, list_inner, &mut state);

                let view = list_inner.height as usize;
                if filtered.len() > view {
                    let mut sb_state = ScrollbarState::new(filtered.len())
                        .viewport_content_length(view)
                        .position(state.selected().unwrap_or(0));
                    frame.render_stateful_widget(
                        Scrollbar::new(ScrollbarOrientation::VerticalRight)
                            .begin_symbol(None)
                            .end_symbol(None),
                        list_area,
                        &mut sb_state,
                    );
                }

                let input_area = chunks[1];
                let input_block = Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(border);
                let input_inner = input_block.inner(input_area);
                frame.render_widget(input_block, input_area);

                let cursor = query.chars().count();
                let avail = input_inner.width.max(1) as usize;
                let scroll_x = cursor.saturating_sub(avail.saturating_sub(1));
                frame.render_widget(
                    Paragraph::new(query.as_str()).scroll((0, scroll_x as u16)),
                    input_inner,
                );
                frame.set_cursor_position(Position {
                    x: input_inner.x + (cursor - scroll_x) as u16,
                    y: input_inner.y,
                });
            })?;
            dirty = false;
        }

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let key = match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => key,
            Event::Resize(_, _) => {
                dirty = true;
                continue;
            }
            _ => continue,
        };
        dirty = true;
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(None),
            (KeyCode::Enter, _) => {
                if let Some(sel) = state.selected()
                    && let Some(&idx) = filtered.get(sel)
                {
                    let item = &items[idx];
                    if !item.busy {
                        return Ok(Some(item.value.clone()));
                    }
                    // a busy port can't be opened, so selecting it does nothing.
                } else if allow_free_text && !query.is_empty() {
                    return Ok(Some(query));
                }
            }
            (KeyCode::Up, _) => {
                let len = filtered.len();
                if len > 0 {
                    let i = state.selected().unwrap_or(0);
                    state.select(Some(if i == 0 { len - 1 } else { i - 1 }));
                }
            }
            (KeyCode::Down, _) => {
                let len = filtered.len();
                if len > 0 {
                    let i = state.selected().unwrap_or(0);
                    state.select(Some((i + 1) % len));
                }
            }
            (KeyCode::Backspace, _) => {
                query.pop();
            }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                query.push(c);
            }
            _ => {}
        }
    }
}
