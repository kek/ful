mod activity;
mod app;
mod scanner;
mod tree;
mod ui;
mod watcher;

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::event::{self, Event, KeyEventKind};

use app::App;
use scanner::ScanMsg;
use watcher::WatchMsg;

#[derive(Parser)]
#[command(name = "dum", about = "Live-activity disk usage explorer")]
struct Cli {
    /// Directory to explore
    #[arg(default_value = ".")]
    path: PathBuf,
    /// Disable live filesystem watching (plain explorer)
    #[arg(long)]
    no_watch: bool,
}

/// Cap channel drains per frame so a flooding scanner can't starve rendering.
const MAX_MSGS_PER_FRAME: usize = 2000;

fn spawn_scanner(root: PathBuf) -> Receiver<ScanMsg> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || scanner::scan(root, tx));
    rx
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();
    let root = match cli.path.canonicalize() {
        Ok(p) if p.is_dir() => p,
        Ok(p) => {
            eprintln!("dum: {} is not a directory", p.display());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("dum: {}: {}", cli.path.display(), e);
            std::process::exit(1);
        }
    };

    ful::term::install_panic_hook();
    let (mut terminal, _guard) = ful::term::init()?;

    let mut scan_rx = spawn_scanner(root.clone());

    let (watch_tx, watch_rx) = mpsc::channel();
    if !cli.no_watch {
        let root = root.clone();
        thread::spawn(move || watcher::watch(root, watch_tx));
    }
    // (If no_watch, watch_tx is dropped here and watch_rx just stays empty.)

    let mut app = App::new(&root, Instant::now());
    app.watching = !cli.no_watch;

    while !app.should_quit {
        let now = Instant::now();
        for msg in scan_rx.try_iter().take(MAX_MSGS_PER_FRAME) {
            app.apply_scan(msg);
        }
        for msg in watch_rx.try_iter().take(MAX_MSGS_PER_FRAME) {
            match msg {
                WatchMsg::Delta(d) => app.apply_delta(d, now),
                WatchMsg::Degraded => app.watch_degraded = true,
            }
        }
        app.activity.evict(now);

        terminal.draw(|f| ui::draw(&app, f, now))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.on_key(key);
                }
            }
        }

        if app.rescan_requested {
            app.rescan_requested = false;
            app.reset(Instant::now());
            scan_rx = spawn_scanner(root.clone());
        }
    }

    Ok(())
}
