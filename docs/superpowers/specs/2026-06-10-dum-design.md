# dum — Live-Activity Disk Usage Explorer (Design)

Date: 2026-06-10

## Summary

`dum` is an ncdu-style interactive disk-usage tree explorer with a twist no
existing tool has: while you have it open, it listens to filesystem events and
**illuminates** the rows on screen — a directory receiving writes glows green
and shows its current growth rate; one being emptied glows red. The tree's
sizes update live.

`dum` ships **in the same crate as `ful`** (package `ful`, two binaries).
`cargo install ful` installs both. The README is titled **"ful & dum"**:
`ful` watches mounts fill up; `dum` shows what, exactly, is moving — two
friends going with each other.

- **Stack:** Rust, ratatui 0.29 + crossterm 0.28 (as ful), plus the
  [`notify`](https://docs.rs/notify) crate (FSEvents backend on macOS).
- **Platform:** macOS-first via `notify`'s cross-platform API; Linux/Windows
  are untested best-effort in v1.
- **Scope:** strictly read-only. No deletion in v1.

## Goals / Non-goals

**Goals**
- Core ncdu-style explorer: scan a path, navigate a size-sorted tree,
  percent bars, totals, rescan key.
- Live activity layer: per-row green/red glow with decay, live net-rate
  column, live-updating sizes, 30s rate sparkline.
- UI stays responsive during the initial scan (tree fills in progressively).
- Graceful degradation: if watching fails, dum remains a working explorer.
- Responsive layout, ~80-col happy path, degrades by dropping columns.

**Non-goals (v1)**
- No deletion or any FS mutation (read-only).
- No ncdu comfort/parity features: no apparent-size toggle, hidden-file
  toggle, sort modes, exclude patterns, hardlink detection, or scan
  export/import. (Hardlinks are counted naively; documented caveat.)
- No verified Linux/Windows support.
- No config files or persistence.

## Crate & repo layout

Same repo (`kek/ful`). The package gains a small library target shared by
both binaries:

```
Cargo.toml            # package "ful": [lib] + bin "ful" + bin "dum"
src/lib.rs            # shared library: pub mod format; pub mod term;
src/format.rs         # moved into the lib (human_bytes, human_rate)
src/term.rs           # NEW shared terminal lifecycle: TerminalGuard,
                      #   restore_terminal(), install_panic_hook()
src/main.rs           # ful binary (unchanged behavior; uses ful::format, ful::term)
src/app.rs ui.rs datasource.rs model.rs   # ful's modules, unchanged otherwise
src/bin/dum/
  main.rs             # CLI (path arg), thread spawn, event loop
  tree.rs             # arena tree: nodes, size rollups, path index
  scanner.rs          # initial walk thread -> ScanMsg
  watcher.rs          # notify -> debounce -> stat -> DeltaMsg thread
  activity.rs         # EWMA rates, exponential decay, sparkline rings
  app.rs              # dum state: apply messages, keys, navigation
  ui.rs               # tree view, glow rendering, responsive columns
```

Refactor note: ful's `main.rs` currently contains the guard/panic-hook code;
it moves to `src/term.rs` and `format.rs` moves into the lib. ful's behavior
and its existing tests are unchanged.

New dependency: `notify = "7"` (or current major; FSEvents on macOS).
Cargo installs **all** package binaries on `cargo install ful`.

## Architecture

Three threads; two `std::sync::mpsc` channels feed the UI thread, which owns
all state. No locks, no async runtime.

```
scanner thread ──ScanMsg──►┐
                           ├──► UI thread: drain channels, apply to tree,
watcher thread ──DeltaMsg──►┘    update activity, render (~10 fps)
```

### Messages

```rust
enum ScanMsg {
    // One scanned directory: its path and immediate entries (name, size, is_dir).
    Dir { path: PathBuf, entries: Vec<ScanEntry> },
    // Walk finished (counts for the status bar).
    Done { dirs: u64, files: u64, errors: u64 },
}

enum DeltaKind { Changed, Created, Removed }

struct DeltaMsg {
    path: PathBuf,
    kind: DeltaKind,
    new_size: Option<u64>, // None for Removed
}
```

### Scanner thread

- Walks the root depth-first using `std::fs` (symlinks NOT followed;
  `symlink_metadata`). Size = allocated bytes: `st_blocks * 512`
  (`MetadataExt::blocks()`), matching `du`/ncdu's default notion of usage.
- Sends one `ScanMsg::Dir` per directory (batched naturally); the UI builds
  the tree progressively so large trees appear as they're discovered, under a
  "scanning…" status with a running item count.
- Permission errors: count and skip; the directory node is marked denied.
- Sends `Done` at the end. The thread then exits.
- Rescan (`r` key): drop tree + activity state, spawn a fresh scanner;
  the watcher keeps running throughout.

### Watcher thread

- Starts immediately at launch (before the scan completes) with a single
  `notify` recursive watch on the root.
- Coalesces raw events into ~100ms batches, dedups by path, then for each
  unique path: `symlink_metadata` → `DeltaMsg{ Changed/Created, new_size }`,
  or `Removed` if the stat fails / event says remove.
- It does NOT compute signed deltas — it reports observed absolute sizes.
  The UI thread computes `signed_delta = new_size - tree_size` against its
  own tree, which keeps all bookkeeping in one place (single source of truth).
- Renames surface as Remove + Create of two paths; no special handling.
- If `notify` reports overflow/rescan or the watch dies: send a flag the UI
  shows as "live updates degraded — press r to rescan". Explorer keeps working.

### UI thread (event loop)

ful's loop shape: `event::poll` with a ~100ms frame timeout; on each
iteration drain both channels completely (`try_recv` until empty), apply
messages, handle input, render.

Applying a `DeltaMsg`:
1. Look up the node by path. Unknown path: if its parent directory exists in
   the tree, insert a new node; otherwise drop the message (the area hasn't
   been scanned yet — the scanner will capture its size, avoiding double
   count).
2. Compute `signed_delta` vs the stored size; update the node's size; add
   `signed_delta` to every ancestor's size up to the root (sizes stay
   truthful live).
