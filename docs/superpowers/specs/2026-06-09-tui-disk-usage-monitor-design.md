# dum — TUI Disk Usage Monitor (Design)

Date: 2026-06-09

## Summary

`dum` is a terminal dashboard that shows a live, periodically-refreshing view of
mounted filesystems: how full each one is and how busy it is. It is a *monitor*
(a `df`-style live dashboard), not an interactive tree explorer like `ncdu`.

- **Language/stack:** Rust + [ratatui](https://ratatui.rs) with the crossterm backend.
- **Data source:** the [`sysinfo`](https://docs.rs/sysinfo) crate (capacity + per-disk I/O counters).
- **Platform target:** macOS first (development environment), but no macOS-only
  code in the core; sysinfo keeps a clean path to Linux later.
- **Refresh:** every 2 seconds by default (configurable via `--interval`).
- **Interaction:** view-only plus `q`/Esc quit, `?` help overlay, `a` toggle all mounts.

## Goals / Non-goals

**Goals**
- At-a-glance fullness (color-coded usage bar + %) and capacity (used/free/total) per mount.
- Live read/write throughput per device, derived from sampling deltas.
- Fits cleanly in 80 columns, and degrades gracefully (clipping, never wrapping)
  in narrower terminals.
- Restore the terminal correctly on quit, error, or panic.

**Non-goals (YAGNI)**
- No directory tree drilling / scanning (that's `ncdu`'s job).
- No sorting, row selection, or threshold-alert logic beyond bar color.
- No persistence, config files, or logging.
- No per-process I/O attribution.

## Architecture

Three layers with clean seams:

1. **Data source** — a `DataSource` trait with one method,
   `sample() -> Vec<DiskSample>`. The `SysinfoSource` implementation wraps
   `sysinfo`. The trait seam allows feeding deterministic fake samples in tests
   and swapping in an IOKit-based source later if sysinfo's macOS I/O proves too
   coarse.
2. **App state** — owns the latest rows, the *previous* raw sample (timestamp +
   per-device byte counters) used to compute I/O rates, the `show_all` toggle,
   the help-overlay flag, and config (refresh interval). State transitions are
   pure; this layer performs no I/O and no rendering.
3. **UI** — pure render functions taking `&App` and a ratatui `Frame`. No state
   mutation here.

The event loop (in `main`) polls crossterm events with a timeout equal to the
refresh interval:
- **timeout elapsed** → `app.tick(source.sample(), now)` then redraw
- **keypress** → `q`/Esc quit, `?` toggle help, `a` toggle `show_all`, then redraw
- **resize** → redraw

Terminal raw-mode setup/teardown is guarded so a panic always restores the terminal.

## Modules

| File | Responsibility |
|------|----------------|
| `main.rs` | CLI arg parsing (`--interval`, default 2s), `TerminalGuard`, panic hook, event loop |
| `datasource.rs` | `DataSource` trait + `SysinfoSource`; emits raw `DiskSample` |
| `app.rs` | `App` state, `tick()` (ingest sample, compute rates vs previous), key handlers |
| `model.rs` | `MountRow` (display-ready), real-vs-pseudo filtering by fs type |
| `ui.rs` | table render, help overlay, footer, responsive column selection |
| `format.rs` | human-readable byte sizes + per-second rates |

### Key types (sketch)

```rust
// datasource.rs — raw, untransformed reading
struct DiskSample {
    mount: String,
    device: String,      // short node, e.g. "disk3s1" (/dev/ stripped)
    fs_type: String,     // "apfs", "exfat", ...
    total: u64,
    available: u64,
    read_bytes: u64,     // cumulative counter; may be 0/None on macOS
    written_bytes: u64,
}

trait DataSource {
    fn sample(&mut self) -> Vec<DiskSample>;
}

// model.rs — display-ready, with derived fields
struct MountRow {
    mount: String,
    device: String,
    fs_type: String,
    used: u64,           // total - available
    free: u64,           // available
    total: u64,
    used_pct: f64,
    read_per_s: Option<f64>,   // None until two samples seen / if unavailable
    write_per_s: Option<f64>,
}
```

## Data flow

1. `SysinfoSource::sample()` refreshes sysinfo's disk list and returns `Vec<DiskSample>`.
2. `App::tick(samples, now)`:
   - For each sample, match against the previous sample by device to compute
     `rate = max(0, Δbytes) / Δseconds`. First tick (no previous) → rates are `None`.
   - Build `MountRow`s (derive used/pct).
   - Apply the real-vs-pseudo filter unless `show_all`.
   - Store current raw samples + `now` as the new "previous".
3. `ui::draw(&app, frame)` renders header, responsive table, footer, and the help
   overlay if toggled.

### Real-vs-pseudo filtering

Hide pseudo/virtual filesystems by default, keyed on `fs_type`. Excluded set
(case-insensitive): `devfs`, `autofs`, `tmpfs`, `proc`, `sysfs`, `overlay`,
`squashfs`, `nullfs`, plus any mount with `total == 0`. `a` toggles `show_all`
to reveal everything. The footer shows `showing N of M mounts`.

## UI layout (responsive, 80-col floor)

Color: usage bar is green `< 70%`, yellow `70–90%`, red `> 90%`. I/O cells show
`—` when no rate is available (first tick, or sysinfo reports no counter).

The table never wraps — it clips. ratatui length-constraints select columns by
width. Column priority, highest kept longest:

`MOUNT → USAGE → FREE → TOTL → READ/s → WRITE/s → USED → FS → DEV`

**Tiers**

- **≥ ~80 cols (full):** MOUNT, FS, USAGE, USED, FREE, TOTL, READ/s, WRITE/s, DEV
- **~56–79:** MOUNT, USAGE, FREE, TOTL, READ/s, WRITE/s
- **< ~56:** drop I/O first, then FREE — worst case MOUNT + usage bar

Full-width mockup (≤ 80 cols):

```
 dum — disk usage monitor                                         refresh 2s

 MOUNT          FS    USAGE             USED  FREE  TOTL   READ/s  WRITE/s  DEV
 /              apfs  [████████░░] 68%  340G  160G  500G   1.2M/s   0 B/s   disk3s1
 /System/Vol…   apfs  [█████████░] 91%  455G   45G  500G   0 B/s    3.4M/s  disk1s1
 /Volumes/Ext   exfat [██░░░░░░░░] 12%  240G  1.7T  2.0T   0 B/s    0 B/s   disk4s2

 q quit   ? help   a toggle all                                  showing 3 of 7 mounts
```

Mid tier (~56–79 cols):

```
 dum — disk usage monitor             refresh 2s

 MOUNT       USAGE         FREE  TOTL  READ/s WRITE/s
 /           [██████░] 68% 160G  500G  1.2M/s 0 B/s
 /System/Vo… [██████▉] 91%  45G  500G  0 B/s  3.4M/s
 /Volumes/E… [█░░░░░░] 12% 1.7T  2.0T  0 B/s  0 B/s

 q quit  ? help  a toggle         3 of 7 mounts
```

MOUNT and DEV truncate with a trailing `…` when they exceed their column width.

## I/O on macOS — known limitation

macOS exposes I/O per *physical device*, not per mount point. We rely on
sysinfo's per-disk counters. If sysinfo returns no/zero counters on macOS, the
READ/s and WRITE/s columns degrade to `—` rather than failing. Rates are
computed as `Δbytes / Δseconds` between two consecutive samples, so the first
2-second window always shows `—`. Accurate per-device I/O via IOKit FFI is a
possible future swap behind the `DataSource` trait, explicitly out of scope now.

## Error handling

- **Terminal safety:** a `TerminalGuard` whose `Drop` disables raw mode and
  leaves the alternate screen, plus a panic hook that does the same before
  printing the panic — a crash never leaves a broken terminal.
- **No disks:** render a centered "No filesystems found" message.
- **Missing I/O counters:** per-cell `—`, never an error.
- **Sampling errors:** sysinfo calls are infallible in practice; an empty list
  is treated as the no-disks case.

## Testing

The pure functions are the test surface:

- `format.rs` — byte sizes (`0 B`, `999 B`, `1.0K`, `1.2M`, `2.0T`) and rates
  (`0 B/s`, `1.2M/s`).
- `model.rs` — pseudo-fs filtering: known pseudo types and `total == 0` excluded;
  real disks kept; `show_all` keeps everything.
- `app::tick` — rate computation: feed two `DiskSample` vecs via a fake
  `DataSource` with known timestamps and byte deltas; assert `read_per_s` /
  `write_per_s`; assert first tick yields `None`; assert counter resets (Δ < 0)
  clamp to `0`.
- `ui.rs` — smoke test: render one frame to a ratatui `TestBackend` buffer at
  80×24 and at 50×24; assert it does not panic and key labels are present.

## Build sequence (for the plan)

1. Cargo project + deps (`ratatui`, `crossterm`, `sysinfo`, arg parsing).
2. `format.rs` with tests.
3. `model.rs` + `MountRow` + filtering, with tests.
4. `datasource.rs` trait + `SysinfoSource`; a `FakeSource` for tests.
5. `app.rs` state + `tick` rate logic, with tests.
6. `ui.rs` responsive table + help overlay + footer; `TestBackend` smoke tests.
7. `main.rs` terminal guard, panic hook, arg parsing, event loop.
8. Manual run-through on macOS; verify 80-col and narrow rendering, I/O behavior.
