# Backronyms in the UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** dum announces itself as "disk usage monitor" in its title bar and help overlay; ful gets a dynamic " ful — as in plentiful/watchful/stressful/dreadful" subtitle driven by the worst real disk's usage.

**Architecture:** Two small, independent UI changes. ful gains `App::worst_real_pct()` (max `used_pct` over non-pseudo filesystems) and a pure `ful_word()` threshold mapping in `src/ui.rs`, consumed by `draw_title`. dum's `draw_title` gains the subtitle with a width-aware fallback, and its help overlay header changes. Clap `about` strings and the README are updated to match.

**Tech Stack:** Rust, ratatui (TestBackend render tests), clap. Run tests with `cargo test`.

Spec: `docs/superpowers/specs/2026-06-10-backronyms-design.md`

---

### Task 1: ful — `App::worst_real_pct()`

The subtitle needs the worst real disk regardless of the `a` (show pseudo) toggle. Pseudo filesystems (devfs etc.) sit at 100% and would false-alarm, so they are always excluded here.

**Files:**
- Modify: `src/app.rs` (impl App, after `visible_rows`; tests at the bottom of the file's `mod tests`)

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/app.rs` (the `sample` helper already exists there; note `sample(dev, fs, total, avail, rd, wr)` and `used_pct` is derived as `(total-avail)/total*100`):

```rust
#[test]
fn worst_real_pct_is_max_over_real_filesystems() {
    let mut app = App::new(cfg());
    app.tick(
        vec![
            sample("disk1", "apfs", 100, 60, 0, 0),  // 40% used
            sample("disk2", "apfs", 100, 10, 0, 0),  // 90% used
            sample("vfs", "devfs", 100, 0, 0, 0),    // pseudo, 100% — must be ignored
        ],
        Instant::now(),
    );
    assert_eq!(app.worst_real_pct(), Some(90.0));
}

#[test]
fn worst_real_pct_none_without_real_filesystems() {
    let mut app = App::new(cfg());
    assert_eq!(app.worst_real_pct(), None); // empty
    app.tick(vec![sample("vfs", "devfs", 100, 0, 0, 0)], Instant::now());
    assert_eq!(app.worst_real_pct(), None); // pseudo only
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test worst_real_pct`
Expected: FAIL to compile with "no method named `worst_real_pct`"

- [ ] **Step 3: Implement**

Add to `impl App` in `src/app.rs`, right after `visible_rows`:

```rust
/// Highest usage percentage among real (non-pseudo) filesystems,
/// regardless of the `show_all` toggle. `None` if there are none.
pub fn worst_real_pct(&self) -> Option<f64> {
    self.all_rows
        .iter()
        .filter(|r| !is_pseudo_fs(&r.fs_type, r.total))
        .map(|r| r.used_pct)
        .fold(None, |acc: Option<f64>, p| Some(acc.map_or(p, |a| a.max(p))))
}
```

(`f64` is not `Ord`, hence the fold instead of `.max()`.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test worst_real_pct`
Expected: 2 passed

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "Add App::worst_real_pct for the dynamic title"
```

---

### Task 2: ful — `ful_word()` threshold mapping

**Files:**
- Modify: `src/ui.rs` (new pub fn near `bar_color`; tests in `mod tests`)

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/ui.rs`:

```rust
#[test]
fn ful_word_thresholds() {
    assert_eq!(ful_word(Some(0.0)), "plentiful");
    assert_eq!(ful_word(Some(59.9)), "plentiful");
    assert_eq!(ful_word(Some(60.0)), "watchful");
    assert_eq!(ful_word(Some(84.9)), "watchful");
    assert_eq!(ful_word(Some(85.0)), "stressful");
    assert_eq!(ful_word(Some(94.9)), "stressful");
    assert_eq!(ful_word(Some(95.0)), "dreadful");
    assert_eq!(ful_word(Some(100.0)), "dreadful");
}

#[test]
fn ful_word_falls_back_to_watchful() {
    assert_eq!(ful_word(None), "watchful");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test ful_word`
Expected: FAIL to compile with "cannot find function `ful_word`"

- [ ] **Step 3: Implement**

Add to `src/ui.rs`, right after `bar_color`:

```rust
/// The "…ful" word for the title, reacting to the worst real disk.
pub fn ful_word(worst_pct: Option<f64>) -> &'static str {
    match worst_pct {
        Some(p) if p < 60.0 => "plentiful",
        Some(p) if p < 85.0 => "watchful",
        Some(p) if p < 95.0 => "stressful",
        Some(_) => "dreadful",
        None => "watchful",
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test ful_word`
Expected: 2 passed

- [ ] **Step 5: Commit**

```bash
git add src/ui.rs
git commit -m "Add ful_word threshold mapping"
```

---

### Task 3: ful — dynamic title

**Files:**
- Modify: `src/ui.rs:160-170` (`draw_title`); tests in `mod tests`
- Modify: `src/main.rs:16` (clap `about`)

- [ ] **Step 1: Write the failing render test**

Add to `mod tests` in `src/ui.rs` (helpers `app_with_rows`, `row`, `buffer_text` already exist; `row(..., pct)` sets `used_pct` directly and `fs_type: "apfs"`, total 1000, so rows are real, not pseudo):

```rust
#[test]
fn title_word_reacts_to_worst_disk() {
    for (pct, word) in [(40.0, "plentiful"), (68.0, "watchful"), (92.0, "stressful"), (97.0, "dreadful")] {
        let app = app_with_rows(vec![row("/", "disk3s1", pct)]);
        let backend = TestBackend::new(80, 10);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| draw(&app, f)).unwrap();
        let text = buffer_text(term.backend().buffer());
        assert!(text.contains(&format!("ful — as in {word}")), "pct {pct}: expected {word}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test title_word_reacts`
Expected: FAIL — title still says "ful — disk usage monitor"

- [ ] **Step 3: Implement**

Replace `draw_title` in `src/ui.rs` (currently lines 160-170):

```rust
fn draw_title(app: &App, frame: &mut Frame, area: Rect) {
    let right = format!("refresh {}s ", app.config.interval.as_secs());
    let [l, r] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right.len() as u16 + 1)])
            .areas(area);
    let worst = app.worst_real_pct();
    // The word inherits the color the usage bar would have at that level.
    let word_color = bar_color(worst.unwrap_or(0.0));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::from(" ful — as in ").bold(),
            Span::styled(ful_word(worst), Style::new().fg(word_color).bold()),
        ])),
        l,
    );
    frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), r);
}
```

- [ ] **Step 4: Update the clap `about`**

In `src/main.rs:16`, "disk usage monitor" now belongs to dum:

```rust
#[command(name = "ful", about = "live filesystem dashboard — ful, as in watchful")]
```

- [ ] **Step 5: Run the full ful test suite**

Run: `cargo test`
Expected: all pass — `full_width_renders_all_headers` only asserts `text.contains("ful")`, which still holds.

- [ ] **Step 6: Commit**

```bash
git add src/ui.rs src/main.rs
git commit -m "Give ful a dynamic title word driven by the worst disk"
```

---

### Task 4: dum — "disk usage monitor" in title and help

**Files:**
- Modify: `src/bin/dum/ui.rs:152-173` (`draw_title`), `src/bin/dum/ui.rs:271-294` (`draw_help`); tests in `mod tests`
- Modify: `src/bin/dum/main.rs:22` (clap `about`)

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/bin/dum/ui.rs` (helpers `demo_app` and `render` already exist; `demo_app` has root `/r`):

```rust
#[test]
fn wide_title_shows_subtitle_and_path() {
    let text = render(&demo_app(), 80, 12);
    assert!(text.contains("dum — disk usage monitor — /r"));
}

#[test]
fn narrow_title_drops_subtitle_before_path() {
    let text = render(&demo_app(), 40, 12);
    assert!(!text.contains("disk usage monitor"));
    assert!(text.contains("dum — /r"));
}
```

And update the existing `empty_dir_and_help_overlay` test — its assertion changes because the help header is being renamed:

```rust
#[test]
fn empty_dir_and_help_overlay() {
    let mut app = demo_app();
    app.show_help = true;
    let text = render(&app, 80, 14);
    assert!(text.contains("dum — disk usage monitor"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --bin dum title; cargo test --bin dum help_overlay`
Expected: all three FAIL — title has no subtitle yet, help header still says "dum — keybindings"

- [ ] **Step 3: Implement the title**

In `src/bin/dum/ui.rs`, replace the title-rendering tail of `draw_title` (lines 165-172). The subtitle is the least important element, so it is dropped before the path gets clipped:

```rust
    let path = app.tree.path_of(app.current);
    let full = format!(" dum — disk usage monitor — {}", path.display());
    // Drop the subtitle before letting the path get clipped.
    let title = if full.chars().count() <= l.width as usize {
        full
    } else {
        format!(" dum — {}", path.display())
    };
    frame.render_widget(Paragraph::new(Line::from(Span::from(title).bold())), l);
    frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), r);
```

- [ ] **Step 4: Implement the help header**

In `src/bin/dum/ui.rs:273`, change the first help line:

```rust
        Line::from("dum — disk usage monitor"),
```

(replacing `Line::from("dum — keybindings"),`)

- [ ] **Step 5: Update the clap `about`**

In `src/bin/dum/main.rs:22`:

```rust
#[command(name = "dum", about = "dum — disk usage monitor, live")]
```

- [ ] **Step 6: Run the full test suite**

Run: `cargo test`
Expected: all pass, including the two new title tests and the updated help test. `full_width_render_shows_headers_and_entries` only asserts `contains("dum")` — still fine.

- [ ] **Step 7: Commit**

```bash
git add src/bin/dum/ui.rs src/bin/dum/main.rs
git commit -m "Show \"disk usage monitor\" in dum's title and help"
```

---

### Task 5: README

**Files:**
- Modify: `README.md:1-10` (intro bullets)

- [ ] **Step 1: Update the intro**

Replace lines 3-10 of `README.md`:

```markdown
Two terminal disk tools that go together:

- **ful** — a live dashboard of your mounted filesystems: usage bars,
  used/free/total, and per-device I/O. *Which disk is in trouble?*
  Not an acronym — a suffix, as in watch**ful** (the title bar shifts to
  plenti*ful*, watch*ful*, stress*ful*, or dread*ful* with your worst disk).
- **dum** — short for **d**isk **u**sage **m**onitor: an ncdu-style tree
  explorer that watches filesystem events and illuminates what's changing:
  directories receiving writes glow green with a live rate and sparkline,
  shrinking ones glow red, sizes update in place. *What exactly is moving?*
```

- [ ] **Step 2: Verify nothing else references the old subtitle**

Run: `grep -rn "disk usage monitor" src README.md Cargo.toml`
Expected: hits only in dum's UI/main, the README dum bullet, and `Cargo.toml`'s package description (which already reads "TUI disk usage monitor (ful) and live disk usage explorer (dum)" — swap it to `disk dashboard (ful) and disk usage monitor (dum)`).

- [ ] **Step 3: Final full check**

Run: `cargo test`
Expected: all pass

- [ ] **Step 4: Commit**

```bash
git add README.md Cargo.toml
git commit -m "Explain both names in the README"
```
