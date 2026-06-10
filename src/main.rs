mod app;
mod datasource;
mod model;
mod ui;

use std::io;
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::event::{self, Event, KeyEventKind};

use app::{App, Config};
use datasource::{DataSource, SysinfoSource};

#[derive(Parser)]
#[command(name = "ful", about = "live filesystem dashboard — ful, as in watchful")]
struct Cli {
    /// Refresh interval in seconds
    #[arg(short, long, default_value_t = 2, value_parser = clap::value_parser!(u64).range(1..))]
    interval: u64,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    ful::term::install_panic_hook();
    let (mut terminal, _guard) = ful::term::init()?;

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
