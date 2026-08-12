//! JSON cells, laid out by `colored_json`.
//!
//! `colored_json` writes ANSI escapes, so the coloured form is handed to
//! `ansi-to-tui` to come back as styled spans. Both forms go through the same
//! formatter, so the plain one — which is what column widths and row heights
//! are measured from — always has the same line breaks as what is drawn.

use ansi_to_tui::IntoText;
use colored_json::{ColorMode, ColoredFormatter, CompactFormatter, PrettyFormatter};
use ratatui::text::Text;
use serde_json::Value;

/// How a JSON cell is written out: on one line, or indented over several.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Compact,
    Pretty,
}

/// The uncoloured text, for measuring.
pub fn plain(value: &Value, layout: Layout) -> String {
    format(value, layout, ColorMode::Off)
}

/// The same text with colours, for drawing.
pub fn colored(value: &Value, layout: Layout) -> Text<'static> {
    let ansi = format(value, layout, ColorMode::On);

    ansi.into_text()
        .unwrap_or_else(|_| Text::raw(plain(value, layout)))
}

/// Lines the value needs, and how wide the widest of them is.
pub fn measure(value: &Value, layout: Layout) -> (usize, usize) {
    let text = plain(value, layout);
    let lines = text.lines();

    (
        text.lines().count().max(1),
        lines.map(crate::input::display_width).max().unwrap_or(0),
    )
}

fn format(value: &Value, layout: Layout, mode: ColorMode) -> String {
    let json = match layout {
        Layout::Compact => ColoredFormatter::new(CompactFormatter {}).to_colored_json(value, mode),
        Layout::Pretty => {
            ColoredFormatter::new(PrettyFormatter::new()).to_colored_json(value, mode)
        }
    };

    // The value came from `serde_json`, so writing it back cannot really fail.
    json.unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value() -> Value {
        serde_json::json!({"kam": -999, "active": false})
    }

    #[test]
    fn lays_json_out_compact_or_pretty() {
        assert_eq!(
            plain(&value(), Layout::Compact),
            r#"{"kam":-999,"active":false}"#
        );
        assert_eq!(
            plain(&value(), Layout::Pretty),
            "{\n  \"kam\": -999,\n  \"active\": false\n}"
        );
    }

    #[test]
    fn measures_what_it_lays_out() {
        assert_eq!(measure(&value(), Layout::Compact), (1, 27));
        // Four lines, the widest being `  "active": false`.
        assert_eq!(measure(&value(), Layout::Pretty), (4, 17));
    }

    #[test]
    fn colours_without_changing_the_layout() {
        let text = colored(&value(), Layout::Pretty);
        let lines: Vec<String> = text
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        // Same text as `plain`, but split into spans that carry a style.
        assert_eq!(lines.join("\n"), plain(&value(), Layout::Pretty));
        assert!(text.lines[1].spans.len() > 1, "expected coloured spans");
    }
}
