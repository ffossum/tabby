mod app;
mod input;
mod ui;

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, IsTerminal, Write};

use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use app::App;
use input::Document;

type Tui = Terminal<CrosstermBackend<BufWriter<File>>>;

fn main() -> io::Result<()> {
    let doc = match std::env::args().nth(1) {
        Some(path) => Document::from_path(&path),
        None if io::stdin().is_terminal() => {
            eprintln!("usage: tabby [FILE]   (or pipe data in: psql -c '...' | tabby)");
            std::process::exit(2);
        }
        None => Document::from_reader(io::stdin().lock()),
    };

    // Report before taking over the terminal, so the message survives.
    let doc = doc.unwrap_or_else(|e| {
        eprintln!("tabby: {e}");
        std::process::exit(2);
    });

    let mut terminal = init()?;
    let result = run(&mut terminal, App::new(doc));
    restore();
    result
}

fn run(terminal: &mut Tui, mut app: App) -> io::Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;

        match event::read()? {
            Event::Key(key) => app.handle_key(key),
            // Redraw happens at the top of the loop; nothing else to do.
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
    Ok(())
}

/// Open the controlling terminal.
///
/// Stdin is usually a pipe when we are somebody's `PAGER`, so we cannot draw to
/// it or read keys from it. (Crossterm's `use-dev-tty` feature makes it read
/// events from `/dev/tty` for the same reason.)
fn tty() -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("cannot open /dev/tty (tabby needs a terminal to draw on): {e}"),
            )
        })
}

fn init() -> io::Result<Tui> {
    let mut out = BufWriter::new(tty()?);
    enable_raw_mode()?;
    execute!(out, EnterAlternateScreen)?;

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        hook(info);
    }));

    Terminal::new(CrosstermBackend::new(out))
}

fn restore() {
    let _ = disable_raw_mode();
    if let Ok(mut out) = tty() {
        let _ = execute!(out, LeaveAlternateScreen);
        let _ = out.flush();
    }
}
