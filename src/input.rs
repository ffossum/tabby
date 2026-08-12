//! Reading the table to page through.
//!
//! Input is psql's aligned output. It is parsed once, up front, into typed
//! cells: after this module nothing looks at the original text, and the screen
//! is laid out from the values themselves.

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::ops::Range;
use std::str::FromStr;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime};
use rust_decimal::Decimal;
use unicode_width::UnicodeWidthChar;
use uuid::Uuid;

/// Tab stop used when expanding `\t`.
const TAB_STOP: usize = 8;

/// A table of parsed cells.
///
/// Every row has one entry per column: a row cut short by a ragged line is
/// padded with [`Data::Null`], so `rows[r][c]` is always the cell of column
/// `c`.
pub struct Document {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Data>>,
}

pub struct Column {
    pub name: String,
    /// What the cells of this column hold, or [`DataKind::Text`] if they
    /// disagree.
    pub kind: DataKind,
    /// Display width of the widest cell, the name included.
    pub width: usize,
}

/// One parsed cell.
///
/// Values that cannot be represented exactly stay [`Data::Text`], so nothing is
/// ever shown with digits it did not have. Timestamps and JSON are the
/// exception: they are re-rendered in a canonical form, which can differ
/// cosmetically from psql's (`+00:00` for the offset, no spaces in JSON).
#[derive(Debug, Clone, PartialEq)]
pub enum Data {
    Null,
    Boolean(bool),
    Integer(i64),
    Decimal(Decimal),
    Date(NaiveDate),
    Timestamp(NaiveDateTime),
    TimestampTz(DateTime<FixedOffset>),
    Uuid(Uuid),
    Json(serde_json::Value),
    /// A `bytea` hex literal, e.g. `\x00000186a3`.
    Bytes(Vec<u8>),
    Text(String),
}

/// The variants of [`Data`] without their payloads, used to describe a whole
/// column and to pick a colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataKind {
    Null,
    Boolean,
    Integer,
    Decimal,
    Date,
    Timestamp,
    TimestampTz,
    Uuid,
    Json,
    Bytes,
    Text,
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// The input has no `----+----` rule, so there is no table to show.
    NotATable,
}

impl Document {
    pub fn from_reader(mut reader: impl Read) -> Result<Self, Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        Self::from_str(&String::from_utf8_lossy(&buf))
    }

    pub fn from_path(path: &str) -> Result<Self, Error> {
        Self::from_reader(File::open(path)?)
    }

    /// Parse psql's aligned output.
    ///
    /// psql prints ` a | b ` over `---+---`, so the `+` in the rule mark where
    /// the fields end and each field carries one space of padding on either
    /// side. Anything above the header or below the last row — blank lines,
    /// `(13 rows)`, timings — is dropped.
    pub fn from_str(text: &str) -> Result<Self, Error> {
        let lines: Vec<String> = text
            .split('\n')
            .map(|line| sanitize(line.strip_suffix('\r').unwrap_or(line)))
            .collect();

        let rule = lines
            .iter()
            .position(|l| is_rule(l))
            .ok_or(Error::NotATable)?;
        let header = &lines[rule.checked_sub(1).ok_or(Error::NotATable)?];

        // A rule is `-` and `+` only, so its byte offsets are display columns.
        let separators: Vec<usize> = lines[rule].match_indices('+').map(|(i, _)| i).collect();
        if !separators_line_up(header, &separators) {
            return Err(Error::NotATable);
        }
        let spans = field_spans(lines[rule].len(), &separators);

        // Rows run until the blank line or `(13 rows)` psql puts underneath.
        let rows: Vec<Vec<Data>> = lines[rule + 1..]
            .iter()
            .take_while(|l| !l.is_empty() && !is_row_count(l))
            .map(|line| spans.iter().map(|s| Data::parse(cell(line, s))).collect())
            .collect();

        let columns = spans
            .iter()
            .enumerate()
            .map(|(i, span)| {
                let name = cell(header, span).to_string();
                Column {
                    kind: rows.iter().map(|r| r[i].kind()).fold(DataKind::Null, merge),
                    width: rows
                        .iter()
                        .map(|r| display_width(&r[i].to_string()))
                        .chain([display_width(&name)])
                        .max()
                        .unwrap_or(0),
                    name,
                }
            })
            .collect();

        Ok(Self { columns, rows })
    }
}

