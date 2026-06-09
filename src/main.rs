mod app;
mod datasource;
mod format;
mod model;
mod ui;

use std::io;
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::{App, Config};
use datasource::{DataSource, SysinfoSource};

#[derive(Parser)]
#[command(name = "ful", about = "TUI disk usage monitor")]
struct Cli {
    /// Refresh interval in seconds
    #[arg(short, long, default_value_t = 2, value_parser = clap::value_parser!(u64).range(1..))]
    interval: u64,
}

/// Restores the terminal on drop (and via the panic hook) so a crash never
/// leaves the terminal in raw mode / the alternate screen.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = restore_terminal();
    }
}

fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(())
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        default_hook(info);
    }));

    enable_raw_mode()?;
    let _guard = TerminalGuard; // from here on, Drop restores the terminal on any exit
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut source = SysinfoSource::new();
    let mut app = App::new(Config {
        interval: Duration::from_secs(cli.interval),
    });

    app.tick(source.sample(), Instant::now());
    let mut next_tick = Instant::now() + app.config.interval;

    while !app.should_quit {
        terminal.draw(|f| ui::draw(&app, f))?;

        let timeout = next_tick.saturating_duration_since(Instant::now());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.on_key(key);
                }
            }
        }

        if Instant::now() >= next_tick {
            app.tick(source.sample(), Instant::now());
            next_tick = Instant::now() + app.config.interval;
        }
    }

    Ok(())
}
