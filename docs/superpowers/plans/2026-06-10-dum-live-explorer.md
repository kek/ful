# dum — Live-Activity Disk Usage Explorer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a second binary `dum` to the `ful` package: an ncdu-style disk-usage tree explorer that watches filesystem events live and illuminates rows that are growing (green) or shrinking (red), with live rates, sparklines, and live-updating sizes.

**Architecture:** Three threads. A scanner thread walks the root once and streams `ScanMsg` batches; a watcher thread (`notify`/FSEvents) coalesces events ~100ms, stats changed paths, and reports observed absolute sizes as `DeltaMsg`; the UI thread owns all state (arena tree + sparse activity map), computes signed deltas itself, and renders at ~10fps. Read-only; no deletion.

**Tech Stack:** Rust 2021, ratatui 0.29, crossterm 0.28, notify 8 (FSEvents on macOS), clap 4, tempfile 3 (dev-dep). Package `ful` gains a `[lib]` shared by both binaries.

**Commit messages:** Plain subject lines, NO conventional-commit prefixes. End each commit body with the trailer shown in the steps.

**Spec:** `docs/superpowers/specs/2026-06-10-dum-design.md`. One deliberate deviation: `DeltaKind` has two variants (`Changed`, `Removed`) instead of the spec's three — `Created` is folded into `Changed` because the UI thread decides created-vs-changed by tree lookup anyway.

---

## File Structure

| File | Responsibility |
|------|----------------|
| `Cargo.toml` | `[lib]` + two `[[bin]]` targets, notify + tempfile deps |
| `src/lib.rs` | NEW: `pub mod format; pub mod term;` |
| `src/format.rs` | UNCHANGED content, now a lib module |
| `src/term.rs` | NEW: shared `TerminalGuard`, `restore_terminal`, `install_panic_hook`, `init` |
| `src/main.rs` | ful binary; loses inlined guard code, uses `ful::term` / `ful::format` |
| `src/ui.rs` (ful) | import changes from `crate::format` to `ful::format` |
| `src/bin/dum/main.rs` | dum CLI, thread spawn, event loop |
| `src/bin/dum/tree.rs` | arena tree, size rollups, path index |
| `src/bin/dum/activity.rs` | EWMA rates, decay, sparkline rings, eviction |
| `src/bin/dum/scanner.rs` | walk thread → `ScanMsg`; `allocated_size` |
| `src/bin/dum/app.rs` | state, apply scan/delta msgs, navigation keys |
| `src/bin/dum/watcher.rs` | `DeltaMsg`/`WatchMsg` types + notify thread |
| `src/bin/dum/ui.rs` | responsive rows, glow styles, sparkline, help |
| `README.md` | rewritten as "ful & dum" |

Tests run per-target: `cargo test --lib` (format), `cargo test --bin ful`, `cargo test --bin dum`. Plain `cargo test` runs all.

---

## Task 1: Library refactor (format + term shared)

**Files:**
- Create: `src/lib.rs`, `src/term.rs`
- Modify: `src/main.rs`, `src/ui.rs`

ful currently declares `mod format;` in its binary and inlines the terminal guard in `main.rs`. Move both into a library target both binaries can use. ful's behavior must not change; all 22 existing tests must keep passing (4 format tests move to the lib target).

- [ ] **Step 1: Create `src/lib.rs`**

```rust
//! Shared library for the `ful` and `dum` binaries.

pub mod format;
pub mod term;
```

