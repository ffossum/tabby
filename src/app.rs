//! Pager state and key handling.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::input::Document;

/// How many columns a single horizontal step moves.
const HSTEP: usize = 1;

pub struct App {
    pub doc: Document,
    /// Index of the topmost visible line.
    pub top: usize,
    /// Display column of the leftmost visible cell.
    pub left: usize,
    /// Height in lines of the text viewport, updated on every draw.
    pub view_height: usize,
    /// Width in columns of the text viewport, updated on every draw.
    pub view_width: usize,
    pub should_quit: bool,
}

impl App {
    pub fn new(doc: Document) -> Self {
        Self {
            doc,
            top: 0,
            left: 0,
            view_height: 1,
            view_width: 1,
            should_quit: false,
        }
    }

    /// Largest valid `top`: scrolling stops with the last line at the bottom.
    fn max_top(&self) -> usize {
        self.doc.lines.len().saturating_sub(self.view_height)
    }

    /// Largest valid `left`: scrolling stops with the widest line flush right.
    fn max_left(&self) -> usize {
        self.doc.max_width.saturating_sub(self.view_width)
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
            KeyCode::Char('0') => self.left = 0,
            KeyCode::Char('$') => self.left = self.max_left(),

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
    use crate::input::Document;

    fn app(lines: usize, width: usize) -> App {
        let text = (0..lines)
            .map(|i| format!("{:width$}", i, width = width))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = App::new(Document::from_str(&text));
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
    fn horizontal_scroll_clamps_to_widest_line() {
        let mut a = app(3, 14);
        press(&mut a, KeyCode::Char('$'));
        assert_eq!(a.left, 4);
        press(&mut a, KeyCode::Right);
        assert_eq!(a.left, 4);
        press(&mut a, KeyCode::Char('0'));
        assert_eq!(a.left, 0);
        press(&mut a, KeyCode::Left);
        assert_eq!(a.left, 0);
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
