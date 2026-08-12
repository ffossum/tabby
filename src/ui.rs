//! Rendering.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Text};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::input::slice_columns;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [text_area, status_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());

    // Remember the viewport so scroll clamping matches what is on screen.
    app.view_height = text_area.height as usize;
    app.view_width = text_area.width as usize;
    app.clamp();

    let visible = app
        .doc
        .lines
        .iter()
        .skip(app.top)
        .take(app.view_height)
        .map(|line| Line::raw(slice_columns(line, app.left, app.view_width)))
        .collect::<Vec<_>>();

    // No `.wrap()`: long lines are cut off, horizontal scrolling reveals them.
    frame.render_widget(Paragraph::new(Text::from(visible)), text_area);
    frame.render_widget(status_line(app), status_area);
}

fn status_line(app: &App) -> Paragraph<'static> {
    let total = app.doc.lines.len();
    let first = if total == 0 { 0 } else { app.top + 1 };
    let last = (app.top + app.view_height).min(total);
    let percent = if total <= app.view_height {
        100
    } else {
        (last * 100) / total
    };

    let text = format!(
        " {first}-{last}/{total}  col {}  {percent}%  q:quit ",
        app.left + 1,
    );

    Paragraph::new(text).style(Style::new().reversed())
}