- [ ] **Step 2: Create `src/term.rs`** (content extracted from ful's `src/main.rs`)

```rust
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
```

- [ ] **Step 3: Update ful's `src/main.rs`**

Remove: the `mod format;` line, the `TerminalGuard` struct + impl, `restore_terminal`, the inlined `enable_raw_mode/EnterAlternateScreen/Terminal::new` block, the manual panic-hook block, and the now-unused imports (`Stdout`, `execute`, terminal fns, `CrosstermBackend`, `Terminal`).

The top of the file becomes:

```rust
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
```

And `fn main` starts:

```rust
fn main() -> io::Result<()> {
    let cli = Cli::parse();

    ful::term::install_panic_hook();
    let (mut terminal, _guard) = ful::term::init()?;
```

(The rest of `main` — source, app, loop — is unchanged.)

- [ ] **Step 4: Update ful's `src/ui.rs` import**

Change `use crate::format::{human_bytes, human_rate};` to `use ful::format::{human_bytes, human_rate};`

- [ ] **Step 5: Verify everything still passes**

Run: `cargo test && cargo clippy --all-targets`
Expected: 22 tests pass total (4 under `--lib` now), clippy clean. The `Cli` struct and event loop behavior are untouched.

- [ ] **Step 6: Commit**

```bash
git add src/lib.rs src/term.rs src/main.rs src/ui.rs
git commit -m "extract shared format and terminal modules into a library target

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Cargo targets + dum skeleton

**Files:**
- Modify: `Cargo.toml`
- Create: `src/bin/dum/main.rs` (stub)

- [ ] **Step 1: Update `Cargo.toml`**

```toml
[package]
name = "ful"
version = "0.1.0"
edition = "2021"
description = "TUI disk usage monitor (ful) and live disk usage explorer (dum)"
license = "MIT"

[lib]
name = "ful"
path = "src/lib.rs"

[[bin]]
name = "ful"
path = "src/main.rs"

[[bin]]
name = "dum"
path = "src/bin/dum/main.rs"

[dependencies]
ratatui = "0.29"
crossterm = "0.28"
sysinfo = { version = "0.33", default-features = false, features = ["disk"] }
clap = { version = "4", features = ["derive"] }
notify = "8"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Create stub `src/bin/dum/main.rs`**

```rust
fn main() {
    println!("dum");
}
```

- [ ] **Step 3: Verify both binaries build**

Run: `cargo build --bins && ls target/debug/ful target/debug/dum`
Expected: both binaries exist.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock src/bin/dum/main.rs
git commit -m "add dum binary target and notify dependency

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: tree.rs — arena tree with rollups

**Files:**
- Create: `src/bin/dum/tree.rs`
- Modify: `src/bin/dum/main.rs` (add `mod tree;`)

Run dum-bin tests with: `cargo test --bin dum`

- [ ] **Step 1: Add `mod tree;` above `fn main` in `src/bin/dum/main.rs`**

- [ ] **Step 2: Write failing tests in `src/bin/dum/tree.rs`**

```rust
//! Arena tree of scanned filesystem nodes with live size rollups.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn tree() -> Tree {
        Tree::new(Path::new("/root"))
    }

    #[test]
    fn new_tree_has_indexed_root() {
        let t = tree();
        assert_eq!(t.lookup(Path::new("/root")), Some(t.root));
        assert_eq!(t.get(t.root).size, 0);
        assert!(t.get(t.root).is_dir);
    }

    #[test]
    fn insert_rolls_size_up_to_ancestors() {
        let mut t = tree();
        let dir = t.insert(t.root, PathBuf::from("/root/a"), OsString::from("a"), 0, true);
        t.insert(dir, PathBuf::from("/root/a/f"), OsString::from("f"), 100, false);
        assert_eq!(t.get(dir).size, 100);
        assert_eq!(t.get(t.root).size, 100);
    }

    #[test]
    fn set_size_returns_signed_delta_and_bubbles() {
        let mut t = tree();
        let f = t.insert(t.root, PathBuf::from("/root/f"), OsString::from("f"), 100, false);
        let d = t.set_size(f, 250);
        assert_eq!(d, 150);
        assert_eq!(t.get(t.root).size, 250);
        let d = t.set_size(f, 50);
        assert_eq!(d, -200);
        assert_eq!(t.get(t.root).size, 50);
    }

    #[test]
    fn remove_subtracts_subtree_and_purges_index() {
        let mut t = tree();
        let dir = t.insert(t.root, PathBuf::from("/root/a"), OsString::from("a"), 0, true);
        t.insert(dir, PathBuf::from("/root/a/f"), OsString::from("f"), 100, false);
        let delta = t.remove(dir, Path::new("/root/a"));
        assert_eq!(delta, -100);
        assert_eq!(t.get(t.root).size, 0);
        assert_eq!(t.lookup(Path::new("/root/a")), None);
        assert_eq!(t.lookup(Path::new("/root/a/f")), None);
        assert!(t.get(t.root).children.is_empty());
    }

    #[test]
    fn path_of_reconstructs_full_path() {
        let mut t = tree();
        let dir = t.insert(t.root, PathBuf::from("/root/a"), OsString::from("a"), 0, true);
        let f = t.insert(dir, PathBuf::from("/root/a/f"), OsString::from("f"), 1, false);
        assert_eq!(t.path_of(f), PathBuf::from("/root/a/f"));
    }

    #[test]
    fn ancestors_inclusive_walks_to_root() {
        let mut t = tree();
        let dir = t.insert(t.root, PathBuf::from("/root/a"), OsString::from("a"), 0, true);
        let f = t.insert(dir, PathBuf::from("/root/a/f"), OsString::from("f"), 1, false);
        assert_eq!(t.ancestors_inclusive(f), vec![f, dir, t.root]);
    }
}
```

- [ ] **Step 3: Run `cargo test --bin dum` — expect FAIL (Tree not found)**

- [ ] **Step 4: Implement above the tests module**

```rust
pub type NodeId = usize;

pub struct Node {
    pub name: OsString,
    /// Allocated bytes; for dirs this includes the whole subtree.
    pub size: u64,
    pub is_dir: bool,
    pub denied: bool,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

pub struct Tree {
    nodes: Vec<Node>,
    by_path: HashMap<PathBuf, NodeId>,
    pub root: NodeId,
}

impl Tree {
    /// Root node's `name` is the full root path, so `path_of` works uniformly.
    pub fn new(root_path: &Path) -> Self {
        let root = Node {
            name: root_path.as_os_str().to_os_string(),
            size: 0,
            is_dir: true,
            denied: false,
            parent: None,
            children: Vec::new(),
        };
        let mut by_path = HashMap::new();
        by_path.insert(root_path.to_path_buf(), 0);
        Tree { nodes: vec![root], by_path, root: 0 }
    }

    pub fn get(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn get_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id]
    }

    pub fn lookup(&self, path: &Path) -> Option<NodeId> {
        self.by_path.get(path).copied()
    }

    /// Insert a node and roll its size up through all ancestors.
    pub fn insert(
        &mut self,
        parent: NodeId,
        path: PathBuf,
        name: OsString,
        size: u64,
        is_dir: bool,
    ) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            name,
            size,
            is_dir,
            denied: false,
            parent: Some(parent),
            children: Vec::new(),
        });
        self.nodes[parent].children.push(id);
        self.by_path.insert(path, id);
        self.bubble(Some(parent), size as i64);
        id
    }

    /// Set a node's size; returns the signed delta, already bubbled to ancestors.
    pub fn set_size(&mut self, id: NodeId, new_size: u64) -> i64 {
        let delta = new_size as i64 - self.nodes[id].size as i64;
        self.nodes[id].size = new_size;
        let parent = self.nodes[id].parent;
        self.bubble(parent, delta);
        delta
    }

    /// Detach a subtree; returns the (negative) signed delta applied to ancestors.
    /// Arena slots are not reclaimed (acceptable for v1); the path index is purged.
    pub fn remove(&mut self, id: NodeId, path: &Path) -> i64 {
        let size = self.nodes[id].size;
        if let Some(p) = self.nodes[id].parent {
            self.nodes[p].children.retain(|&c| c != id);
            self.bubble(Some(p), -(size as i64));
        }
        self.by_path.retain(|p, _| !p.starts_with(path));
        -(size as i64)
    }

    /// Node ids from `id` up to and including the root.
    pub fn ancestors_inclusive(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut cur = Some(id);
        while let Some(i) = cur {
            out.push(i);
            cur = self.nodes[i].parent;
        }
        out
    }

    pub fn path_of(&self, id: NodeId) -> PathBuf {
        let ids = self.ancestors_inclusive(id);
        let mut path = PathBuf::new();
        for &i in ids.iter().rev() {
            path.push(&self.nodes[i].name);
        }
        path
    }

    fn bubble(&mut self, mut cur: Option<NodeId>, delta: i64) {
        while let Some(id) = cur {
            let s = self.nodes[id].size as i64 + delta;
            self.nodes[id].size = s.max(0) as u64;
            cur = self.nodes[id].parent;
        }
    }
}
```

- [ ] **Step 5: Run `cargo test --bin dum` — expect PASS (6 tests)**

- [ ] **Step 6: Commit**

```bash
git add src/bin/dum/tree.rs src/bin/dum/main.rs
git commit -m "add dum arena tree with size rollups and path index

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: activity.rs — rates, decay, sparklines

**Files:**
- Create: `src/bin/dum/activity.rs`
- Modify: `src/bin/dum/main.rs` (add `mod activity;`)

All functions take `now: Instant` so tests use simulated time (`Instant + Duration`), no sleeping.

- [ ] **Step 1: Add `mod activity;` to `src/bin/dum/main.rs`**

- [ ] **Step 2: Write failing tests in `src/bin/dum/activity.rs`**

```rust
//! Sparse per-node live-activity state: EWMA rates, decay, sparkline rings.

use std::collections::HashMap;
use std::time::Instant;

use crate::tree::NodeId;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn record_positive_delta_gives_positive_glow() {
        let t0 = Instant::now();
        let mut m = ActivityMap::new(t0);
        m.record(1, 1_000_000, t0 + Duration::from_secs(1));
        assert!(m.glow(1, t0 + Duration::from_secs(1)) > 0.0);
    }

    #[test]
    fn negative_delta_gives_negative_glow() {
        let t0 = Instant::now();
        let mut m = ActivityMap::new(t0);
        m.record(1, -500_000, t0 + Duration::from_secs(1));
        assert!(m.glow(1, t0 + Duration::from_secs(1)) < 0.0);
    }

    #[test]
    fn glow_decays_exponentially_after_activity_stops() {
        let t0 = Instant::now();
        let mut m = ActivityMap::new(t0);
        let t1 = t0 + Duration::from_secs(1);
        m.record(1, 1_000_000, t1);
        let fresh = m.glow(1, t1);
        let later = m.glow(1, t1 + Duration::from_secs(5));
        assert!(later < fresh * 0.2); // 5s = 2*tau -> e^-2 ~= 0.135
        assert!(later > 0.0);
    }

    #[test]
    fn unknown_node_has_zero_glow_and_no_sparkline() {
        let t0 = Instant::now();
        let m = ActivityMap::new(t0);
        assert_eq!(m.glow(42, t0), 0.0);
        assert!(m.sparkline(42).is_none());
    }

    #[test]
    fn ring_buckets_deltas_by_second_oldest_first() {
        let t0 = Instant::now();
        let mut m = ActivityMap::new(t0);
        m.record(1, 100, t0 + Duration::from_secs(1));
        m.record(1, 200, t0 + Duration::from_secs(1));
        m.record(1, 50, t0 + Duration::from_secs(3));
        let ring = m.sparkline(1).unwrap();
        // newest bucket (second 3) is last; second 1 holds 300; gap second 2 is 0
        assert_eq!(ring[RING_BUCKETS - 1], 50);
        assert_eq!(ring[RING_BUCKETS - 3], 300);
        assert_eq!(ring[RING_BUCKETS - 2], 0);
    }

    #[test]
    fn evict_drops_stale_entries() {
        let t0 = Instant::now();
        let mut m = ActivityMap::new(t0);
        m.record(1, 100, t0 + Duration::from_secs(1));
        m.evict(t0 + Duration::from_secs(2));
        assert!(m.sparkline(1).is_some());
        m.evict(t0 + Duration::from_secs(60));
        assert!(m.sparkline(1).is_none());
    }
}
```

- [ ] **Step 3: Run `cargo test --bin dum activity` — expect FAIL**

- [ ] **Step 4: Implement above the tests module**

```rust
pub const RING_BUCKETS: usize = 30;
/// Glow decay time constant (seconds).
pub const TAU_SECS: f64 = 2.5;
const ALPHA: f64 = 0.3;
/// Entries idle longer than this are evicted (ring fully aged out).
const EVICT_AFTER_SECS: f64 = 35.0;

