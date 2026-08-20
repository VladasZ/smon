//! Copying a clicked line to the clipboard.
//!
//! Two mechanisms run on every copy. OSC 52 asks the terminal to do it, which
//! survives ssh because the terminal doing the writing is the local one.
//! arboard writes the OS clipboard directly, for terminals without OSC 52
//! support. Whichever side works wins, and only both failing is an error.

use std::io::{Write, stdout};

use base64::{Engine, engine::general_purpose::STANDARD};

pub struct Clipboard {
    /// Kept for the whole session because on X11 the clipboard offer dies with
    /// the object that made it.
    os: Result<arboard::Clipboard, String>,
}

impl Clipboard {
    /// A failure to reach the OS clipboard is remembered, not fatal. Over ssh
    /// there is no OS clipboard to reach and OSC 52 carries the copy alone.
    pub fn new() -> Self {
        Self {
            os: arboard::Clipboard::new().map_err(|e| e.to_string()),
        }
    }

    /// # Errors
    /// Returns an error only when both mechanisms failed.
    pub fn copy(&mut self, text: &str) -> Result<(), String> {
        let terminal = write_osc52(text).map_err(|e| e.to_string());
        let os = match &mut self.os {
            Ok(clipboard) => clipboard.set_text(text).map_err(|e| e.to_string()),
            Err(e) => Err(e.clone()),
        };
        match (terminal, os) {
            (Err(terminal), Err(os)) => Err(format!("copy failed, osc52: {terminal}, clipboard: {os}")),
            _ => Ok(()),
        }
    }
}

fn write_osc52(text: &str) -> std::io::Result<()> {
    let mut out = stdout();
    out.write_all(osc52(text).as_bytes())?;
    out.flush()
}

/// The escape sequence asking the terminal to set its clipboard.
fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", STANDARD.encode(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_wraps_the_text_base64_encoded() {
        assert_eq!(osc52("hello"), "\x1b]52;c;aGVsbG8=\x07");
    }
}
