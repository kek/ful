//! Application state: ingests samples, derives rows + I/O rates, handles keys.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent};

use crate::datasource::DiskSample;
use crate::model::{is_pseudo_fs, MountRow};

pub struct Config {
    pub interval: Duration,
}

pub struct App {
    pub config: Config,
    /// All rows from the latest sample, unfiltered (pseudo filter applied in `visible_rows`).
    pub all_rows: Vec<MountRow>,
    pub show_all: bool,
    pub show_help: bool,
    pub should_quit: bool,
    /// True once any device has reported a nonzero cumulative I/O counter.
    pub io_available: bool,
    /// Previous (timestamp, raw samples) for rate computation.
    prev: Option<(Instant, Vec<DiskSample>)>,
}

impl App {
    pub fn new(config: Config) -> Self {
        App {
            config,
            all_rows: Vec::new(),
            show_all: false,
            show_help: false,
            should_quit: false,
            io_available: false,
            prev: None,
        }
    }

    /// Ingest a fresh sample taken at `now`, deriving rows and I/O rates.
    pub fn tick(&mut self, samples: Vec<DiskSample>, now: Instant) {
        // The `io_available` gate is intentionally GLOBAL across all devices: it
        // models the platform-level availability of per-disk I/O counters (e.g.
        // macOS exposes them for all disks or none), so a single observed nonzero
        // counter enables rate display for every row.
        if samples.iter().any(|s| s.read_bytes > 0 || s.written_bytes > 0) {
            self.io_available = true;
        }

        let dt = self
            .prev
            .as_ref()
            .map(|(t, _)| now.duration_since(*t).as_secs_f64());

        let mut rows = Vec::with_capacity(samples.len());
        for s in &samples {
            let (mut read_ps, mut write_ps) = (None, None);
            // Only compute rates once we know per-disk I/O counters are available
            // on this platform (see the global `io_available` note above).
            if self.io_available {
                if let (Some((_, prev_samples)), Some(dt)) = (&self.prev, dt) {
                    if dt > 0.0 {
                        if let Some(p) = prev_samples.iter().find(|p| p.device == s.device) {
                            read_ps = Some(s.read_bytes.saturating_sub(p.read_bytes) as f64 / dt);
                            write_ps =
                                Some(s.written_bytes.saturating_sub(p.written_bytes) as f64 / dt);
                        }
                    }
                }
            }
            rows.push(MountRow::from_sample(s, read_ps, write_ps));
        }

        self.all_rows = rows;
        self.prev = Some((now, samples));
    }

    /// Rows currently visible, honoring the `show_all` toggle.
    pub fn visible_rows(&self) -> Vec<&MountRow> {
        self.all_rows
            .iter()
            .filter(|r| self.show_all || !is_pseudo_fs(&r.fs_type, r.total))
            .collect()
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = !self.show_help,
            KeyCode::Char('a') => self.show_all = !self.show_all,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent};

    fn cfg() -> Config {
        Config { interval: Duration::from_secs(2) }
    }

    fn sample(dev: &str, fs: &str, total: u64, avail: u64, rd: u64, wr: u64) -> DiskSample {
        DiskSample {
            mount: format!("/{dev}"),
            device: dev.to_string(),
            fs_type: fs.to_string(),
            total,
            available: avail,
            read_bytes: rd,
            written_bytes: wr,
        }
    }

    #[test]
    fn first_tick_has_no_rates() {
        let mut app = App::new(cfg());
        app.tick(vec![sample("disk1", "apfs", 100, 40, 1000, 2000)], Instant::now());
        assert_eq!(app.all_rows.len(), 1);
        assert_eq!(app.all_rows[0].read_per_s, None);
        assert_eq!(app.all_rows[0].write_per_s, None);
    }

    #[test]
    fn second_tick_computes_rates_from_delta_over_time() {
        let mut app = App::new(cfg());
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(2);
        app.tick(vec![sample("disk1", "apfs", 100, 40, 1000, 2000)], t0);
        app.tick(vec![sample("disk1", "apfs", 100, 40, 3000, 2000)], t1);
        // read delta 2000 over 2s = 1000/s; write delta 0 = 0/s
        assert_eq!(app.all_rows[0].read_per_s, Some(1000.0));
        assert_eq!(app.all_rows[0].write_per_s, Some(0.0));
    }

    #[test]
    fn counter_reset_clamps_rate_to_zero() {
        let mut app = App::new(cfg());
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(2);
        app.tick(vec![sample("disk1", "apfs", 100, 40, 5000, 0)], t0);
        app.tick(vec![sample("disk1", "apfs", 100, 40, 1000, 0)], t1); // counter went down
        assert_eq!(app.all_rows[0].read_per_s, Some(0.0));
    }

    #[test]
    fn io_unavailable_keeps_rates_none() {
        // All counters zero across ticks -> io never seen -> rates stay None.
        let mut app = App::new(cfg());
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(2);
        app.tick(vec![sample("disk1", "apfs", 100, 40, 0, 0)], t0);
        app.tick(vec![sample("disk1", "apfs", 100, 40, 0, 0)], t1);
        assert_eq!(app.all_rows[0].read_per_s, None);
        assert_eq!(app.all_rows[0].write_per_s, None);
    }

    #[test]
    fn visible_rows_hides_pseudo_until_toggled() {
        let mut app = App::new(cfg());
        app.tick(
            vec![
                sample("disk1", "apfs", 100, 40, 0, 0),
                sample("vfs", "devfs", 0, 0, 0, 0),
            ],
            Instant::now(),
        );
        assert_eq!(app.all_rows.len(), 2);
        assert_eq!(app.visible_rows().len(), 1); // devfs hidden
        app.on_key(KeyEvent::from(KeyCode::Char('a')));
        assert_eq!(app.visible_rows().len(), 2); // now shown
    }

    #[test]
    fn keys_quit_and_toggle_help() {
        let mut app = App::new(cfg());
        assert!(!app.show_help);
        app.on_key(KeyEvent::from(KeyCode::Char('?')));
        assert!(app.show_help);
        app.on_key(KeyEvent::from(KeyCode::Char('q')));
        assert!(app.should_quit);
    }
}
