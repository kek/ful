# dum — TUI Disk Usage Monitor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `dum`, a terminal dashboard that shows a live, 2-second-refreshing view of mounted filesystems — usage bar + %, used/free/total, and per-device read/write throughput — fitting cleanly in 80 columns and degrading gracefully in narrower terminals.

**Architecture:** Three layers with clean seams. A `DataSource` trait (impl: `SysinfoSource`) emits raw `DiskSample`s. An `App` holds state and computes I/O rates from consecutive samples (pure logic, no I/O or rendering). Pure `ui` functions render `&App` into a ratatui `Frame`. `main` owns the terminal and event loop.

**Tech Stack:** Rust 2021, ratatui 0.29 (+ crossterm 0.28 backend), sysinfo 0.33 (disk feature), clap 4 (derive).

**Commit messages:** Per the user's global preference, use plain subject lines — NO conventional-commit prefixes (`feat:`, `chore:`, etc.). End each commit body with the `Co-Authored-By` trailer shown in the steps.

---

## File Structure

| File | Responsibility |
|------|----------------|
| `Cargo.toml` | Package + dependencies |
| `src/main.rs` | CLI args (`--interval`), `TerminalGuard`, panic hook, event loop |
| `src/format.rs` | `human_bytes`, `human_rate` (display formatting) |
| `src/model.rs` | `MountRow` struct, `MountRow::from_sample`, `is_pseudo_fs` |
| `src/datasource.rs` | `DiskSample`, `DataSource` trait, `SysinfoSource`, (test) `FakeSource` |
| `src/app.rs` | `App`, `Config`, `tick` (rate computation + filtering), `on_key`, `visible_rows` |
| `src/ui.rs` | `Column`, `columns_for_width`, `usage_bar`, `bar_color`, `draw` + helpers |

Module dependency direction: `format` ← `ui`; `datasource` ← `model`/`app`; `model` ← `app`/`ui`; `app` ← `ui`/`main`. No cycles.

---

## Task 1: Project scaffold + dependencies

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs` (temporary stub, replaced in Task 7)

- [ ] **Step 1: Create `Cargo.toml`**

```toml
[package]
name = "dum"
version = "0.1.0"
edition = "2021"
description = "TUI disk usage monitor"