impl Data {
    /// Recognise one cell of psql output.
    ///
    /// Numbers are only taken as numbers when they print back exactly as they
    /// came in, so `007`, `1e400` and a 40-digit `numeric` stay text rather
    /// than being quietly rounded or reshaped.
    fn parse(text: &str) -> Self {
        if text.is_empty() {
            return Self::Null;
        }

        if let Some(b) = parse_bool(text) {
            return Self::Boolean(b);
        }
        if let Some(bytes) = parse_bytea(text) {
            return Self::Bytes(bytes);
        }
        if let Ok(i) = text.parse::<i64>()
            && i.to_string() == text
        {
            return Self::Integer(i);
        }
        if let Ok(d) = Decimal::from_str_exact(text)
            && d.to_string() == text
        {
            return Self::Decimal(d);
        }
        if let Ok(u) = Uuid::try_parse(text)
            && u.to_string() == text
        {
            return Self::Uuid(u);
        }
        if let Ok(ts) = DateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f%#z") {
            return Self::TimestampTz(ts);
        }
        if let Ok(ts) = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f") {
            return Self::Timestamp(ts);
        }
        if let Ok(d) = NaiveDate::from_str(text) {
            return Self::Date(d);
        }
        // Only objects and arrays: a bare `123` is a number, not JSON.
        if (text.starts_with('{') || text.starts_with('['))
            && let Ok(v) = serde_json::from_str(text)
        {
            return Self::Json(v);
        }

        Self::Text(text.to_string())
    }

    pub fn kind(&self) -> DataKind {
        match self {
            Self::Null => DataKind::Null,
            Self::Boolean(_) => DataKind::Boolean,
            Self::Integer(_) => DataKind::Integer,
            Self::Decimal(_) => DataKind::Decimal,
            Self::Date(_) => DataKind::Date,
            Self::Timestamp(_) => DataKind::Timestamp,
            Self::TimestampTz(_) => DataKind::TimestampTz,
            Self::Uuid(_) => DataKind::Uuid,
            Self::Json(_) => DataKind::Json,
            Self::Bytes(_) => DataKind::Bytes,
            Self::Text(_) => DataKind::Text,
        }
    }
}

/// How a cell is written out again. NULL renders as nothing, the way psql
/// prints it.
impl fmt::Display for Data {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => Ok(()),
            Self::Boolean(b) => f.write_str(if *b { "t" } else { "f" }),
            Self::Integer(i) => write!(f, "{i}"),
            Self::Decimal(d) => write!(f, "{d}"),
            Self::Date(d) => write!(f, "{}", d.format("%Y-%m-%d")),
            Self::Timestamp(t) => write!(f, "{}", t.format("%Y-%m-%d %H:%M:%S%.f")),
            Self::TimestampTz(t) => write!(f, "{}", t.format("%Y-%m-%d %H:%M:%S%.f%:z")),
            Self::Uuid(u) => write!(f, "{u}"),
            Self::Json(v) => write!(f, "{v}"),
            Self::Bytes(bytes) => {
                f.write_str("\\x")?;
                bytes.iter().try_for_each(|b| write!(f, "{b:02x}"))
            }
            Self::Text(s) => f.write_str(s),
        }
    }
}

impl DataKind {
    /// Numbers are right-aligned, as psql aligns them.
    pub fn is_numeric(self) -> bool {
        matches!(self, Self::Integer | Self::Decimal)
    }
}

/// A column's type is whatever its cells agree on. NULLs abstain, and whole
/// numbers among decimals still make a decimal column.
fn merge(a: DataKind, b: DataKind) -> DataKind {
    use DataKind::{Decimal, Integer, Null};

    match (a, b) {
        (Null, other) | (other, Null) => other,
        _ if a == b => a,
        (Integer, Decimal) | (Decimal, Integer) => Decimal,
        _ => DataKind::Text,
    }
}

