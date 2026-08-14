//! Searching a console's log segments on disk.

use std::{fs, path::PathBuf};

use anyhow::{Context, Result};

use crate::ring::Matcher;

/// One matching log line and where it was found.
pub struct FileMatch {
    pub file:   PathBuf,
    /// 1-based line number within the file.
    pub line:   u64,
    pub text:   String,
    /// Lines just before and after the match, when context was asked for.
    pub before: Vec<String>,
    pub after:  Vec<String>,
}

pub struct Found {
    /// Newest first, both across files and within one.
    pub matches:   Vec<FileMatch>,
    /// True when more matches exist than `max_results` allowed to return.
    pub truncated: bool,
    pub files:     usize,
}

/// Scan `files`, given newest first, and keep the newest `max_results` matching
/// lines. Every file is still scanned in full, so `truncated` is exact.
///
/// # Errors
/// Returns an error if a file cannot be read.
pub fn search_files(
    files: &[PathBuf],
    matcher: &Matcher,
    max_results: usize,
    context: usize,
) -> Result<Found> {
    let mut matches = Vec::new();
    let mut total = 0;
    for file in files {
        let bytes = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
        let mut lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
        // The split leaves an empty tail after the final newline, which is not
        // a line of the file.
        if lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        let hits: Vec<usize> = (0..lines.len()).filter(|i| matcher.find_end(lines[*i]).is_some()).collect();
        total += hits.len();
        for &i in hits.iter().rev() {
            if matches.len() >= max_results {
                break;
            }
            matches.push(FileMatch {
                file:   file.clone(),
                line:   (i + 1) as u64,
                text:   text_of(lines[i]),
                before: lines[i.saturating_sub(context)..i].iter().map(|l| text_of(l)).collect(),
                after:  lines[i + 1..(i + 1 + context).min(lines.len())]
                    .iter()
                    .map(|l| text_of(l))
                    .collect(),
            });
        }
    }
    Ok(Found {
        matches,
        truncated: total > max_results,
        files: files.len(),
    })
}

fn text_of(line: &[u8]) -> String {
    String::from_utf8_lossy(line).into_owned()
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::*;

    fn temp_files(name: &str, contents: &[&str]) -> Vec<PathBuf> {
        let dir = env::temp_dir().join(format!("smon-search-test-{name}"));
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        fs::create_dir_all(&dir).unwrap();
        contents
            .iter()
            .enumerate()
            .map(|(i, text)| {
                // Reverse-numbered so index 0 is the newest, like the caller's
                // newest-first segment list.
                let path = dir.join(format!("{}.log", contents.len() - i));
                fs::write(&path, text).unwrap();
                path
            })
            .collect()
    }

    #[test]
    fn matches_come_newest_first_across_and_within_files() {
        let files = temp_files(
            "order",
            &["early panic\nok\nlate panic\n", "old panic\nnothing else\n"],
        );
        let matcher = Matcher::build("panic", false).unwrap();

        let found = search_files(&files, &matcher, 100, 0).unwrap();

        let texts: Vec<&str> = found.matches.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, ["late panic", "early panic", "old panic"]);
        assert_eq!(found.matches[0].line, 3);
        assert_eq!(found.matches[2].file, files[1]);
        assert!(!found.truncated);
        assert_eq!(found.files, 2);
    }

    // The cap keeps the newest matches and still reports how much more there
    // was, so a client knows the answer is a window and not everything.
    #[test]
    fn cap_keeps_the_newest_and_reports_truncation() {
        let files = temp_files("cap", &["hit 1\nhit 2\nhit 3\n"]);
        let matcher = Matcher::build("hit", false).unwrap();

        let found = search_files(&files, &matcher, 2, 0).unwrap();

        let texts: Vec<&str> = found.matches.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, ["hit 3", "hit 2"]);
        assert!(found.truncated);
    }

    #[test]
    fn context_surrounds_the_match_and_stops_at_file_edges() {
        let files = temp_files("context", &["one\ntwo\npanic\nfour\n"]);
        let matcher = Matcher::build("panic", false).unwrap();

        let found = search_files(&files, &matcher, 100, 2).unwrap();

        assert_eq!(found.matches[0].before, ["one", "two"]);
        assert_eq!(found.matches[0].after, ["four"]);
    }

    #[test]
    fn regex_matching_works_on_lines() {
        let files = temp_files("regex", &["timeout (T2) 61 sec\ntimeout pending\n"]);
        let matcher = Matcher::build(r"\(T\d\)", true).unwrap();

        let found = search_files(&files, &matcher, 100, 0).unwrap();

        assert_eq!(found.matches.len(), 1);
        assert_eq!(found.matches[0].text, "timeout (T2) 61 sec");
    }
}