[dependencies]
ratatui = "0.29"
crossterm = "0.28"
sysinfo = { version = "0.33", default-features = false, features = ["disk"] }
clap = { version = "4", features = ["derive"] }
```

- [ ] **Step 2: Create a stub `src/main.rs` so the crate compiles**

```rust
fn main() {
    println!("dum");
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: builds successfully (dependencies download on first run).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs
git commit -m "scaffold dum crate with ratatui, sysinfo, clap deps

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `format.rs` — human-readable sizes and rates

**Files:**
- Create: `src/format.rs`
- Modify: `src/main.rs` (add `mod format;`)

- [ ] **Step 1: Declare the module in `src/main.rs`**

Add this line at the top of `src/main.rs` (above `fn main`):

```rust
mod format;
```

- [ ] **Step 2: Write failing tests in `src/format.rs`**

```rust
//! Display formatting for byte sizes and per-second rates.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_under_1k_use_b_with_space() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
    }

    #[test]
    fn bytes_scale_to_units() {
        assert_eq!(human_bytes(1024), "1.0K");
        assert_eq!(human_bytes(1536), "1.5K");
        assert_eq!(human_bytes(1_258_291), "1.2M");
        assert_eq!(human_bytes(48_318_382_080), "45G");
        assert_eq!(human_bytes(536_870_912_000), "500G");
    }

    #[test]
    fn rate_appends_per_second() {
        assert_eq!(human_rate(0.0), "0 B/s");
        assert_eq!(human_rate(1_258_291.0), "1.2M/s");
    }

    #[test]
    fn rate_clamps_negative_to_zero() {
        assert_eq!(human_rate(-5.0), "0 B/s");
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --lib format`
Expected: FAIL — `cannot find function human_bytes`.

(Note: tests live in a binary crate. `cargo test` compiles `main.rs` with `#[cfg(test)]`. The module functions below are not yet `pub`-required since tests are inline.)

- [ ] **Step 4: Implement the functions in `src/format.rs`** (above the `tests` module)

```rust
/// Format a byte count as a short human-readable string:
/// `0 B`, `999 B`, `1.0K`, `1.5K`, `1.2M`, `45G`, `500G`, `2.0T`.
/// Values < 1024 use `B` with a leading space; larger values use the
/// largest fitting unit, with one decimal below 10 and no decimal at/above 10.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    if bytes < 1024 {
        return format!("{} B", bytes);
    }
    let mut value = bytes as f64;
    let mut idx = 0usize;
    while value >= 1024.0 && idx < UNITS.len() - 1 {
        value /= 1024.0;
        idx += 1;
    }
    let unit = UNITS[idx];
    if value >= 10.0 {
        format!("{:.0}{}", value, unit)
    } else {
        format!("{:.1}{}", value, unit)
    }
}

/// Format a per-second byte rate, e.g. `0 B/s`, `1.2M/s`.
/// Negative inputs (counter resets) clamp to zero.
pub fn human_rate(bytes_per_sec: f64) -> String {
    let b = if bytes_per_sec < 0.0 {
        0
    } else {
        bytes_per_sec.round() as u64
    };
    format!("{}/s", human_bytes(b))
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib format`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add src/format.rs src/main.rs
git commit -m "add human-readable byte and rate formatting

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `datasource.rs` — sample type and trait

**Files:**
- Create: `src/datasource.rs`
- Modify: `src/main.rs` (add `mod datasource;`)

We define the data contract first (Task 3) so `model` and `app` can depend on `DiskSample`. The `SysinfoSource` implementation is added in this task too; the test `FakeSource` is added for use by `app` tests.

- [ ] **Step 1: Declare the module in `src/main.rs`**

Add near the other `mod` lines:

```rust
mod datasource;
```

- [ ] **Step 2: Write the sample type, trait, and SysinfoSource in `src/datasource.rs`**

```rust
//! Raw disk readings and the source that produces them.

/// One raw reading for a single mounted filesystem.
/// `read_bytes`/`written_bytes` are *cumulative* counters; rates are derived
/// by the `App` from the delta between two samples.
#[derive(Debug, Clone, PartialEq)]
pub struct DiskSample {
    pub mount: String,
    pub device: String, // short node, e.g. "disk3s1" (/dev/ stripped)
    pub fs_type: String,
    pub total: u64,
    pub available: u64,
    pub read_bytes: u64,
    pub written_bytes: u64,
}

/// Produces a snapshot of all mounted filesystems on each call.
pub trait DataSource {
    fn sample(&mut self) -> Vec<DiskSample>;
}

/// Live data source backed by the `sysinfo` crate.
pub struct SysinfoSource {
    disks: sysinfo::Disks,
}

impl SysinfoSource {
    pub fn new() -> Self {
        SysinfoSource {
            disks: sysinfo::Disks::new_with_refreshed_list(),
        }
    }
}

impl DataSource for SysinfoSource {
    fn sample(&mut self) -> Vec<DiskSample> {
        // Refresh existing disks (true = drop disks no longer present).
        self.disks.refresh(true);
        self.disks
            .list()
            .iter()
            .map(|d| {
                let usage = d.usage();
                DiskSample {
                    mount: d.mount_point().to_string_lossy().into_owned(),
                    device: short_device(&d.name().to_string_lossy()),
                    fs_type: d.file_system().to_string_lossy().into_owned(),
                    total: d.total_space(),
                    available: d.available_space(),
                    read_bytes: usage.total_read_bytes,
                    written_bytes: usage.total_written_bytes,
                }
            })
            .collect()
    }
}

/// Strip the directory portion of a device path: `/dev/disk3s1` -> `disk3s1`.
fn short_device(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_string()
}

/// Deterministic source for tests: returns each batch in order, then empty.
#[cfg(test)]
pub struct FakeSource {
    batches: Vec<Vec<DiskSample>>,
    idx: usize,
}

#[cfg(test)]
impl FakeSource {
    pub fn new(batches: Vec<Vec<DiskSample>>) -> Self {
        FakeSource { batches, idx: 0 }
    }
}

#[cfg(test)]
impl DataSource for FakeSource {
    fn sample(&mut self) -> Vec<DiskSample> {
        let b = self.batches.get(self.idx).cloned().unwrap_or_default();
        self.idx += 1;
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_device_strips_dev_prefix() {
        assert_eq!(short_device("/dev/disk3s1"), "disk3s1");
        assert_eq!(short_device("tmpfs"), "tmpfs");
    }

    #[test]
    fn fake_source_returns_batches_then_empty() {
        let mut s = FakeSource::new(vec![vec![sample("a")], vec![]]);
        assert_eq!(s.sample().len(), 1);
        assert_eq!(s.sample().len(), 0);
        assert_eq!(s.sample().len(), 0); // past the end -> empty
    }

    fn sample(dev: &str) -> DiskSample {
        DiskSample {
            mount: format!("/{dev}"),
            device: dev.to_string(),
            fs_type: "apfs".to_string(),
            total: 100,
            available: 50,
            read_bytes: 0,
            written_bytes: 0,
        }
    }
}
```

> **API note:** If the resolved `sysinfo` version differs and `disks.refresh(true)` or `usage().total_read_bytes` don't compile, adjust only those calls — the contract is "cumulative read/written byte counters + total/available space per disk". If a platform reports no I/O counters, leaving them at 0 is correct; the App degrades I/O to `—`.

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib datasource`
Expected: PASS (2 tests). If `SysinfoSource` fails to compile, fix per the API note above before proceeding.

- [ ] **Step 4: Commit**

```bash
git add src/datasource.rs src/main.rs
git commit -m "add DiskSample, DataSource trait, and sysinfo-backed source

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `model.rs` — display row and pseudo-fs filter

**Files:**
- Create: `src/model.rs`
- Modify: `src/main.rs` (add `mod model;`)

- [ ] **Step 1: Declare the module in `src/main.rs`**

```rust
mod model;
```

- [ ] **Step 2: Write failing tests in `src/model.rs`**

```rust
//! Display-ready mount row and real-vs-pseudo filesystem classification.

use crate::datasource::DiskSample;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(fs: &str, total: u64, available: u64) -> DiskSample {
        DiskSample {
            mount: "/".to_string(),
            device: "disk1".to_string(),
            fs_type: fs.to_string(),
            total,
            available,
            read_bytes: 0,
            written_bytes: 0,
        }
    }

    #[test]
    fn pseudo_when_zero_total_or_known_pseudo_fs() {
        assert!(is_pseudo_fs("apfs", 0)); // zero total -> pseudo
        assert!(is_pseudo_fs("devfs", 100));
        assert!(is_pseudo_fs("TMPFS", 100)); // case-insensitive
    }

    #[test]
    fn real_fs_with_capacity_is_not_pseudo() {
        assert!(!is_pseudo_fs("apfs", 100));
        assert!(!is_pseudo_fs("exfat", 100));
    }

    #[test]
    fn from_sample_derives_used_and_pct() {
        let row = MountRow::from_sample(&sample("apfs", 1000, 250), Some(5.0), None);
        assert_eq!(row.used, 750);
        assert_eq!(row.free, 250);
        assert_eq!(row.total, 1000);
        assert!((row.used_pct - 75.0).abs() < 1e-9);
        assert_eq!(row.read_per_s, Some(5.0));
        assert_eq!(row.write_per_s, None);
    }

    #[test]
    fn from_sample_handles_zero_total() {
        let row = MountRow::from_sample(&sample("devfs", 0, 0), None, None);
        assert_eq!(row.used, 0);
        assert_eq!(row.used_pct, 0.0);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --lib model`
Expected: FAIL — `cannot find function is_pseudo_fs` / `MountRow`.

- [ ] **Step 4: Implement in `src/model.rs`** (above the `tests` module)

```rust
const PSEUDO_FS: [&str; 8] = [
    "devfs", "autofs", "tmpfs", "proc", "sysfs", "overlay", "squashfs", "nullfs",
];

/// A filesystem is "pseudo" (hidden by default) if it has no capacity or its
/// type is a known virtual filesystem.
pub fn is_pseudo_fs(fs_type: &str, total: u64) -> bool {
    if total == 0 {
        return true;
    }
    let fs = fs_type.to_ascii_lowercase();
    PSEUDO_FS.contains(&fs.as_str())
}

/// Display-ready row: derived fields plus optional per-second I/O rates.
#[derive(Debug, Clone, PartialEq)]
pub struct MountRow {
    pub mount: String,
    pub device: String,
    pub fs_type: String,
    pub used: u64,
    pub free: u64,
    pub total: u64,
    pub used_pct: f64,
    pub read_per_s: Option<f64>,
    pub write_per_s: Option<f64>,
}

impl MountRow {
    pub fn from_sample(s: &DiskSample, read_per_s: Option<f64>, write_per_s: Option<f64>) -> Self {
        let used = s.total.saturating_sub(s.available);
        let used_pct = if s.total == 0 {
            0.0
        } else {
            used as f64 / s.total as f64 * 100.0
        };
        MountRow {
            mount: s.mount.clone(),
            device: s.device.clone(),
            fs_type: s.fs_type.clone(),
            used,
            free: s.available,
            total: s.total,
            used_pct,
            read_per_s,
            write_per_s,
        }
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib model`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add src/model.rs src/main.rs
git commit -m "add MountRow and pseudo-filesystem classification

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `app.rs` — state, rate computation, key handling

**Files:**
- Create: `src/app.rs`
- Modify: `src/main.rs` (add `mod app;`)

- [ ] **Step 1: Declare the module in `src/main.rs`**

```rust
mod app;
```

- [ ] **Step 2: Write failing tests in `src/app.rs`**

```rust
//! Application state: ingests samples, derives rows + I/O rates, handles keys.

use std::time::{Duration, Instant};

use crate::datasource::DiskSample;
use crate::model::{is_pseudo_fs, MountRow};

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
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --lib app`
Expected: FAIL — `cannot find type App` / `Config`.

- [ ] **Step 4: Implement in `src/app.rs`** (above the `tests` module)

```rust
use crossterm::event::{KeyCode, KeyEvent};

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
            if let (Some((_, prev_samples)), Some(dt)) = (&self.prev, dt) {
                if dt > 0.0 {
                    if let Some(p) = prev_samples.iter().find(|p| p.device == s.device) {
                        read_ps = Some(s.read_bytes.saturating_sub(p.read_bytes) as f64 / dt);
                        write_ps =
                            Some(s.written_bytes.saturating_sub(p.written_bytes) as f64 / dt);
                    }
                }
            }
            if !self.io_available {
                read_ps = None;
                write_ps = None;
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
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib app`
Expected: PASS (6 tests).

- [ ] **Step 6: Commit**

```bash
git add src/app.rs src/main.rs
git commit -m "add App state with I/O rate computation and key handling

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: `ui.rs` — responsive table, bar, help overlay

**Files:**
- Create: `src/ui.rs`
- Modify: `src/main.rs` (add `mod ui;`)

Column fixed widths (used for both layout constraints and ellipsis truncation):
Mount 14, Fs 5, Usage 15 (`[` + 8-cell bar + `]` + ` 100%`), Used 5, Free 5, Total 5, Read 7, Write 7, Dev 7. With `column_spacing(1)` the full 9-column tier sums to 78 ≤ 80.

Responsive thresholds (derived from summed widths + spacing):
- `>= 80`: Mount, Fs, Usage, Used, Free, Total, Read, Write, Dev
- `>= 58`: Mount, Usage, Free, Total, Read, Write
- `>= 40`: Mount, Usage, Free
- else: Mount, Usage

- [ ] **Step 1: Declare the module in `src/main.rs`**

```rust
mod ui;
```

- [ ] **Step 2: Write failing tests in `src/ui.rs`**

```rust
//! Pure rendering: responsive table, usage bar, footer, help overlay.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table};
use ratatui::Frame;

use crate::app::App;
use crate::format::{human_bytes, human_rate};
use crate::model::MountRow;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Config};
    use crate::model::MountRow;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;
    use std::time::Duration;

    fn buffer_text(buf: &Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    fn row(mount: &str, dev: &str, pct: f64) -> MountRow {
        MountRow {
            mount: mount.to_string(),
            device: dev.to_string(),
            fs_type: "apfs".to_string(),
            used: 680,
            free: 320,
            total: 1000,
            used_pct: pct,
            read_per_s: Some(1_258_291.0),
            write_per_s: None,
        }
    }

    fn app_with_rows(rows: Vec<MountRow>) -> App {
        let mut app = App::new(Config { interval: Duration::from_secs(2) });
        app.io_available = true;
        app.all_rows = rows;
        app
    }

    #[test]
    fn columns_by_width_tiers() {
        assert_eq!(columns_for_width(100).len(), 9);
        assert_eq!(columns_for_width(60).len(), 6);
        assert_eq!(columns_for_width(45).len(), 3);
        assert_eq!(columns_for_width(30).len(), 2);
    }

    #[test]
    fn usage_bar_fills_proportionally() {
        assert_eq!(usage_bar(0.0, 8), "[░░░░░░░░]");
        assert_eq!(usage_bar(100.0, 8), "[████████]");
        assert_eq!(usage_bar(50.0, 8), "[████░░░░]");
    }

    #[test]
    fn bar_color_thresholds() {
        assert_eq!(bar_color(10.0), Color::Green);
        assert_eq!(bar_color(80.0), Color::Yellow);
        assert_eq!(bar_color(95.0), Color::Red);
    }

    #[test]
    fn full_width_renders_all_headers() {
        let app = app_with_rows(vec![row("/", "disk3s1", 68.0)]);
        let backend = TestBackend::new(80, 10);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| draw(&app, f)).unwrap();
        let text = buffer_text(term.backend().buffer());
        assert!(text.contains("dum"));
        assert!(text.contains("MOUNT"));
        assert!(text.contains("WRITE/s"));
        assert!(text.contains("disk3s1"));
    }

    #[test]
    fn narrow_width_drops_io_columns() {
        let app = app_with_rows(vec![row("/", "disk3s1", 68.0)]);
        let backend = TestBackend::new(45, 10);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| draw(&app, f)).unwrap();
        let text = buffer_text(term.backend().buffer());
        assert!(text.contains("MOUNT"));
        assert!(!text.contains("WRITE/s"));
    }

    #[test]
    fn empty_shows_placeholder() {
        let app = app_with_rows(vec![]);
        let backend = TestBackend::new(80, 10);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| draw(&app, f)).unwrap();
        let text = buffer_text(term.backend().buffer());
        assert!(text.contains("No filesystems found"));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --lib ui`
Expected: FAIL — `cannot find function columns_for_width` / `draw`.

- [ ] **Step 4: Implement in `src/ui.rs`** (above the `tests` module, below the `use` lines)

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Mount,
    Fs,
    Usage,
    Used,
    Free,
    Total,
    Read,
    Write,
    Dev,
}

/// Choose which columns fit, widest set first.
pub fn columns_for_width(width: u16) -> Vec<Column> {
    use Column::*;
    if width >= 80 {
        vec![Mount, Fs, Usage, Used, Free, Total, Read, Write, Dev]
    } else if width >= 58 {
        vec![Mount, Usage, Free, Total, Read, Write]
    } else if width >= 40 {
        vec![Mount, Usage, Free]
    } else {
        vec![Mount, Usage]
    }
}

fn col_width(c: Column) -> u16 {
    match c {
        Column::Mount => 14,
        Column::Fs => 5,
        Column::Usage => 15,
        Column::Used => 5,
        Column::Free => 5,
        Column::Total => 5,
        Column::Read => 7,
        Column::Write => 7,
        Column::Dev => 7,
    }
}

fn header_label(c: Column) -> &'static str {
    match c {
        Column::Mount => "MOUNT",
        Column::Fs => "FS",
        Column::Usage => "USAGE",
        Column::Used => "USED",
        Column::Free => "FREE",
        Column::Total => "TOTL",
        Column::Read => "READ/s",
        Column::Write => "WRITE/s",
        Column::Dev => "DEV",
    }
}

/// Build the bracketed block-character usage bar (no percent).
pub fn usage_bar(pct: f64, cells: usize) -> String {
    let ratio = (pct / 100.0).clamp(0.0, 1.0);
    let filled = (ratio * cells as f64).round() as usize;
    let filled = filled.min(cells);
    let mut s = String::with_capacity(cells + 2);
    s.push('[');
    for i in 0..cells {
        s.push(if i < filled { '█' } else { '░' });
    }
    s.push(']');
    s
}

pub fn bar_color(pct: f64) -> Color {
    if pct > 90.0 {
        Color::Red
    } else if pct >= 70.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

/// Truncate to `width` chars, appending `…` when clipped.
fn truncate_ellipsis(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out: String = chars[..width - 1].iter().collect();
    out.push('…');
    out
}

fn io_text(rate: Option<f64>) -> String {
    match rate {
        Some(r) => human_rate(r),
        None => "—".to_string(),
    }
}

fn cell_for<'a>(r: &MountRow, c: Column) -> Cell<'a> {
    match c {
        Column::Mount => Cell::from(truncate_ellipsis(&r.mount, col_width(Column::Mount) as usize)),
        Column::Fs => Cell::from(r.fs_type.clone()),
        Column::Usage => {
            let bar = usage_bar(r.used_pct, 8);
            Cell::from(Line::from(vec![
                Span::styled(bar, Style::new().fg(bar_color(r.used_pct))),
                Span::raw(format!(" {:>3.0}%", r.used_pct)),
            ]))
        }
        Column::Used => Cell::from(human_bytes(r.used)),
        Column::Free => Cell::from(human_bytes(r.free)),
        Column::Total => Cell::from(human_bytes(r.total)),
        Column::Read => Cell::from(io_text(r.read_per_s)),
        Column::Write => Cell::from(io_text(r.write_per_s)),
        Column::Dev => Cell::from(truncate_ellipsis(&r.device, col_width(Column::Dev) as usize)),
    }
}

/// Render the whole dashboard.
pub fn draw(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let [title_area, table_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_title(app, frame, title_area);

    let rows = app.visible_rows();
    if app.all_rows.is_empty() {
        let msg = Paragraph::new("No filesystems found").alignment(Alignment::Center);
        frame.render_widget(msg, table_area);
    } else {
        draw_table(&rows, frame, table_area);
    }

    draw_footer(app, rows.len(), frame, footer_area);

    if app.show_help {
        draw_help(frame, area);
    }
}

fn draw_title(app: &App, frame: &mut Frame, area: Rect) {
    let right = format!("refresh {}s ", app.config.interval.as_secs());
    let [l, r] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right.len() as u16 + 1)])
            .areas(area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::from(" dum — disk usage monitor").bold())),
        l,
    );
    frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), r);
}

fn draw_table(rows: &[&MountRow], frame: &mut Frame, area: Rect) {
    let cols = columns_for_width(area.width);
    let header = Row::new(cols.iter().map(|c| Cell::from(header_label(*c))).collect::<Vec<_>>())
        .style(Style::new().bold());
    let body: Vec<Row> = rows
        .iter()
        .map(|r| Row::new(cols.iter().map(|c| cell_for(r, *c)).collect::<Vec<_>>()))
        .collect();
    let widths: Vec<Constraint> = cols
        .iter()
        .map(|c| Constraint::Length(col_width(*c)))
        .collect();
    let table = Table::new(body, widths).header(header).column_spacing(1);
    frame.render_widget(table, area);
}

fn draw_footer(app: &App, visible: usize, frame: &mut Frame, area: Rect) {
    let toggle = if app.show_all { "a hide pseudo" } else { "a toggle all" };
    let left = format!(" q quit   ? help   {toggle}");
    let right = format!("showing {} of {} mounts ", visible, app.all_rows.len());
    let [l, r] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right.len() as u16 + 1)])
            .areas(area);
    frame.render_widget(Paragraph::new(left).style(Style::new().dim()), l);
    frame.render_widget(
        Paragraph::new(right).alignment(Alignment::Right).style(Style::new().dim()),
        r,
    );
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from("dum — keybindings"),
        Line::from(""),
        Line::from("  q / Esc   quit"),
        Line::from("  ?         toggle this help"),
        Line::from("  a         show/hide pseudo filesystems"),
        Line::from(""),
        Line::from("bar: green <70%  yellow 70-90%  red >90%"),
    ];
    let w = 46.min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect { x, y, width: w, height: h };
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" help ")),
        rect,
    );
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib ui`
Expected: PASS (6 tests).

