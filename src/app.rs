//! Pager state and key handling.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::input::{Column, Data, Document, digest};
use crate::json::{self, Layout};

/// How many columns a single horizontal step moves.
const HSTEP: usize = 30;

pub struct App {
    pub doc: Document,
    /// Which columns are showing their readable form rather than the one psql
    /// printed. It cuts both ways: JSON grows, spread over several lines, while
    /// a `bytea` blob shrinks to its size and checksum.
    pub readable: Vec<bool>,
    /// Display width of each column as things stand, which depends on which
    /// form each is being shown in.
    widths: Vec<usize>,
    /// Line each row starts on, with a final entry for the total. Rows are one
    /// line tall until some JSON is pretty-printed.
    offsets: Vec<usize>,
    /// Topmost visible line of the body. Lines, not rows: a pretty-printed row
    /// is taller than the screen often enough that paging by row would strand
    /// its bottom half.
    pub top: usize,
    /// Display column of the leftmost visible cell.
    pub left: usize,
    /// Height in lines of the row viewport, updated on every draw.
    pub view_height: usize,
    /// Width in columns of the text viewport, updated on every draw.
    pub view_width: usize,
    pub should_quit: bool,
}

impl App {
    pub fn new(doc: Document) -> Self {
        let mut app = Self {
            readable: vec![false; doc.columns.len()],
            doc,
            widths: Vec::new(),
            offsets: Vec::new(),
            top: 0,
            left: 0,
            view_height: 1,
            view_width: 1,
            should_quit: false,
        };

        app.relayout();
        app
    }

    /// Measure the table again after a column changed form.
    fn relayout(&mut self) {
        self.widths = (0..self.doc.columns.len())
            .map(|i| self.column_width(i))
            .collect();

        let mut line = 0;
        self.offsets = std::iter::once(0)
            .chain(self.doc.rows.iter().map(|row| {
                line += self.row_height(row);
                line
            }))
            .collect();
    }

    fn column_width(&self, index: usize) -> usize {
        let column = &self.doc.columns[index];
        // The document measured every column from the form psql printed, which
        // is still the one being shown.
        if !self.readable[index] {
            return column.width;
        }

        self.doc
            .rows
            .iter()
            .map(|row| match &row[index] {
                Data::Json(v) => json::measure(v, Layout::Pretty).1,
                Data::Bytes(b) => crate::input::display_width(&digest(b)),
                other => crate::input::display_width(&other.to_string()),
            })
            .chain([crate::input::display_width(&column.name)])
            .max()
            .unwrap_or(0)
    }

    /// Only pretty-printed JSON takes more than a line; a blob's digest is one
    /// line however big the blob is.
    fn row_height(&self, row: &[Data]) -> usize {
        row.iter()
            .zip(&self.readable)
            .map(|(cell, &readable)| match cell {
                Data::Json(v) if readable => json::measure(v, Layout::Pretty).0,
                _ => 1,
            })
            .max()
            .unwrap_or(1)
    }

    pub fn width(&self, column: usize) -> usize {
        self.widths[column]
    }

    /// Total display width of the laid-out table: every column padded by a
    /// space either side, joined by a separator.
    pub fn table_width(&self) -> usize {
        let cells: usize = self.widths.iter().map(|w| w + 2).sum();
        cells + self.widths.len().saturating_sub(1)
    }

    fn total_lines(&self) -> usize {
        self.offsets.last().copied().unwrap_or(0)
    }

    /// The row that `line` falls in, and how far into it.
    pub fn row_at(&self, line: usize) -> (usize, usize) {
        let row = self
            .offsets
            .partition_point(|&start| start <= line)
            .saturating_sub(1)
            .min(self.doc.rows.len().saturating_sub(1));

        (row, line.saturating_sub(self.offsets[row]))
    }

    /// The column covering `display_col`, or the first one to its right if it
    /// falls on a separator.
    pub fn column_at(&self, display_col: usize) -> Option<&Column> {
        let mut x = 0usize;
        self.doc.columns.iter().enumerate().find_map(|(i, column)| {
            x += self.widths[i] + 3;
            (x - 1 > display_col).then_some(column)
        })
    }