fn parse_bool(text: &str) -> Option<bool> {
    match text {
        "t" => Some(true),
        "f" => Some(false),
        _ if text.eq_ignore_ascii_case("true") => Some(true),
        _ if text.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

fn parse_bytea(text: &str) -> Option<Vec<u8>> {
    let hex = text.strip_prefix("\\x")?;
    if hex.is_empty() || hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }

    // Lowercase only: psql prints bytea that way, and re-encoding an uppercase
    // literal would change it.
    if hex.bytes().any(|b| b.is_ascii_uppercase()) {
        return None;
    }

    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// A line of only `-` and `+`, with at least one `-`.
fn is_rule(line: &str) -> bool {
    !line.is_empty()
        && line.bytes().all(|b| b == b'-' || b == b'+')
        && line.bytes().any(|b| b == b'-')
}

/// psql's row count footer, e.g. `(13 rows)`.
fn is_row_count(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('(') && (line.ends_with(" row)") || line.ends_with(" rows)"))
}

/// The display columns each field covers, given where the separators sit.
fn field_spans(rule_width: usize, separators: &[usize]) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut start = 0usize;

    for &end in separators.iter().chain(&[rule_width]) {
        // Drop the one space of padding on either side. A field too narrow to
        // have any yields an empty span, which reads as NULL.
        let from = (start + 1).min(end);
        spans.push(from..end.saturating_sub(1).max(from));
        start = end + 1;
    }

    spans
}

/// Check the header has a `|` above every separator, which is what tells a real
/// table apart from prose that happens to sit above a line of dashes.
fn separators_line_up(header: &str, separators: &[usize]) -> bool {
    separators
        .iter()
        .all(|&col| header[byte_at_column(header, col)..].starts_with('|'))
}

/// The text of one field of `line`, padding trimmed off.
fn cell<'a>(line: &'a str, span: &Range<usize>) -> &'a str {
    let start = byte_at_column(line, span.start);
    let end = byte_at_column(line, span.end);
    line[start..end].trim()
}

/// Byte offset of the first character at or after display column `col`.
fn byte_at_column(s: &str, col: usize) -> usize {
    let mut at = 0usize;

    for (i, ch) in s.char_indices() {
        if at >= col {
            return i;
        }
        at += ch.width().unwrap_or(0);
    }

    s.len()
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

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::NotATable => {
                f.write_str("input is not an aligned table (expected psql's ----+---- output)")
            }
        }
    }
}

impl std::error::Error for Error {}