struct Activity {
    rate: f64, // EWMA of signed bytes/sec
    last_update: Instant,
    ring: [i64; RING_BUCKETS],
    last_bucket: u64,
}

pub struct ActivityMap {
    map: HashMap<NodeId, Activity>,
    start: Instant,
}

impl ActivityMap {
    pub fn new(start: Instant) -> Self {
        ActivityMap { map: HashMap::new(), start }
    }

    pub fn record(&mut self, id: NodeId, delta: i64, now: Instant) {
        let bucket = now.duration_since(self.start).as_secs();
        let a = self.map.entry(id).or_insert(Activity {
            rate: 0.0,
            last_update: now,
            ring: [0; RING_BUCKETS],
            last_bucket: bucket,
        });
        let dt = now.duration_since(a.last_update).as_secs_f64().max(0.05);
        a.rate = ALPHA * (delta as f64 / dt) + (1.0 - ALPHA) * a.rate;
        a.last_update = now;
        // Zero any skipped buckets, then add into the current one.
        let from = a.last_bucket + 1;
        for b in from..=bucket {
            a.ring[(b % RING_BUCKETS as u64) as usize] = 0;
        }
        a.last_bucket = bucket;
        a.ring[(bucket % RING_BUCKETS as u64) as usize] += delta;
    }

    /// Signed bytes/sec with exponential age decay; 0.0 when unknown.
    pub fn glow(&self, id: NodeId, now: Instant) -> f64 {
        self.map
            .get(&id)
            .map(|a| {
                let age = now.duration_since(a.last_update).as_secs_f64();
                a.rate * (-age / TAU_SECS).exp()
            })
            .unwrap_or(0.0)
    }

    /// Ring contents ordered oldest..newest (last element = bucket of last activity).
    pub fn sparkline(&self, id: NodeId) -> Option<[i64; RING_BUCKETS]> {
        self.map.get(&id).map(|a| {
            let mut out = [0i64; RING_BUCKETS];
            for (i, slot) in out.iter_mut().enumerate() {
                let b = a.last_bucket + 1 + i as u64; // oldest first
                *slot = a.ring[(b % RING_BUCKETS as u64) as usize];
            }
            out
        })
    }

    pub fn evict(&mut self, now: Instant) {
        self.map
            .retain(|_, a| now.duration_since(a.last_update).as_secs_f64() < EVICT_AFTER_SECS);
    }
}
```

- [ ] **Step 5: Run `cargo test --bin dum activity` — expect PASS (6 tests)**

- [ ] **Step 6: Commit**

```bash
git add src/bin/dum/activity.rs src/bin/dum/main.rs
git commit -m "add dum activity model with EWMA rates, decay, and sparkline rings

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: scanner.rs — initial walk thread

**Files:**
- Create: `src/bin/dum/scanner.rs`
- Modify: `src/bin/dum/main.rs` (add `mod scanner;`)

Integration tests use `tempfile` trees created by the test. Note: `DirEntry::metadata()` does NOT traverse symlinks (correct for us). Allocated size on APFS rounds up to block size, so tests assert `size >= content_len`, never exact equality.

- [ ] **Step 1: Add `mod scanner;` to `src/bin/dum/main.rs`**

- [ ] **Step 2: Write failing tests in `src/bin/dum/scanner.rs`**

```rust
//! Initial filesystem walk: streams directory listings to the UI thread.

use std::ffi::OsString;
use std::fs::Metadata;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc;

    /// Build: root/{small.txt (5 bytes), sub/{big.bin (100_000 bytes)}}
    fn fixture() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        fs::write(td.path().join("small.txt"), b"hello").unwrap();
        fs::create_dir(td.path().join("sub")).unwrap();
        fs::write(td.path().join("sub/big.bin"), vec![0u8; 100_000]).unwrap();
        td
    }

    fn run_scan(root: PathBuf) -> Vec<ScanMsg> {
        let (tx, rx) = mpsc::channel();
        scan(root, tx);
        rx.try_iter().collect()
    }

    #[test]
    fn scan_emits_dirs_then_done_with_counts() {
        let td = fixture();
        let msgs = run_scan(td.path().to_path_buf());
        let dirs: Vec<_> = msgs
            .iter()
            .filter_map(|m| match m {
                ScanMsg::Dir { path, entries, .. } => Some((path.clone(), entries.len())),
                _ => None,
            })
            .collect();
        assert_eq!(dirs.len(), 2); // root and sub
        assert_eq!(dirs[0].0, td.path()); // root listed first
        assert_eq!(dirs[0].1, 2); // small.txt + sub
        match msgs.last().unwrap() {
            ScanMsg::Done { dirs, files, errors } => {
                assert_eq!(*dirs, 1); // sub
                assert_eq!(*files, 2);
                assert_eq!(*errors, 0);
            }
            _ => panic!("last message must be Done"),
        }
    }

    #[test]
    fn entries_have_allocated_sizes_and_kinds() {
        let td = fixture();
        let msgs = run_scan(td.path().to_path_buf());
        let ScanMsg::Dir { entries, .. } = &msgs[0] else { panic!() };
        let small = entries.iter().find(|e| e.name == "small.txt").unwrap();
        let sub = entries.iter().find(|e| e.name == "sub").unwrap();
        assert!(!small.is_dir);
        assert!(small.size >= 5); // allocated >= content length
        assert!(sub.is_dir);
        assert_eq!(sub.size, 0); // dirs start at 0; children roll up in the tree
    }

    #[cfg(unix)]
    #[test]
    fn denied_directory_is_flagged_and_counted() {
        use std::os::unix::fs::PermissionsExt;
        let td = fixture();
        let locked = td.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let msgs = run_scan(td.path().to_path_buf());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap(); // cleanup
        let denied = msgs.iter().any(|m| matches!(
            m, ScanMsg::Dir { path, denied: true, .. } if *path == locked));
        assert!(denied);
        match msgs.last().unwrap() {
            ScanMsg::Done { errors, .. } => assert_eq!(*errors, 1),
            _ => panic!(),
        }
    }
}
```

