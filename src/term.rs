//! Shared terminal lifecycle: raw mode, alternate screen, panic-safe restore.

use std::io::{self, Stdout};

use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

/// Restores the terminal on drop (and via the panic hook) so a crash never
/// leaves the terminal in raw mode / the alternate screen.
pub struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = restore_terminal();
    }
}

pub fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(())
}

/// Chain a terminal-restoring panic hook in front of the default hook.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        default_hook(info);
    }));
}

/// Enter raw mode + alternate screen. The guard is bound BEFORE the fallible
/// calls so any error path restores the terminal.
pub fn init() -> io::Result<(Terminal<CrosstermBackend<Stdout>>, TerminalGuard)> {
    enable_raw_mode()?;
    let guard = TerminalGuard;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    Ok((terminal, guard))
}