- [ ] **Step 6: Commit**

```bash
git add src/ui.rs src/main.rs
git commit -m "add responsive table rendering, usage bar, and help overlay

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: `main.rs` — terminal lifecycle and event loop

**Files:**
- Modify: `src/main.rs` (replace the stub `fn main`; keep the `mod` lines)

- [ ] **Step 1: Replace `src/main.rs` with the full program**

Keep the module declarations at the top; replace the stub `main`. Final file:

```rust
mod app;
mod datasource;
mod format;
mod model;
mod ui;

use std::io::{self, Stdout};
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
#[command(name = "dum", about = "TUI disk usage monitor")]
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

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout))
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

    let mut terminal = setup_terminal()?;
    let _guard = TerminalGuard;

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
```

- [ ] **Step 2: Verify the whole crate builds and all tests pass**

Run: `cargo build && cargo test`
Expected: build succeeds; all unit tests (format, datasource, model, app, ui) PASS.

- [ ] **Step 3: Check for warnings and lint**

Run: `cargo clippy --all-targets`
Expected: no errors. Fix any clippy warnings it reports (e.g. needless clones) before committing.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "wire terminal lifecycle, panic-safe restore, and event loop

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Manual verification on macOS

**Files:** none (manual run + optional README).

- [ ] **Step 1: Run the app**

Run: `cargo run` (then `cargo run -- --interval 1` to test the flag)
Expected: dashboard appears, real disks listed, usage bars colored by fullness.

- [ ] **Step 2: Verify behavior**

Check each:
- Resize the terminal narrower than 80 cols → table drops columns and never wraps.
- Resize below ~40 cols → only MOUNT + USAGE remain.
- Press `a` → pseudo filesystems (devfs, etc.) appear/disappear; footer count updates.
- Press `?` → help overlay toggles.
- Press `q` then Esc in another run → quits and terminal is restored cleanly (prompt intact, echo working).
- Observe I/O columns: they show `—` on the first refresh, then either live rates or `—` if macOS reports no per-disk counters via sysinfo (documented limitation).

- [ ] **Step 3: Write a short `README.md`**

```markdown
# dum