- [ ] **Step 3: Run `cargo test --bin dum scanner` — expect FAIL**

- [ ] **Step 4: Implement above the tests module**

```rust
pub struct ScanEntry {
    pub name: OsString,
    /// Allocated bytes for files; 0 for dirs (their totals roll up in the tree).
    pub size: u64,
    pub is_dir: bool,
}

pub enum ScanMsg {
    Dir { path: PathBuf, entries: Vec<ScanEntry>, denied: bool },
    Done { dirs: u64, files: u64, errors: u64 },
}

/// Allocated (on-disk) bytes: what `du` reports, honest about sparse files.
#[cfg(unix)]
pub fn allocated_size(md: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    md.blocks() * 512
}

#[cfg(not(unix))]
pub fn allocated_size(md: &Metadata) -> u64 {
    md.len()
}

/// Walk `root` depth-first, streaming one `Dir` message per directory,
/// then `Done`. Symlinks are not followed. Send errors mean the UI is
/// gone — just stop.
pub fn scan(root: PathBuf, tx: Sender<ScanMsg>) {
    let (mut dirs, mut files, mut errors) = (0u64, 0u64, 0u64);
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => {
                errors += 1;
                if tx.send(ScanMsg::Dir { path: dir, entries: Vec::new(), denied: true }).is_err() {
                    return;
                }
                continue;
            }
        };
        let mut entries = Vec::new();
        for ent in rd.flatten() {
            let Ok(md) = ent.metadata() else {
                errors += 1;
                continue;
            };
            let is_dir = md.is_dir();
            if is_dir {
                dirs += 1;
                stack.push(dir.join(ent.file_name()));
            } else {
                files += 1;
            }
            entries.push(ScanEntry {
                name: ent.file_name(),
                size: if is_dir { 0 } else { allocated_size(&md) },
                is_dir,
            });
        }
        if tx.send(ScanMsg::Dir { path: dir, entries, denied: false }).is_err() {
            return;
        }
    }
    let _ = tx.send(ScanMsg::Done { dirs, files, errors });
}
```

NOTE: the first test asserts root is listed first; the stack-based walk pops the root first, so this holds. Later dir ordering is unspecified (tests don't depend on it).

- [ ] **Step 5: Run `cargo test --bin dum scanner` — expect PASS (3 tests on unix)**

- [ ] **Step 6: Commit**

```bash
git add src/bin/dum/scanner.rs src/bin/dum/main.rs
git commit -m "add dum scanner thread streaming directory listings

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: app.rs — state, message application, navigation

**Files:**
- Create: `src/bin/dum/app.rs`, `src/bin/dum/watcher.rs` (types ONLY; thread fn comes in Task 7)
- Modify: `src/bin/dum/main.rs` (add `mod app; mod watcher;`)

`DeltaMsg` lives in `watcher.rs` (its producer) but is needed by app tests now, so this task creates `watcher.rs` containing only the message types.

- [ ] **Step 1: Add `mod app;` and `mod watcher;` to `src/bin/dum/main.rs`**

- [ ] **Step 2: Create `src/bin/dum/watcher.rs` with types only**

```rust
//! Watcher messages (the notify thread itself is implemented separately).

use std::path::PathBuf;

pub enum DeltaKind {
    /// Path exists; `new_size` is its current allocated size. Covers both
    /// modified and newly-created paths — the UI decides by tree lookup.
    Changed,
    /// Path no longer stats; it was removed.
    Removed,
}

pub struct DeltaMsg {
    pub path: PathBuf,
    pub kind: DeltaKind,
    pub new_size: Option<u64>, // Some for Changed, None for Removed
    pub is_dir: bool,          // false for Removed
}

pub enum WatchMsg {
    Delta(DeltaMsg),
    /// Watch failed or overflowed; live updates are unreliable until rescan.
    Degraded,
}
```

- [ ] **Step 3: Write failing tests in `src/bin/dum/app.rs`**

```rust
//! dum application state: applies scan/delta messages, handles navigation.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};

use crate::activity::ActivityMap;
use crate::scanner::{ScanEntry, ScanMsg};
use crate::tree::{NodeId, Tree};
use crate::watcher::{DeltaKind, DeltaMsg};

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, size: u64, is_dir: bool) -> ScanEntry {
        ScanEntry { name: OsString::from(name), size, is_dir }
    }

    /// root/{a/{f1: 100}, g: 50}
    fn scanned_app() -> App {
        let mut app = App::new(Path::new("/r"), Instant::now());
        app.apply_scan(ScanMsg::Dir {
            path: PathBuf::from("/r"),
            entries: vec![entry("a", 0, true), entry("g", 50, false)],
            denied: false,
        });
        app.apply_scan(ScanMsg::Dir {
            path: PathBuf::from("/r/a"),
            entries: vec![entry("f1", 100, false)],
            denied: false,
        });
        app.apply_scan(ScanMsg::Done { dirs: 1, files: 2, errors: 0 });
        app
    }

    fn changed(path: &str, size: u64) -> DeltaMsg {
        DeltaMsg {
            path: PathBuf::from(path),
            kind: DeltaKind::Changed,
            new_size: Some(size),
            is_dir: false,
        }
    }

    #[test]
    fn scan_messages_build_tree_with_rollups() {
        let app = scanned_app();
        assert_eq!(app.tree.get(app.tree.root).size, 150);
        assert!(!app.scanning);
        let a = app.tree.lookup(Path::new("/r/a")).unwrap();
        assert_eq!(app.tree.get(a).size, 100);
    }

    #[test]
    fn delta_changed_updates_sizes_and_activity_up_the_chain() {
        let mut app = scanned_app();
        let now = Instant::now();
        app.apply_delta(changed("/r/a/f1", 300), now);
        let a = app.tree.lookup(Path::new("/r/a")).unwrap();
        assert_eq!(app.tree.get(a).size, 300);
        assert_eq!(app.tree.get(app.tree.root).size, 350);
        let f1 = app.tree.lookup(Path::new("/r/a/f1")).unwrap();
        assert!(app.activity.glow(f1, now) > 0.0);
        assert!(app.activity.glow(a, now) > 0.0);
        assert!(app.activity.glow(app.tree.root, now) > 0.0);
    }

    #[test]
    fn delta_for_unknown_path_with_known_parent_inserts_node() {
        let mut app = scanned_app();
        app.apply_delta(changed("/r/a/new.bin", 500), Instant::now());
        assert!(app.tree.lookup(Path::new("/r/a/new.bin")).is_some());
        assert_eq!(app.tree.get(app.tree.root).size, 650);
    }

    #[test]
    fn delta_for_unknown_parent_is_dropped() {
        let mut app = scanned_app();
        app.apply_delta(changed("/r/unscanned/deep.bin", 999), Instant::now());
        assert!(app.tree.lookup(Path::new("/r/unscanned/deep.bin")).is_none());
        assert_eq!(app.tree.get(app.tree.root).size, 150);
    }

    #[test]
    fn delta_removed_subtracts_subtree_and_records_negative_activity() {
        let mut app = scanned_app();
        let now = Instant::now();
        app.apply_delta(
            DeltaMsg {
                path: PathBuf::from("/r/a"),
                kind: DeltaKind::Removed,
                new_size: None,
                is_dir: false,
            },
            now,
        );
        assert_eq!(app.tree.get(app.tree.root).size, 50);
        assert!(app.tree.lookup(Path::new("/r/a")).is_none());
        assert!(app.activity.glow(app.tree.root, now) < 0.0);
    }

    #[test]
    fn zero_delta_records_no_activity() {
        let mut app = scanned_app();
        let now = Instant::now();
        app.apply_delta(changed("/r/g", 50), now); // same size
        let g = app.tree.lookup(Path::new("/r/g")).unwrap();
        assert_eq!(app.activity.glow(g, now), 0.0);
    }

    #[test]
    fn sorted_children_by_size_desc_then_name() {
        let app = scanned_app();
        let kids = app.sorted_children(app.tree.root);
        assert_eq!(app.tree.get(kids[0]).name, "a"); // 100
        assert_eq!(app.tree.get(kids[1]).name, "g"); // 50
    }

    #[test]
    fn navigation_descend_and_up() {
        let mut app = scanned_app();
        app.on_key(KeyEvent::from(KeyCode::Enter)); // into "a" (largest, selected=0)
        let a = app.tree.lookup(Path::new("/r/a")).unwrap();
        assert_eq!(app.current, a);
        app.on_key(KeyEvent::from(KeyCode::Char('h'))); // back up
        assert_eq!(app.current, app.tree.root);
    }

    #[test]
    fn quit_help_and_rescan_keys() {
        let mut app = scanned_app();
        app.on_key(KeyEvent::from(KeyCode::Char('?')));
        assert!(app.show_help);
        app.on_key(KeyEvent::from(KeyCode::Esc)); // closes help first
        assert!(!app.show_help);
        assert!(!app.should_quit);
        app.on_key(KeyEvent::from(KeyCode::Char('r')));
        assert!(app.rescan_requested);
        app.on_key(KeyEvent::from(KeyCode::Char('q')));
        assert!(app.should_quit);
    }
}
```

- [ ] **Step 4: Run `cargo test --bin dum app` — expect FAIL**

- [ ] **Step 5: Implement above the tests module**

```rust
pub struct App {
    pub tree: Tree,
    pub activity: ActivityMap,
    pub root_path: PathBuf,
    pub current: NodeId,
    pub selected: usize,
    pub scanning: bool,
    pub watching: bool,
    pub watch_degraded: bool,
    pub show_help: bool,
    pub should_quit: bool,
    pub rescan_requested: bool,
    pub items_seen: u64,
    pub done_stats: Option<(u64, u64, u64)>, // dirs, files, errors
}