3. Feed `signed_delta` into the activity model for the node **and each
   ancestor** — deep activity illuminates whatever ancestor row is visible.
4. `Removed`: subtract the node's stored size from ancestors, drop the
   subtree (its stored total is the best estimate of what vanished).

## Data model

```rust
// tree.rs
struct Node {
    name: OsString,
    size: u64,          // allocated bytes, includes subtree for dirs
    is_dir: bool,
    denied: bool,
    parent: Option<NodeId>,
    children: Vec<NodeId>, // unsorted; sorted at render
}
type NodeId = usize;     // index into Vec<Node> arena

struct Tree {
    nodes: Vec<Node>,
    by_path: HashMap<PathBuf, NodeId>, // event attribution
    root: NodeId,
}
```

Activity is sparse side-state — only nodes with recent events pay any cost:

```rust
// activity.rs
struct Activity {
    rate: f64,            // EWMA of signed bytes/sec
    last_update: Instant,
    ring: [i64; 30],      // net bytes per 1s bucket, last 30s (sparkline)
    ring_head: usize,
}
struct ActivityMap(HashMap<NodeId, Activity>);
```

- On delta: `rate = alpha * (delta/dt) + (1-alpha) * rate` (alpha ≈ 0.3),
  bucket the delta into the ring.
- At render: effective glow `g = rate * exp(-age/tau)`, tau = 2.5s.
  Sign → green (growing) / red (shrinking); brightness ∝ `log10(|g|)`
  banded into dim/normal/bold. Entries with negligible glow and stale rings
  are evicted from the map.

## UI

```
 dum — /Users/ke/src                            watching · total 2.1G

    RATE     LAST 30s   SIZE                      NAME
  +12.3M/s   ▁▁▂▄▆█▆▄   1.2G [████████░░] 58%  ▸ node_modules/
  -340K/s    ▆▃▁▁▁▂▁▁   480M [███░░░░░░░] 23%  ▸ target/
                        210M [█░░░░░░░░░] 10%  ▸ .git/
                         48M [░░░░░░░░░░]  2%    Cargo.lock
 q quit  ? help  r rescan                            scanning… 12,034 items
```

