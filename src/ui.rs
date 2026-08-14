//! What the person at the terminal sees: the scrollback and the input line.

use std::{collections::VecDeque, mem::take};

use nucleo_matcher::{
    Config as FuzzyConfig, Matcher, Utf32String,
    pattern::{CaseMatching, Normalization, Pattern},
};

use crate::console::Origin;

pub const MAX_LINES: usize = 5000;

/// Where a scrollback line came from. Input typed here and input injected by an
/// agent are told apart, so the person can see what a client did.
pub enum Source {
    Rx,
    Local,
    Agent,
    System,
}

impl From<Origin> for Source {
    fn from(origin: Origin) -> Self {
        match origin {
            Origin::Typed | Origin::Key => Source::Local,
            Origin::Agent | Origin::Bridge => Source::Agent,
        }
    }
}

pub struct OutLine {
    pub source: Source,
    pub text:   String,
}

/// The query typed into the search prompt while it is open.
pub struct Search {
    pub query: Vec<char>,
}

#[derive(Default)]
pub struct Ui {
    pub lines:      VecDeque<OutLine>,
    pub rx_partial: String,
    pub input:      Vec<char>,
    pub cursor:     usize,
    pub history:    Vec<String>,
    pub hist_pos:   Option<usize>,
    pub suggestion: Option<String>,
    /// Complete lines between the bottom of the view and the newest line. Zero
    /// means pinned to the live bottom.
    pub scroll:     usize,
    /// The scrollback search prompt, open while this is Some.
    pub search:     Option<Search>,
}

impl Ui {
    pub fn push_rx(&mut self, bytes: &[u8]) {
        for ch in String::from_utf8_lossy(bytes).chars() {
            match ch {
                '\n' => self.end_line(),
                // Control bytes would corrupt the on-screen lines. The log file
                // keeps the escaped form.
                '\r' => {}
                c if c.is_control() && c != '\t' => {}
                c => self.rx_partial.push(c),
            }
        }
        self.cap_lines();
    }

    fn end_line(&mut self) {
        let text = take(&mut self.rx_partial);
        self.push_line(OutLine {
            source: Source::Rx,
            text,
        });
    }

    fn push_line(&mut self, line: OutLine) {
        self.lines.push_back(line);
        // A view scrolled into the past stays on what it shows. New lines land
        // below the window, so the offset grows with them.
        if self.scroll > 0 {
            self.scroll += 1;
        }
    }

    /// Input that reached the port, whoever sent it. It joins the same
    /// scrollback in order with everything else.
    pub fn push_echo(&mut self, origin: Origin, text: &str) {
        self.push_out(origin.into(), text);
    }

    pub fn push_system(&mut self, text: &str) {
        self.push_out(Source::System, text);
    }

    fn push_out(&mut self, source: Source, text: &str) {
        if !self.rx_partial.is_empty() {
            self.end_line();
        }
        self.push_line(OutLine {
            source,
            text: text.to_string(),
        });
        self.cap_lines();
    }

    fn cap_lines(&mut self) {
        while self.lines.len() > MAX_LINES {
            self.lines.pop_front();
        }
        self.scroll = self.scroll.min(self.lines.len().saturating_sub(1));
    }