impl App {
    pub fn new(root: &Path, now: Instant) -> Self {
        let tree = Tree::new(root);
        let current = tree.root;
        App {
            tree,
            activity: ActivityMap::new(now),
            root_path: root.to_path_buf(),
            current,
            selected: 0,
            scanning: true,
            watching: true,
            watch_degraded: false,
            show_help: false,
            should_quit: false,
            rescan_requested: false,
            items_seen: 0,
            done_stats: None,
        }
    }

    /// Reset tree + activity for a rescan; watcher state is unaffected.
    pub fn reset(&mut self, now: Instant) {
        self.tree = Tree::new(&self.root_path);
        self.activity = ActivityMap::new(now);
        self.current = self.tree.root;
        self.selected = 0;
        self.scanning = true;
        self.items_seen = 0;
        self.done_stats = None;
    }

    pub fn apply_scan(&mut self, msg: ScanMsg) {
        match msg {
            ScanMsg::Dir { path, entries, denied } => {
                let Some(dir_id) = self.tree.lookup(&path) else { return };
                if denied {
                    self.tree.get_mut(dir_id).denied = true;
                    return;
                }
                self.items_seen += entries.len() as u64;
                for e in entries {
                    let child_path = path.join(&e.name);
                    self.tree.insert(dir_id, child_path, e.name, e.size, e.is_dir);
                }
            }
            ScanMsg::Done { dirs, files, errors } => {
                self.scanning = false;
                self.done_stats = Some((dirs, files, errors));
            }
        }
    }

    pub fn apply_delta(&mut self, msg: DeltaMsg, now: Instant) {
        match msg.kind {
            DeltaKind::Changed => {
                let new_size = msg.new_size.unwrap_or(0);
                let id = match self.tree.lookup(&msg.path) {
                    Some(id) => id,
                    None => {
                        // Insert only under an already-scanned parent; otherwise the
                        // scanner will pick it up (avoids double counting).
                        let Some(parent) = msg.path.parent().and_then(|p| self.tree.lookup(p))
                        else {
                            return;
                        };
                        let name = msg
                            .path
                            .file_name()
                            .map(|n| n.to_os_string())
                            .unwrap_or_default();
                        let id =
                            self.tree
                                .insert(parent, msg.path.clone(), name, new_size, msg.is_dir);
                        self.record_chain(id, new_size as i64, now);
                        return;
                    }
                };
                let delta = self.tree.set_size(id, new_size);
                if delta != 0 {
                    self.record_chain(id, delta, now);
                }
            }
            DeltaKind::Removed => {
                let Some(id) = self.tree.lookup(&msg.path) else { return };
                let parent = self.tree.get(id).parent;
                let delta = self.tree.remove(id, &msg.path);
                if delta != 0 {
                    if let Some(p) = parent {
                        self.record_chain(p, delta, now);
                    }
                }
                self.clamp_selection();
            }
        }
    }

    fn record_chain(&mut self, id: NodeId, delta: i64, now: Instant) {
        for anc in self.tree.ancestors_inclusive(id) {
            self.activity.record(anc, delta, now);
        }
    }

    /// Children of `dir`, size descending, ties by name ascending.
    pub fn sorted_children(&self, dir: NodeId) -> Vec<NodeId> {
        let mut kids = self.tree.get(dir).children.clone();
        kids.sort_by(|&a, &b| {
            let (na, nb) = (self.tree.get(a), self.tree.get(b));
            nb.size.cmp(&na.size).then_with(|| na.name.cmp(&nb.name))
        });
        kids
    }