/// Lay cells out the way psql would, for tests that need input to parse.
#[cfg(test)]
pub fn psql(rows: &[Vec<&str>]) -> String {
    let widths: Vec<usize> = (0..rows[0].len())
        .map(|i| rows.iter().map(|r| display_width(r[i])).max().unwrap_or(0))
        .collect();

    let line = |cells: &Vec<&str>| {
        cells
            .iter()
            .zip(&widths)
            .map(|(c, w)| format!(" {c:<w$} "))
            .collect::<Vec<_>>()
            .join("|")
    };

    let rule: Vec<String> = widths.iter().map(|w| "-".repeat(w + 2)).collect();
    let body: Vec<String> = rows[1..].iter().map(line).collect();

    format!(
        "{}\n{}\n{}\n({} rows)\n\n",
        line(&rows[0]),
        rule.join("+"),
        body.join("\n"),
        body.len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Document {
        let text = psql(&[
            vec!["id", "name", "ok", "ratio", "seen"],
            vec!["1", "alice", "t", "0.25", "2025-08-27 07:07:35.502286+00"],
            vec!["22", "bob", "f", "3.00", "2026-06-10 08:30:00+00"],
            vec!["333", "", "t", "-1.5", "2026-08-11 13:00:23+00"],
        ]);

        Document::from_str(&text).expect("expected a table")
    }

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
    fn reads_names_and_rows() {
        let doc = doc();

        let names: Vec<&str> = doc.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["id", "name", "ok", "ratio", "seen"]);

        // Three rows: the footer and the blank line after it are not data.
        assert_eq!(doc.rows.len(), 3);
        assert_eq!(doc.rows[0][0], Data::Integer(1));
        assert_eq!(doc.rows[0][1], Data::Text("alice".into()));
        assert_eq!(doc.rows[0][2], Data::Boolean(true));
        // An empty field is NULL.
        assert_eq!(doc.rows[2][1], Data::Null);
    }

    #[test]
    fn infers_column_kinds() {
        let kinds: Vec<DataKind> = doc().columns.iter().map(|c| c.kind).collect();

        assert_eq!(
            kinds,
            [
                DataKind::Integer,
                // NULL in the column does not make it a mixed one.
                DataKind::Text,
                DataKind::Boolean,
                DataKind::Decimal,
                DataKind::TimestampTz,
            ]
        );
    }

    #[test]
    fn measures_columns() {
        let doc = doc();

        // The widest of `id`, `1`, `22`, `333`.
        assert_eq!(doc.columns[0].width, 3);
        // Widths measure the cells as we render them, not as psql printed
        // them: the offset comes back as `+00:00`, three columns wider.
        assert_eq!(
            doc.columns[4].width,
            display_width("2025-08-27 07:07:35.502286+00:00")
        );
    }

    #[test]
    fn keeps_decimals_exact() {
        // Trailing zeros survive: this is `numeric`, not a float.
        assert_eq!(Data::parse("3.00").to_string(), "3.00");
        assert_eq!(Data::parse("-1.5"), Data::Decimal(Decimal::new(-15, 1)));
        // Anything that would not print back the same stays text.
        assert_eq!(Data::parse("007"), Data::Text("007".into()));
        assert_eq!(Data::parse("+1"), Data::Text("+1".into()));
        assert_eq!(Data::parse("1e400"), Data::Text("1e400".into()));
        let huge = "123456789012345678901234567890123456789";
        assert_eq!(Data::parse(huge), Data::Text(huge.into()));
    }

    #[test]
    fn parses_postgres_scalars() {
        let cases = [
            ("3008", DataKind::Integer),
            ("f", DataKind::Boolean),
            ("\\x00000186a3", DataKind::Bytes),
            ("2b1f9c4e-0000-4a1b-8c2d-1a2b3c4d5e6f", DataKind::Uuid),
            ("2025-08-27", DataKind::Date),
            ("2025-08-27 07:07:35.502286", DataKind::Timestamp),
            ("2025-08-27 07:07:35.502286+00", DataKind::TimestampTz),
            (r#"{"kam": -999}"#, DataKind::Json),
            ("Fredrik's Bank", DataKind::Text),
            ("{not json", DataKind::Text),
        ];

        for (text, kind) in cases {
            assert_eq!(Data::parse(text).kind(), kind, "parsing {text:?}");
        }
    }

    #[test]
    fn renders_cells_back() {
        // Exact round-trips.
        for text in ["3008", "f", "\\x00000186a3", "2025-08-27", "Fredrik's Bank"] {
            assert_eq!(Data::parse(text).to_string(), text);
        }

        // Canonicalised: a full offset, and JSON without psql's spacing but
        // with its key order.
        assert_eq!(
            Data::parse("2025-08-27 07:07:35.502286+00").to_string(),
            "2025-08-27 07:07:35.502286+00:00"
        );
        assert_eq!(
            Data::parse(r#"{"kam": -999, "active": false}"#).to_string(),
            r#"{"kam":-999,"active":false}"#
        );
        assert_eq!(Data::Null.to_string(), "");
    }

    #[test]
    fn rejects_input_that_is_not_a_table() {
        // A rule with no header above it, and prose that has neither.
        for text in ["---+---\n 1 | 2\n", "just some prose\n", ""] {
            assert!(matches!(Document::from_str(text), Err(Error::NotATable)));
        }
    }

    #[test]
    fn reads_a_single_column_table() {
        let doc = Document::from_str(" count \n-------\n     7 \n(1 row)\n").unwrap();

        assert_eq!(doc.columns.len(), 1);
        assert_eq!(doc.rows, [[Data::Integer(7)]]);
    }
}