    /// Wheel up: further into the past.
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll = (self.scroll + n).min(self.lines.len().saturating_sub(1));
    }

    pub fn scroll_down(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_sub(n);
    }

    pub fn take_input(&mut self) -> String {
        self.cursor = 0;
        self.hist_pos = None;
        // Sending a command means watching for the reply, so the view snaps
        // back to the live bottom.
        self.scroll = 0;
        let text: String = take(&mut self.input).into_iter().collect();
        self.update_suggestion();
        text
    }

    pub fn insert(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += 1;
        self.update_suggestion();
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.input.remove(self.cursor);
            self.update_suggestion();
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.input.len() {
            self.input.remove(self.cursor);
            self.update_suggestion();
        }
    }

    pub fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
        self.hist_pos = None;
        self.update_suggestion();
    }

    pub fn set_input(&mut self, text: &str) {
        self.input = text.chars().collect();
        self.cursor = self.input.len();
        self.update_suggestion();
    }

    pub fn accept_suggestion(&mut self) {
        if let Some(suggestion) = self.suggestion.clone() {
            self.set_input(&suggestion);
        }
    }

    fn update_suggestion(&mut self) {
        let input: String = self.input.iter().collect();
        if input.is_empty() {
            self.suggestion = None;
            return;
        }
        let mut matcher = Matcher::new(FuzzyConfig::DEFAULT);
        // Pattern rather than a raw needle, for the same reason as in picker.rs.
        // fuzzy_match panics on any uppercase input, because its prefilter folds
        // case and the optimal pass does not. Here that would kill a live session
        // as soon as a recalled command carried a capital.
        let pattern = Pattern::parse(&input, CaseMatching::Ignore, Normalization::Smart);
        let mut best: Option<(u32, &String)> = None;
        for cmd in &self.history {
            if *cmd == input {
                continue;
            }
            let hay = Utf32String::from(cmd.as_str());
            if let Some(score) = pattern.score(hay.slice(..), &mut matcher)
                && best.is_none_or(|(b, _)| score >= b)
            {
                best = Some((score, cmd));
            }
        }
        self.suggestion = best.map(|(_, cmd)| cmd.clone());
    }

    pub fn history_prev(&mut self) {
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

    pub fn history_next(&mut self) {
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

    pub fn open_search(&mut self) {
        if self.search.is_none() {
            self.search = Some(Search { query: Vec::new() });
        }
    }

    pub fn close_search(&mut self) {
        self.search = None;
    }

    pub fn search_insert(&mut self, c: char) {
        if let Some(search) = &mut self.search {
            search.query.push(c);
        }
    }

    pub fn search_backspace(&mut self) {
        if let Some(search) = &mut self.search {
            search.query.pop();
        }
    }

    /// The query to match with, or None while the prompt is closed or empty.
    pub fn search_query(&self) -> Option<String> {
        let query: String = self.search.as_ref()?.query.iter().collect();
        if query.is_empty() { None } else { Some(query) }
    }

    /// How many scrollback lines match the query.
    pub fn search_count(&self) -> usize {
        let Some(query) = self.search_query() else {
            return 0;
        };
        self.lines.iter().filter(|l| !smart_find(&l.text, &query).is_empty()).count()
    }

    // The line the view's bottom row shows. A jump puts the match there, so it
    // is also where the walk continues from.
    fn view_bottom(&self) -> usize {
        self.lines.len().saturating_sub(1).saturating_sub(self.scroll)
    }

    /// Jump the view to the nearest matching line above the current position.
    pub fn search_older(&mut self) {
        let Some(query) = self.search_query() else {
            return;
        };
        for idx in (0..self.view_bottom()).rev() {
            if !smart_find(&self.lines[idx].text, &query).is_empty() {
                self.scroll = self.lines.len() - 1 - idx;
                return;
            }
        }
    }

    /// Jump the view to the nearest matching line below the current position.
    pub fn search_newer(&mut self) {
        let Some(query) = self.search_query() else {
            return;
        };
        for idx in self.view_bottom() + 1..self.lines.len() {
            if !smart_find(&self.lines[idx].text, &query).is_empty() {
                self.scroll = self.lines.len() - 1 - idx;
                return;
            }
        }
    }
}

/// Byte ranges of every occurrence of `query` in `hay`. Case-insensitive until
/// the query carries an uppercase letter, then exact, like less and modern
/// editors.
pub fn smart_find(hay: &str, query: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    let exact = query.chars().any(char::is_uppercase);
    let needle: Vec<char> = query.chars().collect();
    let chars: Vec<(usize, char)> = hay.char_indices().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i + needle.len() <= chars.len() {
        if needle.iter().zip(&chars[i..]).all(|(q, (_, h))| char_eq(*h, *q, exact)) {
            let start = chars[i].0;
            let end = chars.get(i + needle.len()).map_or(hay.len(), |(b, _)| *b);
            found.push((start, end));
            i += needle.len();
        } else {
            i += 1;
        }
    }
    found
}

fn char_eq(a: char, b: char, exact: bool) -> bool {
    if exact {
        a == b
    } else {
        a.to_lowercase().eq(b.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn scroll_clamps_at_the_ends() {
        let mut ui = Ui::default();
        for i in 0..10 {
            ui.push_rx(format!("line {i}\n").as_bytes());
        }
        ui.scroll_up(100);
        assert_eq!(ui.scroll, 9);
        ui.scroll_down(3);
        assert_eq!(ui.scroll, 6);
        ui.scroll_down(100);
        assert_eq!(ui.scroll, 0);
    }

    // A person reading the past must not have the view yanked away by new
    // output, while a view at the bottom keeps following it.
    #[test]
    fn new_lines_do_not_move_a_scrolled_view() {
        let mut ui = Ui::default();
        for i in 0..10 {
            ui.push_rx(format!("line {i}\n").as_bytes());
        }
        ui.scroll_up(3);
        ui.push_rx(b"more\n");
        ui.push_echo(Origin::Typed, "sent");
        assert_eq!(ui.scroll, 5);

        ui.scroll_down(100);
        ui.push_rx(b"even more\n");
        assert_eq!(ui.scroll, 0);
    }

    #[test]
    fn sending_input_pins_back_to_the_bottom() {
        let mut ui = Ui::default();
        for i in 0..10 {
            ui.push_rx(format!("line {i}\n").as_bytes());
        }
        ui.scroll_up(5);
        ui.set_input("reboot");
        assert_eq!(ui.take_input(), "reboot");
        assert_eq!(ui.scroll, 0);
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

    // A keystroke and an agent's injection must not look the same in the
    // scrollback, that distinction is the point of showing injected input at all.
    #[test]
    fn typed_and_injected_input_land_in_different_sources() {
        let mut ui = Ui::default();
        ui.push_echo(Origin::Typed, "version");
        ui.push_echo(Origin::Agent, "reboot");
        ui.push_echo(Origin::Key, "Ctrl+C");

        assert!(matches!(ui.lines[0].source, Source::Local));
        assert!(matches!(ui.lines[1].source, Source::Agent));
        assert!(matches!(ui.lines[2].source, Source::Local));
    }

    #[test]
    fn smart_find_ignores_case_until_the_query_has_an_uppercase() {
        assert_eq!(smart_find("Boot OK boot", "boot"), vec![(0, 4), (8, 12)]);
        assert_eq!(smart_find("Boot OK boot", "Boot"), vec![(0, 4)]);
        assert_eq!(smart_find("nothing here", "boot"), Vec::new());
        assert_eq!(smart_find("anything", ""), Vec::new());
    }

    fn searching_ui(query: &str) -> Ui {
        let mut ui = Ui::default();
        for i in 0..10 {
            ui.push_rx(format!("line {i}\n").as_bytes());
        }
        ui.push_rx(b"error one\n");
        for i in 10..20 {
            ui.push_rx(format!("line {i}\n").as_bytes());
        }
        ui.push_rx(b"error two\n");
        for i in 20..25 {
            ui.push_rx(format!("line {i}\n").as_bytes());
        }
        ui.open_search();
        for c in query.chars() {
            ui.search_insert(c);
        }
        ui
    }

    // Walking older puts each match at the bottom of the view, and walking
    // newer comes back the same way.
    #[test]
    fn search_walks_older_and_newer_matches() {
        let mut ui = searching_ui("error");
        // 27 lines total: indices 10 and 21 match.
        ui.search_older();
        assert_eq!(ui.scroll, 27 - 1 - 21);
        ui.search_older();
        assert_eq!(ui.scroll, 27 - 1 - 10);
        // No older match, the view stays put.
        ui.search_older();
        assert_eq!(ui.scroll, 27 - 1 - 10);
        ui.search_newer();
        assert_eq!(ui.scroll, 27 - 1 - 21);
        ui.search_newer();
        assert_eq!(ui.scroll, 27 - 1 - 21);
    }

    #[test]
    fn search_counts_matching_lines() {
        let ui = searching_ui("error");
        assert_eq!(ui.search_count(), 2);
        let ui = searching_ui("nothing-like-this");
        assert_eq!(ui.search_count(), 0);
    }

    // An empty query must not move the view, or opening the prompt and hitting
    // Enter would jump somewhere meaningless.
    #[test]
    fn empty_query_does_not_navigate() {
        let mut ui = searching_ui("");
        ui.search_older();
        assert_eq!(ui.scroll, 0);
        assert_eq!(ui.search_query(), None);
    }

    #[test]
    fn closing_and_reopening_search_keeps_nothing_stale() {
        let mut ui = searching_ui("error");
        ui.close_search();
        assert!(ui.search.is_none());
        assert_eq!(ui.search_count(), 0);
        ui.open_search();
        assert_eq!(ui.search_query(), None);
    }

    // A partial line still on the wire must be closed before an echo goes in,
    // or the two would run together on one row.
    #[test]
    fn an_echo_closes_a_partial_received_line() {
        let mut ui = Ui::default();
        ui.push_rx(b"boot");
        ui.push_echo(Origin::Typed, "version");
        assert_eq!(ui.lines[0].text, "boot");
        assert_eq!(ui.lines[1].text, "version");
        assert!(ui.rx_partial.is_empty());
    }
}