    fn clamp_selection(&mut self) {
        let n = self.tree.get(self.current).children.len();
        if n == 0 {
            self.selected = 0;
        } else if self.selected >= n {
            self.selected = n - 1;
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc if self.show_help => self.show_help = false,
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = !self.show_help,
            KeyCode::Char('r') => self.rescan_requested = true,
            KeyCode::Down | KeyCode::Char('j') => {
                let n = self.tree.get(self.current).children.len();
                if n > 0 && self.selected + 1 < n {
                    self.selected += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                let kids = self.sorted_children(self.current);
                if let Some(&id) = kids.get(self.selected) {
                    if self.tree.get(id).is_dir {
                        self.current = id;
                        self.selected = 0;
                    }
                }
            }
            KeyCode::Backspace | KeyCode::Left | KeyCode::Char('u') | KeyCode::Char('h') => {
                if let Some(p) = self.tree.get(self.current).parent {
                    self.current = p;
                    self.selected = 0;
                }
            }
            _ => {}
        }
    }
}
```

- [ ] **Step 6: Run `cargo test --bin dum app` — expect PASS (9 tests)**

- [ ] **Step 7: Commit**

```bash
git add src/bin/dum/app.rs src/bin/dum/watcher.rs src/bin/dum/main.rs
git commit -m "add dum app state with delta application and navigation

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: watcher.rs — notify thread with coalescing

**Files:**
- Modify: `src/bin/dum/watcher.rs` (add the thread fn below the existing types)

notify 8 API used: `notify::recommended_watcher(closure)`, `watcher.watch(&path, RecursiveMode::Recursive)`, `event.need_rescan()`, `event.kind.is_access()`. The watcher reports OBSERVED sizes; it never computes deltas (the UI thread owns that).

Integration tests hit real FSEvents: generous timeouts, tempdir-only mutations. If these prove flaky on CI later, mark them `#[ignore]` — do not weaken assertions.

- [ ] **Step 1: Write failing tests (append to the existing `watcher.rs`)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// Wait up to `secs` for a delta matching `pred`.
    fn wait_for(
        rx: &mpsc::Receiver<WatchMsg>,
        secs: u64,
        pred: impl Fn(&DeltaMsg) -> bool,
    ) -> Option<DeltaMsg> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(WatchMsg::Delta(d)) if pred(&d) => return Some(d),
                Ok(_) => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => return None,
            }
        }
        None
    }

    #[test]
    fn watcher_reports_create_and_remove() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().canonicalize().unwrap();
        let (tx, rx) = mpsc::channel();
        {
            let root = root.clone();
            std::thread::spawn(move || watch(root, tx));
        }
        std::thread::sleep(Duration::from_millis(500)); // FSEvents warmup

        let target = root.join("blob.bin");
        fs::write(&target, vec![0u8; 200_000]).unwrap();
        let d = wait_for(&rx, 10, |d| {
            d.path == target && matches!(d.kind, DeltaKind::Changed)
        })
        .expect("expected Changed for created file");
        assert!(d.new_size.unwrap() >= 200_000);
        assert!(!d.is_dir);

        fs::remove_file(&target).unwrap();
        wait_for(&rx, 10, |d| {
            d.path == target && matches!(d.kind, DeltaKind::Removed)
        })
        .expect("expected Removed after deletion");
    }
}
```

- [ ] **Step 2: Run `cargo test --bin dum watcher` — expect FAIL (no `watch` fn)**

- [ ] **Step 3: Implement (below the types, above the tests)**

```rust
use std::collections::BTreeSet;
use std::sync::mpsc::{self as std_mpsc, Sender};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

use crate::scanner::allocated_size;

const COALESCE_MS: u64 = 100;

/// Watch `root` recursively; coalesce events ~100ms; stat each unique path
/// once per batch and report its observed state. Runs until the UI drops
/// the receiving end.
pub fn watch(root: std::path::PathBuf, tx: Sender<WatchMsg>) {
    let (raw_tx, raw_rx) = std_mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = match notify::recommended_watcher(move |res| {
        let _ = raw_tx.send(res);
    }) {
        Ok(w) => w,
        Err(_) => {
            let _ = tx.send(WatchMsg::Degraded);
            return;
        }
    };
    if watcher.watch(&root, RecursiveMode::Recursive).is_err() {
        let _ = tx.send(WatchMsg::Degraded);
        return;
    }

    loop {
        // Block for the first event of a batch.
        let first = match raw_rx.recv() {
            Ok(res) => res,
            Err(_) => return, // notify died; nothing more will come
        };
        let mut paths = BTreeSet::new();
        collect(first, &mut paths, &tx);
        // Coalesce everything arriving within the window.
        let deadline = Instant::now() + Duration::from_millis(COALESCE_MS);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match raw_rx.recv_timeout(remaining) {
                Ok(res) => collect(res, &mut paths, &tx),
                Err(_) => break,
            }
        }
        // One stat per unique path; report observed state.
        for path in paths {
            let msg = match std::fs::symlink_metadata(&path) {
                Ok(md) => DeltaMsg {
                    path,
                    kind: DeltaKind::Changed,
                    new_size: Some(if md.is_dir() { 0 } else { allocated_size(&md) }),
                    is_dir: md.is_dir(),
                },
                Err(_) => DeltaMsg { path, kind: DeltaKind::Removed, new_size: None, is_dir: false },
            };
            if tx.send(WatchMsg::Delta(msg)).is_err() {
                return; // UI gone
            }
        }
    }
}

fn collect(
    res: notify::Result<notify::Event>,
    paths: &mut BTreeSet<std::path::PathBuf>,
    tx: &Sender<WatchMsg>,
) {
    match res {
        Ok(ev) => {
            if ev.need_rescan() {
                let _ = tx.send(WatchMsg::Degraded);
            }
            if !ev.kind.is_access() {
                paths.extend(ev.paths);
            }
        }
        Err(_) => {
            let _ = tx.send(WatchMsg::Degraded);
        }
    }
}
```

NOTE on `Changed` for dirs: a dir's observed `new_size` is reported as 0, but `App::apply_delta` calls `set_size` only on lookup hits — for an existing dir this would zero its rolled-up total. Guard in apply_delta is required: **add this refinement to `App::apply_delta`** (Changed branch, lookup-hit path) and a test:

```rust
// In App::apply_delta, DeltaKind::Changed, after `Some(id) =>` lookup hit:
if msg.is_dir {
    return; // dir totals come from children; a dir event itself carries no size
}
```

And in `src/bin/dum/app.rs` tests:

```rust
#[test]
fn changed_event_for_existing_dir_does_not_clobber_rollup() {
    let mut app = scanned_app();
    app.apply_delta(
        DeltaMsg {
            path: PathBuf::from("/r/a"),
            kind: DeltaKind::Changed,
            new_size: Some(0),
            is_dir: true,
        },
        Instant::now(),
    );
    let a = app.tree.lookup(Path::new("/r/a")).unwrap();
    assert_eq!(app.tree.get(a).size, 100); // unchanged
}
```

- [ ] **Step 4: Run `cargo test --bin dum` — expect PASS (all dum tests incl. the new app test and the watcher integration test)**

If the API differs at compile time (notify minor drift), check docs.rs for the resolved version and adjust only the construction calls; the coalesce/stat/report contract must not change.

- [ ] **Step 5: Commit**

```bash
git add src/bin/dum/watcher.rs src/bin/dum/app.rs
git commit -m "add dum watcher thread with event coalescing and stat reporting

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: ui.rs — responsive rendering with glow

**Files:**
- Create: `src/bin/dum/ui.rs`
- Modify: `src/bin/dum/main.rs` (add `mod ui;`)

Column widths: RATE 9 (`+12.3M/s`), SPARK 8 (8 newest buckets), SIZE 5, BAR 16 (`[` + 10 cells + `]` + ` 99%`), NAME Min(10). Spacing 1. Tiers: ≥64 all columns; ≥48 drop SPARK; else drop RATE too (SIZE+BAR+NAME floor).

- [ ] **Step 1: Add `mod ui;` to `src/bin/dum/main.rs`**

- [ ] **Step 2: Write failing tests in `src/bin/dum/ui.rs`**

