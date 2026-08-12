//! Reading the document to page through.

use std::fs::File;
use std::io::{self, Read};

use unicode_width::UnicodeWidthChar;

/// Tab stop used when expanding `\t`.
const TAB_STOP: usize = 8;

/// A document is just a list of display-ready lines plus the widest line's
/// display width (used to clamp horizontal scrolling).
pub struct Document {
    pub lines: Vec<String>,
    pub max_width: usize,
}

impl Document {
    pub fn from_reader(mut reader: impl Read) -> io::Result<Self> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        Ok(Self::from_str(&String::from_utf8_lossy(&buf)))
    }

    pub fn from_path(path: &str) -> io::Result<Self> {
        Self::from_reader(File::open(path)?)
    }

    pub fn from_str(text: &str) -> Self {
        let mut lines: Vec<String> = text
            .split('\n')
            .map(|line| sanitize(line.strip_suffix('\r').unwrap_or(line)))
            .collect();

        // A trailing newline produces a final empty element; drop it.
        if lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }

        let max_width = lines.iter().map(|l| display_width(l)).max().unwrap_or(0);

        Self { lines, max_width }
    }
}

/// Expand tabs and drop control characters.
///
/// TODO: ANSI escape sequences are stripped along with other control
/// characters, which leaves their parameter bytes behind (e.g. `[0m`). Once we
/// care about styled input, parse them into ratatui spans instead.
fn sanitize(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut col = 0usize;

    for ch in line.chars() {
        match ch {
            '\t' => {
                let pad = TAB_STOP - (col % TAB_STOP);
                out.extend(std::iter::repeat_n(' ', pad));
                col += pad;
            }
            c if c.is_control() => {}
            c => {
                out.push(c);
                col += c.width().unwrap_or(0);
            }
        }
    }

    out
}

pub fn display_width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Take the slice of `s` covering display columns `[start, start + max)`.
///
/// Wide characters straddling either edge are replaced by spaces so the result
/// always occupies exactly the columns it claims to.
pub fn slice_columns(s: &str, start: usize, max: usize) -> String {
    let mut out = String::new();
    let mut col = 0usize; // display column of the next input char
    let mut used = 0usize; // display columns already emitted

    if max == 0 {
        return out;
    }

    for ch in s.chars() {
        let w = ch.width().unwrap_or(0);
        let end = col + w;

        if end <= start {
            col = end;
            continue;
        }

        if col < start {
            // Straddles the left edge: emit spaces for the visible half.
            let visible = (end - start).min(max - used);
            out.extend(std::iter::repeat_n(' ', visible));
            used += visible;
        } else if used + w > max {
            // Straddles the right edge: pad and stop.
            out.extend(std::iter::repeat_n(' ', max - used));
            used = max;
        } else {
            out.push(ch);
            used += w;
        }

        col = end;
        if used >= max {
            break;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_ascii() {
        assert_eq!(slice_columns("abcdef", 2, 3), "cde");
        assert_eq!(slice_columns("abcdef", 0, 100), "abcdef");
        assert_eq!(slice_columns("abcdef", 10, 3), "");
        assert_eq!(slice_columns("abcdef", 2, 0), "");
    }

    #[test]
    fn pads_straddling_wide_chars() {
        // "あ" is two columns wide.
        assert_eq!(slice_columns("あい", 1, 3), " い");
        assert_eq!(slice_columns("あい", 0, 3), "あ ");
    }

    #[test]
    fn expands_tabs_and_drops_controls() {
        assert_eq!(sanitize("a\tb"), "a       b");
        assert_eq!(sanitize("a\x07b"), "ab");
    }

    #[test]
    fn parses_document() {
        let doc = Document::from_str("ab\nlonger\n");
        assert_eq!(doc.lines, vec!["ab", "longer"]);
        assert_eq!(doc.max_width, 6);
    }
}
