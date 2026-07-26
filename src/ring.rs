//! The rolling receive buffer and the pattern matcher that scans it.

use memchr::memmem;

// At 115200 baud a console fills slowly, so a few hundred KB keeps enough
// scrollback that expect() can still find a line that scrolled past a moment
// ago. The buffer can grow to twice this before it is trimmed.
pub const DEFAULT_RING_CAP: usize = 512 * 1024;

/// A rolling window of recently received bytes, addressed by an absolute offset
/// so a client can page through with a cursor even after old bytes are dropped.
pub struct Ring {
    buf:  Vec<u8>,
    base: u64, // absolute offset of buf[0]
    cap:  usize,
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: Vec::new(),
            base: 0,
            cap: cap.max(1),
        }
    }

    pub fn total(&self) -> u64 {
        self.base + self.buf.len() as u64
    }

    pub fn append(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        // Trim in large chunks. Draining on every append would shift the whole
        // buffer for each received chunk once full. Letting it grow to twice the
        // cap first makes the shift amortized O(1) per byte.
        if self.buf.len() > self.cap * 2 {
            let drop = self.buf.len() - self.cap;
            self.buf.drain(..drop);
            self.base += drop as u64;
        }
    }

    // Bytes from `cursor` to the end, plus the absolute offset the slice starts
    // at. `cursor` is clamped into the retained window, so a cursor pointing at
    // bytes already dropped simply starts at the oldest retained byte.
    pub fn slice_from(&self, cursor: u64) -> (u64, &[u8]) {
        let start = cursor.clamp(self.base, self.total());
        let idx = (start - self.base) as usize;
        (start, &self.buf[idx..])
    }

    pub fn base(&self) -> u64 {
        self.base
    }

    pub fn tail_lines(&self, lines: usize) -> String {
        if lines == 0 {
            return String::new();
        }
        // Scan backwards for the newline where the requested tail starts, so the
        // cost tracks the answer size instead of the whole retained buffer. A
        // trailing newline only terminates the last line, it does not start a
        // new one.
        let buf = &self.buf;
        let end = buf.len() - usize::from(buf.last() == Some(&b'\n'));
        let mut count = 0;
        let mut start = 0;
        for i in (0..end).rev() {
            if buf[i] == b'\n' {
                count += 1;
                if count == lines {
                    start = i + 1;
                    break;
                }
            }
        }
        let tail = String::from_utf8_lossy(&buf[start..]);
        let all: Vec<&str> = tail.lines().collect();
        all.join("\n")
    }
}

/// Either a plain substring or a compiled regex, matched against raw bytes so
/// the offsets it returns line up with the ring.
pub enum Matcher {
    Substr(Vec<u8>),
    Regex(regex::bytes::Regex),
}

impl Matcher {
    pub fn build(pattern: &str, regex: bool) -> Result<Self, String> {
        if regex {
            regex::bytes::Regex::new(pattern)
                .map(Matcher::Regex)
                .map_err(|e| e.to_string())
        } else {
            Ok(Matcher::Substr(pattern.as_bytes().to_vec()))
        }
    }

    // Offset just past the first match in `hay`, or None.
    pub fn find_end(&self, hay: &[u8]) -> Option<usize> {
        match self {
            Matcher::Substr(needle) => memmem::find(hay, needle).map(|i| i + needle.len()),
            Matcher::Regex(re) => re.find(hay).map(|m| m.end()),
        }
    }

    // Where the next scan can resume after a miss that ended at absolute offset
    // `end`. A substring hit can straddle the scanned region's edge by at most
    // needle length minus one, so scanning restarts just before it. A regex can
    // match a span of any length, so it rescans from `start` every time.
    pub fn resume_from(&self, start: u64, end: u64) -> u64 {
        match self {
            Matcher::Substr(needle) => {
                let overlap = needle.len().saturating_sub(1) as u64;
                end.saturating_sub(overlap).max(start)
            }
            Matcher::Regex(_) => start,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_drops_oldest_and_tracks_offset() {
        let mut ring = Ring::new(DEFAULT_RING_CAP);
        ring.append(b"hello ");
        ring.append(b"world");
        assert_eq!(ring.total(), 11);
        let (abs, slice) = ring.slice_from(6);
        assert_eq!(abs, 6);
        assert_eq!(slice, b"world");
    }

    #[test]
    fn ring_cursor_before_window_clamps_to_base() {
        let mut ring = Ring::new(DEFAULT_RING_CAP);
        ring.append(b"abc");
        let (abs, slice) = ring.slice_from(0);
        assert_eq!(abs, 0);
        assert_eq!(slice, b"abc");
    }

    // A small cap makes the trim path cheap to test. The window keeps the cap
    // after a trim, and the base moves by exactly what was dropped.
    #[test]
    fn ring_trims_to_cap_and_moves_base() {
        let mut ring = Ring::new(4);
        ring.append(&[b'x'; 9]);
        assert_eq!(ring.total(), 9);
        assert_eq!(ring.base(), 5);
        let (abs, slice) = ring.slice_from(0);
        assert_eq!(abs, 5);
        assert_eq!(slice.len(), 4);
    }

    #[test]
    fn tail_lines_returns_last_lines() {
        let mut ring = Ring::new(DEFAULT_RING_CAP);
        ring.append(b"one\r\ntwo\r\nthree\r\npartial");
        assert_eq!(ring.tail_lines(2), "three\npartial");
        assert_eq!(ring.tail_lines(10), "one\ntwo\nthree\npartial");
        assert_eq!(ring.tail_lines(0), "");
    }

    #[test]
    fn tail_lines_ignores_trailing_newline() {
        let mut ring = Ring::new(DEFAULT_RING_CAP);
        ring.append(b"a\nb\n");
        assert_eq!(ring.tail_lines(1), "b");
        assert_eq!(ring.tail_lines(2), "a\nb");
    }

    #[test]
    fn substr_match_returns_offset_past_match() {
        let m = Matcher::build("ready> ", false).unwrap();
        // "ready> " sits at bytes 11..18, so the offset just past it is 18.
        assert_eq!(m.find_end(b"value = 1\r\nready> "), Some(18));
        assert_eq!(m.find_end(b"still running"), None);
    }

    #[test]
    fn regex_match_finds_tagged_line() {
        let m = Matcher::build(r"\(T\d\)", true).unwrap();
        // "(T2)" sits at bytes 8..12, so the offset just past it is 12.
        assert_eq!(m.find_end(b"timeout (T2) 61 sec"), Some(12));
        assert_eq!(m.find_end(b"timeout pending"), None);
    }

    #[test]
    fn substr_scan_resumes_with_overlap_and_regex_rescans() {
        let sub = Matcher::build("abc", false).unwrap();
        assert_eq!(sub.resume_from(0, 100), 98);
        assert_eq!(sub.resume_from(99, 100), 99); // never before the start cursor
        let re = Matcher::build("a.*b", true).unwrap();
        assert_eq!(re.resume_from(5, 100), 5);
    }
}