```rust
//! dum rendering: responsive activity table, glow styling, help overlay.

use std::time::Instant;

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table};
use ratatui::Frame;

use ful::format::{human_bytes, human_rate};

use crate::activity::RING_BUCKETS;
use crate::app::App;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::{ScanEntry, ScanMsg};
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    fn buffer_text(buf: &Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    fn demo_app() -> App {
        let mut app = App::new(Path::new("/r"), Instant::now());
        app.apply_scan(ScanMsg::Dir {
            path: PathBuf::from("/r"),
            entries: vec![
                ScanEntry { name: OsString::from("big"), size: 0, is_dir: true },
                ScanEntry { name: OsString::from("file.txt"), size: 1024, is_dir: false },
            ],
            denied: false,
        });
        app.apply_scan(ScanMsg::Done { dirs: 1, files: 1, errors: 0 });
        app
    }

    fn render(app: &App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(app, f, Instant::now())).unwrap();
        buffer_text(term.backend().buffer())
    }

    #[test]
    fn column_tiers_by_width() {
        assert_eq!(columns_for_width(80), vec![Col::Rate, Col::Spark, Col::Size, Col::Bar, Col::Name]);
        assert_eq!(columns_for_width(50), vec![Col::Rate, Col::Size, Col::Bar, Col::Name]);
        assert_eq!(columns_for_width(40), vec![Col::Size, Col::Bar, Col::Name]);
    }

    #[test]
    fn glow_style_sign_and_magnitude() {
        assert_eq!(glow_style(0.0), Style::default());
        let g = glow_style(50_000.0);
        assert_eq!(g.fg, Some(Color::Green));
        let r = glow_style(-5_000_000.0);
        assert_eq!(r.fg, Some(Color::Red));
        assert!(r.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn spark_renders_eight_scaled_chars() {
        let mut ring = [0i64; RING_BUCKETS];
        ring[RING_BUCKETS - 1] = 100; // newest, max
        ring[RING_BUCKETS - 2] = 50;
        let s = spark(&ring);
        assert_eq!(s.chars().count(), 8);
        assert!(s.ends_with('█'));
    }

    #[test]
    fn signed_rate_formats_with_sign() {
        assert_eq!(signed_rate(1_258_291.0), "+1.2M/s");
        assert_eq!(signed_rate(-512.0), "-512 B/s");
    }

    #[test]
    fn full_width_render_shows_headers_and_entries() {
        let text = render(&demo_app(), 80, 12);
        assert!(text.contains("dum"));
        assert!(text.contains("RATE"));
        assert!(text.contains("LAST 30s"));
        assert!(text.contains("big"));
        assert!(text.contains("file.txt"));
    }

    #[test]
    fn narrow_render_drops_spark_and_rate() {
        let text = render(&demo_app(), 40, 12);
        assert!(!text.contains("LAST 30s"));
        assert!(!text.contains("RATE"));
        assert!(text.contains("SIZE"));
    }

    #[test]
    fn empty_dir_and_help_overlay() {
        let mut app = demo_app();
        app.show_help = true;
        let text = render(&app, 80, 14);
        assert!(text.contains("keybindings"));
    }
}
```

- [ ] **Step 3: Run `cargo test --bin dum ui` — expect FAIL**

- [ ] **Step 4: Implement above the tests module**

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Col {
    Rate,
    Spark,
    Size,
    Bar,
    Name,
}

pub fn columns_for_width(width: u16) -> Vec<Col> {
    use Col::*;
    if width >= 64 {
        vec![Rate, Spark, Size, Bar, Name]
    } else if width >= 48 {
        vec![Rate, Size, Bar, Name]
    } else {
        vec![Size, Bar, Name]
    }
}

fn col_constraint(c: Col) -> Constraint {
    match c {
        Col::Rate => Constraint::Length(9),
        Col::Spark => Constraint::Length(8),
        Col::Size => Constraint::Length(5),
        Col::Bar => Constraint::Length(16),
        Col::Name => Constraint::Min(10),
    }
}

fn header_label(c: Col) -> &'static str {
    match c {
        Col::Rate => "RATE",
        Col::Spark => "LAST 30s",
        Col::Size => "SIZE",
        Col::Bar => "USAGE",
        Col::Name => "NAME",
    }
}

/// Style for a glow value: sign -> color, log-magnitude -> emphasis band.
pub fn glow_style(glow: f64) -> Style {
    let mag = glow.abs();
    if mag < 1.0 {
        return Style::default();
    }
    let color = if glow > 0.0 { Color::Green } else { Color::Red };
    let style = Style::default().fg(color);
    if mag >= 1_000_000.0 {
        style.add_modifier(Modifier::BOLD)
    } else if mag < 1_000.0 {
        style.add_modifier(Modifier::DIM)
    } else {
        style
    }
}

/// 8-char sparkline of the newest 8 ring buckets, scaled to their max |value|.
pub fn spark(ring: &[i64; RING_BUCKETS]) -> String {
    const CHARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let newest = &ring[RING_BUCKETS - 8..];
    let max = newest.iter().map(|v| v.abs()).max().unwrap_or(0);
    newest
        .iter()
        .map(|&v| {
            if max == 0 || v == 0 {
                CHARS[0]
            } else {
                let idx = ((v.abs() as f64 / max as f64) * 7.0).round() as usize;
                CHARS[idx.min(7)]
            }
        })
        .collect()
}

pub fn signed_rate(glow: f64) -> String {
    let sign = if glow >= 0.0 { "+" } else { "-" };
    format!("{}{}", sign, human_rate(glow.abs()))
}

fn usage_bar(pct: f64) -> String {
    let cells = 10usize;
    let filled = ((pct / 100.0).clamp(0.0, 1.0) * cells as f64).round() as usize;
    let mut s = String::from("[");
    for i in 0..cells {
        s.push(if i < filled.min(cells) { '█' } else { '░' });
    }
    s.push(']');
    s
}

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

pub fn draw(app: &App, frame: &mut Frame, now: Instant) {
    let area = frame.area();
    let [title_area, table_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_title(app, frame, title_area);
    draw_rows(app, frame, table_area, now);
    draw_footer(app, frame, footer_area);

    if app.show_help {
        draw_help(frame, area);
    }
}

fn draw_title(app: &App, frame: &mut Frame, area: Rect) {
    let status = if !app.watching {
        "not watching"
    } else if app.watch_degraded {
        "degraded"
    } else {
        "watching"
    };
    let total = human_bytes(app.tree.get(app.tree.root).size);
    let right = format!("{status} · total {total} ");
    let [l, r] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right.len() as u16 + 1)])
            .areas(area);
    let path = app.tree.path_of(app.current);
    frame.render_widget(
        Paragraph::new(Line::from(
            Span::from(format!(" dum — {}", path.display())).bold(),
        )),
        l,
    );
    frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), r);
}