A terminal disk-usage monitor: a live, refreshing dashboard of mounted
filesystems — usage bar, used/free/total, and per-device I/O.

## Usage

    cargo run                 # default 2s refresh
    cargo run -- --interval 1 # custom refresh interval (seconds)

Keys: `q`/Esc quit · `?` help · `a` toggle pseudo filesystems.

The table is responsive and fits within 80 columns, dropping lower-priority
columns (DEV, FS, USED, then I/O) as the terminal narrows.

## Notes

macOS reports I/O per physical device. If `sysinfo` surfaces no per-disk
counters on your system, the READ/s and WRITE/s columns show `—`.
```

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "add README with usage and notes

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review (completed)

**Spec coverage:**
- Live mount dashboard, 2s refresh → Task 5 (`tick`), Task 7 (loop, `--interval`). ✓
- Usage bar + % color-coded → Task 6 (`usage_bar`, `bar_color`). ✓
- Used/Free/Total → Task 4 (`MountRow`), Task 6 (cells). ✓
- Mount + device info (DEV at full width) → Task 6 (column tiers). ✓
- Live I/O rate (per-device, graceful degradation) → Task 3 (counters), Task 5 (rates + `io_available`), Task 6 (`—`). ✓
- Real-vs-pseudo with `a` toggle → Task 4 (`is_pseudo_fs`), Task 5 (`visible_rows`/`on_key`). ✓
- `q`/`?` interactions → Task 5 (`on_key`), Task 6 (help overlay). ✓
- 80-col fit + responsive no-wrap → Task 6 (`columns_for_width`, fixed widths). ✓
- Terminal safety (guard + panic hook), empty state → Task 7, Task 6. ✓
- Tests for format/model/app/ui → Tasks 2,4,5,6. ✓
- `DataSource` trait seam → Task 3. ✓

**Placeholder scan:** No TBD/TODO; all code steps contain complete code. The one "API note" in Task 3 is version-adaptation guidance, not missing logic.

**Type consistency:** `DiskSample` fields (Task 3) match usage in `model`/`app` (Tasks 4,5). `MountRow` fields (Task 4) match `cell_for` (Task 6). `is_pseudo_fs(&str, u64)` signature consistent across model/app. `Column`/`columns_for_width`/`usage_bar`/`bar_color`/`draw` names consistent between ui tests and impl. `App::new`, `tick`, `visible_rows`, `on_key`, `all_rows`, `io_available`, `config` consistent across app/ui/main.
