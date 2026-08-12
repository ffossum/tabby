//! Rendering.
//!
//! The table is laid out here rather than replayed from the input: each cell is
//! written out from its [`Data`], padded to its column's width, and the row is
//! then clipped to the horizontal viewport.

use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::input::{Column, Data, DataKind, Document, display_width, slice_columns};

/// A piece of a laid-out line: its text and how to paint it.
type Segment = (String, Style);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [table_area, status_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());
    // The names and the rule under them stay put while the rows scroll.
    let [head_area, rows_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(table_area);

    // Remember the viewport so scroll clamping matches what is on screen.
    app.view_height = rows_area.height as usize;
    app.view_width = table_area.width as usize;
    app.clamp();

    let view = app.left..app.left + app.view_width;
    let head = Text::from(vec![
        clip(header_segments(&app.doc), &view),
        clip(rule_segments(&app.doc), &view),
    ]);
    let rows = app
        .doc
        .rows
        .iter()
        .skip(app.top)
        .take(app.view_height)
        .map(|row| clip(row_segments(&app.doc, row), &view))
        .collect::<Vec<_>>();

    frame.render_widget(Paragraph::new(head), head_area);
    frame.render_widget(Paragraph::new(Text::from(rows)), rows_area);
    frame.render_widget(status_line(app), status_area);
}

fn header_segments(doc: &Document) -> Vec<Segment> {
    segments(doc, |column| {
        let name = center(&column.name, column.width);
        (name, Style::new().bold())
    })
}

/// The rule spans the padding as well, and joins with `+` rather than `|`.
fn rule_segments(doc: &Document) -> Vec<Segment> {
    let dim = Style::new().dark_gray();
    let dashes: Vec<String> = doc
        .columns
        .iter()
        .map(|c| "-".repeat(c.width + 2))
        .collect();

    vec![(dashes.join("+"), dim)]
}

fn row_segments(doc: &Document, row: &[Data]) -> Vec<Segment> {
    let mut cells = row.iter();
    segments(doc, move |column| {
        let cell = cells.next().unwrap_or(&Data::Null);
        let text = pad(&cell.to_string(), column.width, column.kind.is_numeric());
        (text, style(cell.kind()))
    })
}

/// Walk the columns left to right, gluing the cells `render` produces together
/// with the padding and `|` separators psql puts between them.
fn segments(doc: &Document, mut render: impl FnMut(&Column) -> Segment) -> Vec<Segment> {
    let mut out = Vec::with_capacity(doc.columns.len() * 4);

    for (i, column) in doc.columns.iter().enumerate() {
        if i > 0 {
            out.push(("|".to_string(), Style::new()));
        }

        out.push((" ".to_string(), Style::new()));
        out.push(render(column));
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
    let first = if total == 0 { 0 } else { app.top + 1 };
    let last = (app.top + app.view_height).min(total);
    let percent = if total <= app.view_height {
        100
    } else {
        (last * 100) / total
    };

    // In a wide table the column name is more use than the column number.
    let column = app.doc.column_at(app.left).map_or_else(
        || format!("col {}", app.left + 1),
        |c| format!("col {} {}", app.left + 1, c.name),
    );

    let text = format!(" {first}-{last}/{total} rows  {column}  {percent}%  q:quit ");

    Paragraph::new(text).style(Style::new().reversed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::psql;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Draw into a `width` x 5 terminal and read the cells back as text.
    fn render(app: &mut App, width: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, 5)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();

        let buffer = terminal.backend().buffer().clone();
        (0..5)
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
        let lines = render(&mut app(), 24);

        assert_eq!(
            lines[..4],
            [
                " id | name  | ratio   ",
                "----+-------+-------  ",
                "  1 | alice |  0.25   ",
                " 22 |       |  3.00   ",
            ]
            .map(|l| format!("{l:<24}"))
        );
    }

    #[test]
    fn clips_to_the_horizontal_viewport() {
        // The table is 20 wide, so in a 12 wide terminal it can scroll by 8.
        let mut app = app();
        app.left = 6;

        let lines = render(&mut app, 12);

        assert_eq!(lines[0], "name  | rati");
        assert_eq!(lines[2], "alice |  0.2");
    }
}