    pub fn has_readable(&self) -> bool {
        self.doc.columns.iter().any(|c| c.kind.has_readable_form())
    }

    /// Switch every column that has a readable form into it, or back to the
    /// form psql printed.
    ///
    /// They move together: with no cursor there is no one column to mean, and
    /// one keypress to make a screenful legible beats hunting column by column.
    fn toggle_readable(&mut self) {
        let on = !self.readable.iter().any(|&r| r);
        let (anchor, _) = self.row_at(self.top);

        for (i, column) in self.doc.columns.iter().enumerate() {
            self.readable[i] = on && column.kind.has_readable_form();
        }

        self.relayout();

        // Keep the row that was at the top of the screen there.
        self.top = self.offsets[anchor];
        self.clamp();
    }

    /// Largest valid `top`: scrolling stops with the last line at the bottom.
    fn max_top(&self) -> usize {
        self.total_lines().saturating_sub(self.view_height)
    }

    /// Largest valid `left`: scrolling stops with the last column flush right.
    fn max_left(&self) -> usize {
        self.table_width().saturating_sub(self.view_width)
    }

    fn scroll_vertical(&mut self, delta: isize) {
        self.top = self.top.saturating_add_signed(delta).min(self.max_top());
    }

    fn scroll_horizontal(&mut self, delta: isize) {
        self.left = self.left.saturating_add_signed(delta).min(self.max_left());
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        // Ignore key-release/repeat reports from terminals that send them.
        if key.kind != KeyEventKind::Press {
            return;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.view_height.saturating_sub(1).max(1) as isize;

        match key.code {
            KeyCode::Char('c' | 'd') if ctrl => self.should_quit = true,
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,

            KeyCode::Down | KeyCode::Char('j') | KeyCode::Enter => self.scroll_vertical(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_vertical(-1),
            KeyCode::Right | KeyCode::Char('l') => self.scroll_horizontal(HSTEP as isize),
            KeyCode::Left | KeyCode::Char('h') => self.scroll_horizontal(-(HSTEP as isize)),

            KeyCode::PageDown | KeyCode::Char(' ' | 'f') => self.scroll_vertical(page),
            KeyCode::PageUp | KeyCode::Char('b') => self.scroll_vertical(-page),
            KeyCode::Char('d') => self.scroll_vertical(page / 2),
            KeyCode::Char('u') => self.scroll_vertical(-page / 2),

            KeyCode::Home | KeyCode::Char('g') => self.top = 0,
            KeyCode::End | KeyCode::Char('G') => self.top = self.max_top(),
            KeyCode::Char('0' | '^') => self.left = 0,
            KeyCode::Char('$') => self.left = self.max_left(),

            KeyCode::Char('x') => self.toggle_readable(),

            _ => {}
        }
    }

    /// Re-clamp after a resize changed the viewport.
    pub fn clamp(&mut self) {
        self.top = self.top.min(self.max_top());
        self.left = self.left.min(self.max_left());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Document, psql};

    /// A one-column table of `rows` rows, laid out `width` columns wide plus
    /// the space either side of the cells.
    fn app(rows: usize, width: usize) -> App {
        let name = "n".repeat(width);
        let ids: Vec<String> = (0..rows).map(|i| i.to_string()).collect();

        let mut table = vec![vec![name.as_str()]];
        table.extend(ids.iter().map(|id| vec![id.as_str()]));

        let mut app = App::new(Document::from_str(&psql(&table)).expect("a table"));
        app.view_height = 10;
        app.view_width = 10;
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn vertical_scroll_stops_at_last_page() {
        let mut a = app(25, 5);
        press(&mut a, KeyCode::End);
        assert_eq!(a.top, 15);
        press(&mut a, KeyCode::Down);
        assert_eq!(a.top, 15);
        press(&mut a, KeyCode::Up);
        assert_eq!(a.top, 14);
    }

    #[test]
    fn does_not_scroll_when_content_fits() {
        let mut a = app(3, 5);
        press(&mut a, KeyCode::Down);
        press(&mut a, KeyCode::Right);
        assert_eq!((a.top, a.left), (0, 0));
    }

    #[test]
    fn horizontal_scroll_clamps_to_last_column() {
        // 14 columns of cell plus a space either side, against a 10 wide view.
        let mut a = app(3, 14);
        press(&mut a, KeyCode::Char('$'));
        assert_eq!(a.left, 6);
        press(&mut a, KeyCode::Right);
        assert_eq!(a.left, 6);
        press(&mut a, KeyCode::Char('0'));
        assert_eq!(a.left, 0);
        press(&mut a, KeyCode::Left);
        assert_eq!(a.left, 0);
    }

    /// Two rows of one JSON column, the second value twice the size of the
    /// first.
    fn json_app() -> App {
        let text = psql(&[
            vec!["id", "doc"],
            vec!["1", r#"{"a": 1}"#],
            vec!["2", r#"{"a": 1, "b": 2}"#],
        ]);

        let mut app = App::new(Document::from_str(&text).expect("a table"));
        app.view_height = 10;
        app.view_width = 40;
        app
    }

    #[test]
    fn switches_json_between_its_two_forms() {
        let mut a = json_app();
        assert!(a.has_readable());

        // Folded: one line per row, and the column is as wide as the widest
        // value written on one line.
        assert_eq!((a.row_at(0), a.row_at(1)), ((0, 0), (1, 0)));
        assert_eq!(a.width(1), r#"{"a":1,"b":2}"#.len());

        press(&mut a, KeyCode::Char('x'));

        // Expanded: three lines for the one-key value, four for the two-key
        // one, and the column is only as wide as one indented line.
        assert_eq!(a.readable, [false, true]);
        assert_eq!((a.row_at(2), a.row_at(3)), ((0, 2), (1, 0)));
        assert_eq!(a.row_at(6), (1, 3));
        assert_eq!(a.width(1), r#"  "a": 1,"#.len());

        press(&mut a, KeyCode::Char('x'));

        assert_eq!(a.readable, [false, false]);
        assert_eq!(a.row_at(1), (1, 0));
    }

    /// One `x` has to move the JSON and the blobs at once, in opposite
    /// directions: the JSON opens out over several lines while the blob
    /// collapses from its hex to a digest.
    #[test]
    fn one_key_switches_json_and_blobs_together() {
        let long = format!("\\x{}", "ab".repeat(32));
        let short = format!("\\x{}", "cd".repeat(8));
        let text = psql(&[
            vec!["id", "doc", "blob"],
            vec!["1", r#"{"a": 1}"#, &long],
            vec!["2", r#"{"b": 22}"#, &short],
        ]);

        let mut a = App::new(Document::from_str(&text).expect("a table"));
        a.view_height = 10;
        a.view_width = 40;

        // Raw: compact JSON, and hex at two characters a byte plus the `\x`.
        assert_eq!(a.readable, [false, false, false]);
        assert_eq!((a.width(1), a.width(2)), (8, 66));
        assert_eq!(a.row_at(1), (1, 0));

        press(&mut a, KeyCode::Char('x'));

        // `id` has no second form, so it is left alone.
        assert_eq!(a.readable, [false, true, true]);
        // The JSON column is now as wide as `  "b": 22`, three lines a row.
        assert_eq!(a.width(1), 9);
        assert_eq!(a.row_at(3), (1, 0));
        // The blob column is down to `32 B  ` and eight characters of md5 —
        // from 66 columns to 14, and still one line tall.
        assert_eq!(a.width(2), 14);
        assert_eq!(a.row_height(&a.doc.rows[0]), 3);
    }

    #[test]
    fn changing_form_keeps_the_top_row_in_place() {
        let mut a = json_app();
        a.view_height = 1;
        press(&mut a, KeyCode::Down);
        assert_eq!(a.row_at(a.top).0, 1);

        press(&mut a, KeyCode::Char('x'));

        // Row 1 starts on line 3 now, but it is still the row at the top.
        assert_eq!(a.top, 3);
        assert_eq!(a.row_at(a.top), (1, 0));
    }

    #[test]
    fn resize_reclamps() {
        let mut a = app(25, 5);
        press(&mut a, KeyCode::End);
        a.view_height = 25;
        a.clamp();
        assert_eq!(a.top, 0);
    }
}
