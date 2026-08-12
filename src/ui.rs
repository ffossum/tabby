//! Rendering.
//!
//! The table is laid out here rather than replayed from the input: each cell is
//! written out from its [`Data`], padded to its column's width, and the row is
//! then clipped to the horizontal viewport. A pretty-printed JSON cell covers
//! several lines, so a row is as tall as its tallest cell.

use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout as Areas};
use ratatui::style::{Color, Style};
use ratatui::symbols::line;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::input::{Data, DataKind, display_width, slice_columns};
use crate::json::{self, Layout};

/// The glyphs the grid is drawn with — the shape psql draws in ASCII, in box
/// characters. Swapping this for `ROUNDED`, `THICK` or `DOUBLE` re-skins it.
///
/// Every one of them is a single display column wide, so the layout arithmetic
/// is the same as it was for `-`, `+` and `|`.
const GRID: line::Set = line::NORMAL;

/// A piece of a laid-out line: its text and how to paint it.
type Segment = (String, Style);

/// The grid recedes so the values stand out.
fn grid_style() -> Style {
    Style::new().dark_gray()
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [table_area, status_area] =
        Areas::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());
    // The names and the rule under them stay put while the rows scroll.
    let [head_area, rows_area] =
        Areas::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(table_area);

    // Remember the viewport so scroll clamping matches what is on screen.
    app.view_height = rows_area.height as usize;
    app.view_width = table_area.width as usize;
    app.clamp();

    let view = app.left..app.left + app.view_width;
    let head = Text::from(vec![
        clip(header_segments(app), &view),
        clip(rule_segments(app), &view),
    ]);

    frame.render_widget(Paragraph::new(head), head_area);
    frame.render_widget(Paragraph::new(Text::from(body(app, &view))), rows_area);
    frame.render_widget(status_line(app), status_area);
}

/// Fill the viewport from `top`, which can start part-way down a tall row.
fn body(app: &App, view: &Range<usize>) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(app.view_height);
    let (mut row, mut skip) = app.row_at(app.top);

    while lines.len() < app.view_height && row < app.doc.rows.len() {
        for line in row_lines(app, row).into_iter().skip(skip) {
            if lines.len() == app.view_height {
                break;
            }
            lines.push(clip(line, view));
        }

        skip = 0;
        row += 1;
    }

    lines
}

fn header_segments(app: &App) -> Vec<Segment> {
    let names: Vec<Vec<Segment>> = app
        .doc
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| vec![(center(&c.name, app.width(i)), Style::new().bold())])
        .collect();

    join(&names)
}

/// The rule spans the padding as well, and crosses rather than meets.
fn rule_segments(app: &App) -> Vec<Segment> {
    let dashes: Vec<String> = (0..app.doc.columns.len())
        .map(|i| GRID.horizontal.repeat(app.width(i) + 2))
        .collect();

    vec![(dashes.join(GRID.cross), grid_style())]
}

/// Lay one row out, as one line per line of its tallest cell.
fn row_lines(app: &App, row: usize) -> Vec<Vec<Segment>> {
    let cells: Vec<Vec<Vec<Segment>>> = app.doc.rows[row]
        .iter()
        .enumerate()
        .map(|(i, cell)| cell_lines(app, i, cell))
        .collect();

    let height = cells.iter().map(Vec::len).max().unwrap_or(1);

    (0..height)
        .map(|line| {
            let row: Vec<Vec<Segment>> = cells
                .iter()
                .enumerate()
                .map(|(i, cell)| {
                    cell.get(line)
                        .cloned()
                        // A cell shorter than the row leaves its width blank.
                        .unwrap_or_else(|| vec![(" ".repeat(app.width(i)), Style::new())])
                })
                .collect();

            join(&row)
        })
        .collect()
}

/// Write one cell out, padded to its column, as one line or several.
fn cell_lines(app: &App, column: usize, cell: &Data) -> Vec<Vec<Segment>> {
    let width = app.width(column);

    let lines = match cell {
        // JSON keeps colored_json's own colours rather than the column's.
        Data::Json(value) if app.expanded[column] => spans(json::colored(value, Layout::Pretty)),
        Data::Json(value) => spans(json::colored(value, Layout::Compact)),
        other => {
            let numeric = app.doc.columns[column].kind.is_numeric();
            vec![vec![(
                pad(&other.to_string(), width, numeric),
                style(other.kind()),
            )]]
        }
    };

    lines.into_iter().map(|l| fill(l, width)).collect()
}

/// Take the styled spans `ansi-to-tui` produced back apart into segments.
fn spans(text: Text<'static>) -> Vec<Vec<Segment>> {
    text.lines
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| (span.content.into_owned(), span.style))
                .collect()
        })
        .collect()
}

/// Pad a laid-out cell out to its column's width.
fn fill(mut line: Vec<Segment>, width: usize) -> Vec<Segment> {
    let used: usize = line.iter().map(|(text, _)| display_width(text)).sum();

    if used < width {
        line.push((" ".repeat(width - used), Style::new()));
    }

    line
}