fn draw_rows(app: &App, frame: &mut Frame, area: Rect, now: Instant) {
    let kids = app.sorted_children(app.current);
    if kids.is_empty() {
        let msg = if app.scanning { "scanning…" } else { "empty directory" };
        frame.render_widget(
            Paragraph::new(msg).alignment(Alignment::Center),
            area,
        );
        return;
    }
    let cols = columns_for_width(area.width);
    let dir_total = app.tree.get(app.current).size.max(1);
    let header = Row::new(
        cols.iter()
            .map(|c| Cell::from(header_label(*c)))
            .collect::<Vec<_>>(),
    )
    .style(Style::new().bold());

    let body: Vec<Row> = kids
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            let node = app.tree.get(id);
            let glow = app.activity.glow(id, now);
            let pct = node.size as f64 / dir_total as f64 * 100.0;
            let cells: Vec<Cell> = cols
                .iter()
                .map(|c| match c {
                    Col::Rate => {
                        if glow.abs() >= 1.0 {
                            Cell::from(signed_rate(glow)).style(glow_style(glow))
                        } else {
                            Cell::from("")
                        }
                    }
                    Col::Spark => match app.activity.sparkline(id) {
                        Some(ring) => Cell::from(spark(&ring)).style(glow_style(glow)),
                        None => Cell::from(""),
                    },
                    Col::Size => Cell::from(human_bytes(node.size)),
                    Col::Bar => Cell::from(format!("{} {:>2.0}%", usage_bar(pct), pct)),
                    Col::Name => {
                        let mut label = node.name.to_string_lossy().into_owned();
                        if node.is_dir {
                            label.push('/');
                        }
                        if node.denied {
                            label.push_str(" [denied]");
                        }
                        let w = area.width.saturating_sub(40).max(10) as usize;
                        Cell::from(truncate_ellipsis(&label, w)).style(glow_style(glow))
                    }
                })
                .collect();
            let row = Row::new(cells);
            if i == app.selected {
                row.style(Style::new().add_modifier(Modifier::REVERSED))
            } else {
                row
            }
        })
        .collect();

    let widths: Vec<Constraint> = cols.iter().map(|c| col_constraint(*c)).collect();
    let table = Table::new(body, widths).header(header).column_spacing(1);
    frame.render_widget(table, area);
}

fn draw_footer(app: &App, frame: &mut Frame, area: Rect) {
    let left = " q quit  ? help  r rescan  ⏎ enter  u up";
    let right = if app.scanning {
        format!("scanning… {} items ", app.items_seen)
    } else if let Some((dirs, files, errors)) = app.done_stats {
        if errors > 0 {
            format!("{dirs} dirs · {files} files · {errors} errors ")
        } else {
            format!("{dirs} dirs · {files} files ")
        }
    } else {
        String::new()
    };
    let [l, r] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right.len() as u16 + 1)])
            .areas(area);
    frame.render_widget(Paragraph::new(left).style(Style::new().dim()), l);
    frame.render_widget(
        Paragraph::new(right)
            .alignment(Alignment::Right)
            .style(Style::new().dim()),
        r,
    );
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from("dum — keybindings"),
        Line::from(""),
        Line::from("  ↑↓ / jk        move selection"),
        Line::from("  ⏎ / l / →      enter directory"),
        Line::from("  u / h / ← / ⌫  go up"),
        Line::from("  r              rescan"),
        Line::from("  ?              toggle this help"),
        Line::from("  q / Esc        quit"),
        Line::from(""),
        Line::from("green = growing   red = shrinking"),
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

- [ ] **Step 5: Run `cargo test --bin dum ui` — expect PASS (7 tests)**

- [ ] **Step 6: Commit**

```bash
git add src/bin/dum/ui.rs src/bin/dum/main.rs
git commit -m "add dum responsive rendering with activity glow and sparklines

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: main.rs — wiring it all together

**Files:**
- Modify: `src/bin/dum/main.rs` (replace stub `fn main`, keep `mod` lines)

- [ ] **Step 1: Replace `src/bin/dum/main.rs` with the full program**

```rust
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
```

- [ ] **Step 2: Full verification**

Run: `cargo build --bins && cargo test && cargo clippy --all-targets`
Expected: both binaries build; all tests pass (ful's 22 + lib 4 included… total = lib 4 + ful 18 + dum ~32); clippy clean. Fix any clippy warnings without changing behavior.

- [ ] **Step 3: Commit**

```bash
git add src/bin/dum/main.rs
git commit -m "wire dum threads, event loop, and rescan handling

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: README "ful & dum" + manual verification

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Rewrite `README.md`**

```markdown
# ful & dum

Two terminal disk tools that go together:

- **ful** — a live dashboard of your mounted filesystems: usage bars,
  used/free/total, and per-device I/O. *Which disk is in trouble?*
- **dum** — an ncdu-style tree explorer that watches filesystem events and
  illuminates what's changing: directories receiving writes glow green with
  a live rate and sparkline, shrinking ones glow red, sizes update in place.
  *What exactly is moving?*

Install both with one command:

    cargo install ful

## ful

    ful                  # default 2s refresh
    ful --interval 1     # custom refresh interval (seconds)

Keys: `q`/Esc quit · `?` help · `a` toggle pseudo filesystems.

## dum

    dum                  # explore the current directory, watching live
    dum ~/src            # explore a specific path
    dum --no-watch PATH  # plain explorer, no live updates

Keys: arrows/`hjkl` move · `⏎` enter · `u`/`⌫` up · `r` rescan ·
`?` help · `q`/Esc quit.

dum is read-only: it never modifies, moves, or deletes anything.

## Notes

- Sizes are allocated (on-disk) bytes, like `du`.
- macOS reports I/O per physical device; if `sysinfo` surfaces no per-disk
  counters, ful's READ/s and WRITE/s columns show `—`.
- dum's live layer uses FSEvents on macOS (via `notify`). Hardlinks are
  counted naively. Linux/Windows are untested in v1.
- If event watching fails or overflows, dum keeps working as a plain
  explorer and shows "degraded" — press `r` to rescan.
```

- [ ] **Step 2: Manual verification on macOS (real TTY required)**

Run `cargo run --bin dum -- ~/src` in a real terminal and check:
- Tree fills in progressively under "scanning…"; totals settle.
- In another terminal, run a build or `dd if=/dev/zero of=/tmp/...` inside a
  *scratch directory you create* under the watched root: the row glows green,
  RATE shows `+X/s`, sparkline moves, sizes tick up.
- Delete that scratch file: red glow, negative rate, sizes tick down.
- Navigate: Enter into dirs, `u` up, selection follows.
- Resize below 64 then 48 cols: SPARK then RATE drop, never wraps.
- `?` overlay, `r` rescan, `q` quit with terminal restored.
- `cargo run --bin ful` still works unchanged.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "rewrite README as ful & dum

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review (completed)

**Spec coverage:** lib refactor → T1; bins+deps → T2; tree → T3; activity (EWMA/decay/ring/evict) → T4; scanner (allocated size, denied, progressive, Done) → T5; app (apply rules incl. unknown-path drop, removed accounting, navigation, rescan) → T6; watcher (coalesce, stat, degraded, need_rescan) → T7; ui (glow, rate, sparkline, live sizes via tree, responsive tiers, help, scanning/empty states) → T8; wiring (CLI, --no-watch, panic-safe term, rescan respawn, drain caps) → T9; README + manual verification → T10. Spec's `Created` variant folded into `Changed` — documented deviation in header.

**Placeholder scan:** none; all code steps complete.

**Type consistency:** `ScanEntry{name,size,is_dir}` and `ScanMsg::Dir{path,entries,denied}`/`Done{dirs,files,errors}` consistent across T5/T6/T8. `DeltaMsg{path,kind,new_size,is_dir}` + `WatchMsg::{Delta,Degraded}` consistent across T6/T7/T9. `Tree::{new,get,get_mut,lookup,insert,set_size,remove,ancestors_inclusive,path_of}` usage in app/ui matches T3 signatures. `ActivityMap::{new,record,glow,sparkline,evict}` + `RING_BUCKETS` match T4. `ful::term::{install_panic_hook,init}` and `ful::format::{human_bytes,human_rate}` match T1. `App` fields used by ui/main (`tree,activity,current,selected,scanning,watching,watch_degraded,show_help,should_quit,rescan_requested,items_seen,done_stats,root_path`) all defined in T6.
