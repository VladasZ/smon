use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use nucleo_matcher::{Config, Matcher, Utf32String};
use ratatui::{
    DefaultTerminal,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, List, ListItem, ListState, Paragraph},
};

pub struct Item {
    pub value: String,
    pub label: String,
}

pub fn pick(
    terminal: &mut DefaultTerminal,
    title: &str,
    items: &[Item],
    default_index: Option<usize>,
) -> Result<Option<String>> {
    let mut matcher = Matcher::new(Config::DEFAULT);
    let labels: Vec<Utf32String> = items
        .iter()
        .map(|i| Utf32String::from(i.label.as_str()))
        .collect();

    let mut query = String::new();
    let mut state = ListState::default();
    state.select(default_index.or(if items.is_empty() { None } else { Some(0) }));

    loop {
        let filtered: Vec<&Item> = if query.is_empty() {
            items.iter().collect()
        } else {
            let needle = Utf32String::from(query.as_str());
            let mut hits: Vec<(&Item, u16)> = items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| {
                    matcher
                        .fuzzy_match(labels[i].slice(..), needle.slice(..))
                        .map(|score| (item, score))
                })
                .collect();
            hits.sort_by_key(|h| std::cmp::Reverse(h.1));
            hits.into_iter().map(|(item, _)| item).collect()
        };

        if filtered.is_empty() {
            state.select(None);
        } else if state.selected().is_none_or(|s| s >= filtered.len()) {
            state.select(Some(0));
        }

        terminal.draw(|frame| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(0),
                    Constraint::Length(1),
                ])
                .split(frame.area());

            let header = Paragraph::new(format!("> {query}"))
                .block(Block::bordered().title(title));
            frame.render_widget(header, chunks[0]);

            let list_items: Vec<ListItem> = filtered
                .iter()
                .map(|item| ListItem::new(item.label.as_str()))
                .collect();
            let list = List::new(list_items)
                .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
                .highlight_symbol("> ");
            frame.render_stateful_widget(list, chunks[1], &mut state);

            let footer = Paragraph::new(Line::from(
                "type to filter | up/down: move | enter: select | esc: cancel",
            ));
            frame.render_widget(footer, chunks[2]);
        })?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(None),
            (KeyCode::Enter, _) => {
                if let Some(sel) = state.selected()
                    && let Some(item) = filtered.get(sel)
                {
                    return Ok(Some(item.value.clone()));
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