/// Glue one line of every column together with the padding and separators psql
/// puts between them.
fn join(cells: &[Vec<Segment>]) -> Vec<Segment> {
    let mut out = Vec::with_capacity(cells.len() * 4);

    for (i, cell) in cells.iter().enumerate() {
        if i > 0 {
            out.push((GRID.vertical.to_string(), grid_style()));
        }

        out.push((" ".to_string(), Style::new()));
        out.extend(cell.iter().cloned());
        out.push((" ".to_string(), Style::new()));
    }

    out
}

/// Keep the part of the line inside the horizontal viewport.
fn clip(segments: Vec<Segment>, view: &Range<usize>) -> Line<'static> {
    let mut spans = Vec::new();
    let mut x = 0usize; // display column the next segment starts at

    for (text, style) in segments {
        let width = display_width(&text);
        let start = x.max(view.start);
        let end = (x + width).min(view.end);

        if start < end {
            spans.push(Span::styled(
                slice_columns(&text, start - x, end - start),
                style,
            ));
        }

        x += width;
        if x >= view.end {
            break;
        }
    }

    Line::from(spans)
}

fn pad(text: &str, width: usize, right_align: bool) -> String {
    let fill = " ".repeat(width.saturating_sub(display_width(text)));

    if right_align {
        fill + text
    } else {
        text.to_string() + &fill
    }
}

fn center(text: &str, width: usize) -> String {
    let fill = width.saturating_sub(display_width(text));
    " ".repeat(fill / 2) + text + &" ".repeat(fill - fill / 2)
}

fn style(kind: DataKind) -> Style {
    match kind {
        DataKind::Null => Style::new().dark_gray(),
        DataKind::Boolean => Style::new().fg(Color::Green),
        DataKind::Integer | DataKind::Decimal => Style::new().fg(Color::Cyan),
        DataKind::Date | DataKind::Timestamp | DataKind::TimestampTz => {
            Style::new().fg(Color::Magenta)
        }
        DataKind::Uuid => Style::new().fg(Color::Blue),
        DataKind::Json => Style::new().fg(Color::Yellow),
        DataKind::Bytes => Style::new().dark_gray(),
        DataKind::Text => Style::new(),
    }
}

fn status_line(app: &App) -> Paragraph<'static> {
    let total = app.doc.rows.len();
    let last_line = (app.top + app.view_height).saturating_sub(1);
    let (first, last) = match total {
        0 => (0, 0),
        _ => (app.row_at(app.top).0 + 1, app.row_at(last_line).0 + 1),
    };
    let percent = if last == total {
        100
    } else {
        (last * 100) / total
    };

    // In a wide table the column name is more use than the column number.
    let column = app.column_at(app.left).map_or_else(
        || format!("col {}", app.left + 1),
        |c| format!("col {} {}", app.left + 1, c.name),
    );

    let json = if !app.has_json() {
        ""
    } else if app.expanded.iter().any(|&e| e) {
        "x:fold  "
    } else {
        "x:json  "
    };

    let text = format!(" {first}-{last}/{total} rows  {column}  {percent}%  {json}q:quit ");

    Paragraph::new(text).style(Style::new().reversed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Document, psql};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Draw into a `width` x `height` terminal and read the cells back as
    /// text. Two of those lines are the frozen head and one is the status.
    fn render(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    fn app() -> App {
        let text = psql(&[
            vec!["id", "name", "ratio"],
            vec!["1", "alice", "0.25"],
            vec!["22", "", "3.00"],
        ]);

        App::new(Document::from_str(&text).expect("a table"))
    }

    #[test]
    fn lays_the_table_out_from_the_values() {
        let lines = render(&mut app(), 24, 5);

        assert_eq!(
            lines[..4],
            [
                " id │ name  │ ratio   ",
                "────┼───────┼───────  ",
                "  1 │ alice │  0.25   ",
                " 22 │       │  3.00   ",
            ]
            .map(|l| format!("{l:<24}"))
        );
    }

    #[test]
    fn pretty_prints_an_expanded_json_column() {
        let text = psql(&[vec!["id", "doc"], vec!["1", r#"{"a": 1}"#], vec!["2", "{}"]]);
        let mut app = App::new(Document::from_str(&text).expect("a table"));

        // Folded, the value sits on the one line its row is tall.
        assert_eq!(
            render(&mut app, 20, 5)[2],
            format!("{:<20}", "  1 │ {\"a\":1}")
        );

        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        let lines = render(&mut app, 20, 6);

        assert_eq!(
            lines[..5],
            [
                " id │   doc    ",
                "────┼──────────",
                "  1 │ {        ",
                "    │   \"a\": 1 ",
                "    │ }        ",
            ]
            .map(|l| format!("{l:<20}"))
        );
    }

    #[test]
    fn clips_to_the_horizontal_viewport() {
        // The table is 20 wide, so in a 12 wide terminal it can scroll by 8.
        let mut app = app();
        app.left = 6;

        let lines = render(&mut app, 12, 5);

        assert_eq!(lines[0], "name  │ rati");
        assert_eq!(lines[2], "alice │  0.2");
    }
}