- Rows: current directory's entries, sorted by size descending (ties by
  name). Bar/% relative to the current directory's total (ncdu-style).
- RATE: live net rate for rows with glow, blank otherwise. LAST 30s:
  braille/block sparkline of |net| per bucket, colored by current sign.
- Name cell carries the glow color; `▸` marks directories; `[denied]`
  suffix on permission-skipped dirs.
- Header: root path, watch status (`watching` / `degraded`), total.
  Footer: keys + scan progress or item count.
- Responsive (ful-style fixed budgets): < ~64 cols drops LAST 30s;
  < ~48 drops RATE; floor is SIZE + bar + NAME. Never wraps; names
  truncate with `…`.
- Keys: `↑↓`/`jk` move, `Enter`/`l`/`→` descend, `u`/`Backspace`/`h`/`←`
  up, `r` rescan, `?` help overlay, `q`/Esc quit (Esc closes help first).
- Selection follows the entry (by name) across re-sorts where possible.

CLI: `dum [PATH]` (default `.`). `--no-watch` flag to run as a plain
explorer (also the fallback path if the watcher fails to start).

## Error handling

- Root missing / not a directory: clean CLI error before entering the TUI.
- Permission-denied subtrees: skipped, marked, counted in `Done.errors`.
- Watcher failure at startup or overflow mid-run: status-bar warning,
  explorer fully functional (degrades to ncdu-without-delete).
- Stat failures on event paths: treated as `Removed` if the node exists,
  otherwise ignored.
- Terminal safety: shared `TerminalGuard` bound immediately after
  `enable_raw_mode()` + panic hook (the pattern proven in ful, via `ful::term`).
- Channel disconnects (thread death) are tolerated: scanner disconnect after
  `Done` is normal; watcher disconnect flips status to degraded.

## Testing

All destructive/FS-touching tests operate **only on temp trees the test
itself creates** (`tempfile` crate); nothing ever touches a real user path.

- `tree.rs` (pure): insert, rollup arithmetic, remove-subtree, path index
  consistency.
- `activity.rs` (pure): EWMA math, exponential decay over simulated time,
  ring bucketing, eviction.
- `app.rs` (pure): feed synthetic `ScanMsg`/`DeltaMsg` sequences; assert
  sizes, ancestor rollups, unknown-path drop rule, removed-subtree
  accounting, navigation/selection behavior.
- `scanner.rs` (integration, tempdir): build a known tree, scan, assert
  emitted structure and sizes; permission-denied case via `chmod 000` dir.
- `watcher.rs` (integration, tempdir): start watcher, create/append/
  truncate/delete files, assert coalesced `DeltaMsg`s arrive (generous
  timeouts for FSEvents latency; marked for skip-on-CI if flaky).
- `ui.rs`: `TestBackend` smoke tests at 80×24 and 48×24; glow banding and
  column tiers unit-tested as pure functions.
- ful regression: ful's existing 22 tests must keep passing after the
  lib refactor.

## Build sequence (for the plan)

1. Refactor: introduce `src/lib.rs`, move `format.rs` in, extract `term.rs`
   from ful's main; ful tests stay green.
2. `Cargo.toml`: bin targets + `notify` + `tempfile` (dev-dep).
3. `tree.rs` with tests.
4. `activity.rs` with tests.
5. `scanner.rs` + `ScanMsg` with tempdir tests.
6. `app.rs` (apply/navigate) with synthetic-message tests.
7. `watcher.rs` + `DeltaMsg` with tempdir tests.
8. `ui.rs` responsive rendering + glow with TestBackend tests.
9. `main.rs` wiring: CLI, threads, event loop, degradation paths.
10. README rewrite as **"ful & dum"**; manual verification on macOS
    (watch `cargo build` light up `target/`, `npm install`, big deletes).

## Known limitations (documented in README)

- Hardlinks counted once per path (naive); sizes can over-count.
- FSEvents coalescing means very short-lived files may net out invisibly.
- Other-volume mount points inside the tree are scanned as ordinary dirs.
- Linux/Windows: `notify` abstracts inotify/ReadDirectoryChangesW but v1 is
  only verified on macOS (inotify watch limits on huge trees are a known
  caveat for later).
