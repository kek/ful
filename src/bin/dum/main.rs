mod activity;
mod app;
mod deleter;
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
use deleter::DeleteMsg;
use scanner::ScanMsg;
use watcher::WatchMsg;

#[derive(Parser)]
#[command(name = "dum", about = "dum — disk usage monitor, live")]
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
    // Subtree scans for dirs that appear without per-child events (renames,
    // moves into the tree). Their Done messages are dropped: only the main
    // scan may drive `scanning` and the footer stats.
    let mut subscans: Vec<Receiver<ScanMsg>> = Vec::new();
    // The in-flight background delete's message stream, if one is running.
    let mut delete_rx: Option<Receiver<DeleteMsg>> = None;

    while !app.should_quit {
        let now = Instant::now();
        for msg in scan_rx.try_iter().take(MAX_MSGS_PER_FRAME) {
            app.apply_scan(msg);
        }
        subscans.retain(|rx| {
            for msg in rx.try_iter().take(MAX_MSGS_PER_FRAME) {
                match msg {
                    ScanMsg::Dir { .. } => app.apply_scan(msg),
                    ScanMsg::Done { .. } => return false,
                }
            }
            true
        });
        for path in std::mem::take(&mut app.subscan_requested) {
            subscans.push(spawn_scanner(path));
        }
        for msg in watch_rx.try_iter().take(MAX_MSGS_PER_FRAME) {
            match msg {
                WatchMsg::Delta(d) => app.apply_delta(d, now),
                WatchMsg::Degraded => app.watch_degraded = true,
            }
        }
        if let Some(rx) = &delete_rx {
            for msg in rx.try_iter().take(MAX_MSGS_PER_FRAME) {
                app.apply_delete_msg(msg, now);
            }
            if app.deleting.is_none() {
                delete_rx = None; // Done or Failed arrived; the thread is finished
            }
        }
        app.activity.evict(now);

        terminal.draw(|f| ui::draw(&app, f, now))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.on_key(key, now);
                }
            }
        }

        if let Some(target) = app.begin_delete() {
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || deleter::delete(target, tx));
            delete_rx = Some(rx);
        }

        if app.rescan_requested {
            app.rescan_requested = false;
            app.reset(Instant::now());
            subscans.clear();
            scan_rx = spawn_scanner(root.clone());
        }
    }

    // Quitting. Restore the terminal, then exit *without* running destructors:
    // freeing a multi-million-node tree (plus any unprocessed scan backlog) can
    // take well over a second, which would otherwise freeze the final frame on
    // screen while the OS is about to reclaim every page instantly anyway.
    ful::term::restore_terminal()?;
    std::process::exit(0);
}
