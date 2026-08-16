//! dum application state: applies scan/delta messages, handles navigation.

use std::cmp::Ordering;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};

use crate::activity::ActivityMap;
use crate::scanner::ScanMsg;
use crate::tree::{NodeId, Tree};
use crate::watcher::{DeltaKind, DeltaMsg};

/// How `sorted_children` orders entries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortMode {
    /// Largest first (ties by name) — the default.
    Size,
    /// Most live change first, by |bytes/sec|; idle entries fall back to size.
    Rate,
}

impl SortMode {
    fn toggled(self) -> Self {
        match self {
            SortMode::Size => SortMode::Rate,
            SortMode::Rate => SortMode::Size,
        }
    }
}

/// Snapshot of the entry `d` was pressed on. A snapshot (not a NodeId) so a
/// watcher removal between arming and confirming can't redirect the delete.
pub struct DeleteTarget {
    pub path: PathBuf,
    pub name: OsString,
    pub size: u64,
    pub is_dir: bool,
}

impl DeleteTarget {
    /// Permanently remove the target from disk (recursively for directories).
    pub fn execute(&self) -> io::Result<()> {
        if self.is_dir {
            std::fs::remove_dir_all(&self.path)
        } else {
            std::fs::remove_file(&self.path)
        }
    }
}

pub struct App {
    pub tree: Tree,
    pub activity: ActivityMap,
    pub root_path: PathBuf,
    pub current: NodeId,
    pub selected: usize,
    pub sort: SortMode,
    pub scanning: bool,
    pub watching: bool,
    pub watch_degraded: bool,
    pub show_help: bool,
    pub should_quit: bool,
    pub rescan_requested: bool,
    /// Set by `d`: the entry awaiting y/N confirmation in the modal.
    pub pending_delete: Option<DeleteTarget>,
    /// Set by `y`: a confirmed delete for the main loop to perform.
    pub delete_confirmed: Option<DeleteTarget>,
    pub last_error: Option<String>,
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
            sort: SortMode::Size,
            scanning: true,
            watching: true,
            watch_degraded: false,
            show_help: false,
            should_quit: false,
            rescan_requested: false,
            pending_delete: None,
            delete_confirmed: None,
            last_error: None,
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
                    if self.tree.lookup(&child_path).is_some() {
                        continue; // watcher inserted it mid-scan; don't double count
                    }
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
                if msg.is_dir {
                    return; // dir totals come from children; a dir event itself carries no size
                }
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

    /// Perform a `y`-confirmed delete: remove from disk, then drop the subtree
    /// from the tree (so it works under --no-watch too; the watcher's own
    /// Removed event for the same path is a harmless no-op). Failures land in
    /// `last_error` and leave the tree untouched.
    pub fn process_confirmed_delete(&mut self, now: Instant) {
        let Some(target) = self.delete_confirmed.take() else { return };
        match target.execute() {
            Ok(()) => {
                self.last_error = None;
                self.apply_delta(
                    DeltaMsg {
                        path: target.path,
                        kind: DeltaKind::Removed,
                        new_size: None,
                        is_dir: target.is_dir,
                    },
                    now,
                );
            }
            Err(e) => {
                self.last_error = Some(format!("delete {}: {}", target.path.display(), e));
            }
        }
    }

    fn record_chain(&mut self, id: NodeId, delta: i64, now: Instant) {
        for anc in self.tree.ancestors_inclusive(id) {
            self.activity.record(anc, delta, now);
        }
    }

    /// Children of `dir` in the current sort order.
    pub fn sorted_children(&self, dir: NodeId, now: Instant) -> Vec<NodeId> {
        let mut kids = self.tree.get(dir).children.clone();
        match self.sort {
            SortMode::Size => kids.sort_by(|&a, &b| self.cmp_size(a, b)),
            SortMode::Rate => kids.sort_by(|&a, &b| {
                let mag = |id| self.activity.glow(id, now).abs();
                // Most live change first; idle ties fall back to size order.
                mag(b).total_cmp(&mag(a)).then_with(|| self.cmp_size(a, b))
            }),
        }
        kids
    }

    /// Size descending, ties by name ascending.
    fn cmp_size(&self, a: NodeId, b: NodeId) -> Ordering {
        let (na, nb) = (self.tree.get(a), self.tree.get(b));
        nb.size.cmp(&na.size).then_with(|| na.name.cmp(&nb.name))
    }

    fn clamp_selection(&mut self) {
        let n = self.tree.get(self.current).children.len();
        if n == 0 {
            self.selected = 0;
        } else if self.selected >= n {
            self.selected = n - 1;
        }
    }

    pub fn on_key(&mut self, key: KeyEvent, now: Instant) {
        // The confirm modal swallows every key: y confirms, anything else cancels.
        if let Some(target) = self.pending_delete.take() {
            if key.code == KeyCode::Char('y') {
                self.delete_confirmed = Some(target);
            }
            return;
        }
        match key.code {
            KeyCode::Esc if self.show_help => self.show_help = false,
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = !self.show_help,
            KeyCode::Char('r') => self.rescan_requested = true,
            KeyCode::Char('d') => {
                let kids = self.sorted_children(self.current, now);
                if let Some(&id) = kids.get(self.selected) {
                    let node = self.tree.get(id);
                    self.pending_delete = Some(DeleteTarget {
                        path: self.tree.path_of(id),
                        name: node.name.clone(),
                        size: node.size,
                        is_dir: node.is_dir,
                    });
                }
            }
            KeyCode::Char('s') => {
                // Keep the cursor on the same entry across the reorder.
                let anchor = self.sorted_children(self.current, now).get(self.selected).copied();
                self.sort = self.sort.toggled();
                if let Some(id) = anchor {
                    let kids = self.sorted_children(self.current, now);
                    if let Some(pos) = kids.iter().position(|&k| k == id) {
                        self.selected = pos;
                    }
                }
            }
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
                let kids = self.sorted_children(self.current, now);
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

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use crate::scanner::ScanEntry;

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
        let kids = app.sorted_children(app.tree.root, Instant::now());
        assert_eq!(app.tree.get(kids[0]).name, "a"); // 100
        assert_eq!(app.tree.get(kids[1]).name, "g"); // 50
    }

    #[test]
    fn sort_defaults_to_size() {
        assert_eq!(scanned_app().sort, SortMode::Size);
    }

    #[test]
    fn s_key_toggles_sort_mode() {
        let mut app = scanned_app();
        let now = Instant::now();
        app.on_key(KeyEvent::from(KeyCode::Char('s')), now);
        assert_eq!(app.sort, SortMode::Rate);
        app.on_key(KeyEvent::from(KeyCode::Char('s')), now);
        assert_eq!(app.sort, SortMode::Size);
    }

    #[test]
    fn rate_sort_orders_by_activity_magnitude() {
        let mut app = scanned_app();
        let now = Instant::now();
        // "g" (smaller) is changing fast; "a" (bigger) is idle.
        let g = app.tree.lookup(Path::new("/r/g")).unwrap();
        app.activity.record(g, 5_000_000, now);
        app.sort = SortMode::Rate;
        let kids = app.sorted_children(app.tree.root, now);
        assert_eq!(app.tree.get(kids[0]).name, "g"); // most active first
        assert_eq!(app.tree.get(kids[1]).name, "a");
    }

    #[test]
    fn rate_sort_ranks_shrinking_as_high_as_growing() {
        let mut app = scanned_app();
        let now = Instant::now();
        // "g" shrinking hard, "a" growing gently => |rate| puts "g" on top.
        let g = app.tree.lookup(Path::new("/r/g")).unwrap();
        let a = app.tree.lookup(Path::new("/r/a")).unwrap();
        app.activity.record(g, -9_000_000, now);
        app.activity.record(a, 100_000, now);
        app.sort = SortMode::Rate;
        let kids = app.sorted_children(app.tree.root, now);
        assert_eq!(app.tree.get(kids[0]).name, "g");
    }

    #[test]
    fn rate_sort_falls_back_to_size_when_idle() {
        let mut app = scanned_app();
        app.sort = SortMode::Rate;
        // No activity recorded: order matches size sort.
        let kids = app.sorted_children(app.tree.root, Instant::now());
        assert_eq!(app.tree.get(kids[0]).name, "a"); // 100
        assert_eq!(app.tree.get(kids[1]).name, "g"); // 50
    }

    #[test]
    fn toggling_sort_keeps_the_selected_node_under_the_cursor() {
        let mut app = scanned_app();
        let now = Instant::now();
        // Size order is [a, g]; select "g" at index 1.
        app.selected = 1;
        let g = app.tree.lookup(Path::new("/r/g")).unwrap();
        // Make "g" the most active so rate order becomes [g, a].
        app.activity.record(g, 5_000_000, now);
        app.on_key(KeyEvent::from(KeyCode::Char('s')), now);
        // Cursor should follow "g" to its new index 0, not stay at 1 (= "a").
        let kids = app.sorted_children(app.current, now);
        assert_eq!(kids[app.selected], g);
    }

    #[test]
    fn navigation_descend_and_up() {
        let mut app = scanned_app();
        let now = Instant::now();
        app.on_key(KeyEvent::from(KeyCode::Enter), now); // into "a" (largest, selected=0)
        let a = app.tree.lookup(Path::new("/r/a")).unwrap();
        assert_eq!(app.current, a);
        app.on_key(KeyEvent::from(KeyCode::Char('h')), now); // back up
        assert_eq!(app.current, app.tree.root);
    }

    #[test]
    fn quit_help_and_rescan_keys() {
        let mut app = scanned_app();
        let now = Instant::now();
        app.on_key(KeyEvent::from(KeyCode::Char('?')), now);
        assert!(app.show_help);
        app.on_key(KeyEvent::from(KeyCode::Esc), now); // closes help first
        assert!(!app.show_help);
        assert!(!app.should_quit);
        app.on_key(KeyEvent::from(KeyCode::Char('r')), now);
        assert!(app.rescan_requested);
        app.on_key(KeyEvent::from(KeyCode::Char('q')), now);
        assert!(app.should_quit);
    }

    #[test]
    fn d_key_arms_delete_confirmation_for_selected_entry() {
        let mut app = scanned_app();
        // Size sort puts "a" (100) first; it's selected.
        app.on_key(KeyEvent::from(KeyCode::Char('d')), Instant::now());
        let t = app.pending_delete.as_ref().expect("d should arm a pending delete");
        assert_eq!(t.path, PathBuf::from("/r/a"));
        assert_eq!(t.name, OsString::from("a"));
        assert_eq!(t.size, 100);
        assert!(t.is_dir);
    }

    #[test]
    fn d_key_with_nothing_selected_is_noop() {
        let mut app = App::new(Path::new("/r"), Instant::now());
        app.on_key(KeyEvent::from(KeyCode::Char('d')), Instant::now());
        assert!(app.pending_delete.is_none());
    }

    #[test]
    fn y_turns_pending_delete_into_confirmed_request() {
        let mut app = scanned_app();
        let now = Instant::now();
        app.on_key(KeyEvent::from(KeyCode::Char('d')), now);
        app.on_key(KeyEvent::from(KeyCode::Char('y')), now);
        assert!(app.pending_delete.is_none());
        let t = app.delete_confirmed.as_ref().expect("y should confirm the delete");
        assert_eq!(t.path, PathBuf::from("/r/a"));
    }

    #[test]
    fn any_other_key_cancels_pending_delete() {
        let mut app = scanned_app();
        let now = Instant::now();
        app.on_key(KeyEvent::from(KeyCode::Char('d')), now);
        // 'q' would quit outside the modal; here it must only cancel.
        app.on_key(KeyEvent::from(KeyCode::Char('q')), now);
        assert!(app.pending_delete.is_none());
        assert!(app.delete_confirmed.is_none());
        assert!(!app.should_quit);
    }

    #[test]
    fn navigation_is_inert_while_delete_pending() {
        let mut app = scanned_app();
        let now = Instant::now();
        app.on_key(KeyEvent::from(KeyCode::Char('d')), now);
        app.on_key(KeyEvent::from(KeyCode::Char('j')), now); // cancels, must not move
        assert_eq!(app.selected, 0);
        assert!(app.pending_delete.is_none());
    }

    /// App over a real tempdir containing one file, scanned into the tree.
    fn app_on_disk() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("victim.txt"), vec![0u8; 100]).unwrap();
        let mut app = App::new(dir.path(), Instant::now());
        app.apply_scan(ScanMsg::Dir {
            path: dir.path().to_path_buf(),
            entries: vec![entry("victim.txt", 100, false)],
            denied: false,
        });
        app.apply_scan(ScanMsg::Done { dirs: 0, files: 1, errors: 0 });
        (dir, app)
    }

    #[test]
    fn confirmed_delete_removes_from_disk_and_tree() {
        let (dir, mut app) = app_on_disk();
        let now = Instant::now();
        app.on_key(KeyEvent::from(KeyCode::Char('d')), now);
        app.on_key(KeyEvent::from(KeyCode::Char('y')), now);
        app.process_confirmed_delete(now);
        assert!(!dir.path().join("victim.txt").exists());
        assert!(app.tree.lookup(&dir.path().join("victim.txt")).is_none());
        assert_eq!(app.tree.get(app.tree.root).size, 0);
        assert!(app.delete_confirmed.is_none());
        assert!(app.last_error.is_none());
    }

    #[test]
    fn failed_delete_sets_error_and_keeps_tree() {
        let (dir, mut app) = app_on_disk();
        let now = Instant::now();
        std::fs::remove_file(dir.path().join("victim.txt")).unwrap(); // vanish underneath
        app.on_key(KeyEvent::from(KeyCode::Char('d')), now);
        app.on_key(KeyEvent::from(KeyCode::Char('y')), now);
        app.process_confirmed_delete(now);
        assert!(app.last_error.is_some());
        assert!(app.tree.lookup(&dir.path().join("victim.txt")).is_some());
        assert_eq!(app.tree.get(app.tree.root).size, 100);
    }

    #[test]
    fn process_without_confirmation_is_noop() {
        let (dir, mut app) = app_on_disk();
        app.process_confirmed_delete(Instant::now());
        assert!(dir.path().join("victim.txt").exists());
        assert!(app.last_error.is_none());
    }

    #[test]
    fn execute_removes_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("victim.txt");
        std::fs::write(&f, b"bytes").unwrap();
        let t = DeleteTarget {
            path: f.clone(),
            name: OsString::from("victim.txt"),
            size: 5,
            is_dir: false,
        };
        t.execute().unwrap();
        assert!(!f.exists());
    }

    #[test]
    fn execute_removes_a_directory_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("inner.txt"), b"x").unwrap();
        let t = DeleteTarget {
            path: sub.clone(),
            name: OsString::from("sub"),
            size: 1,
            is_dir: true,
        };
        t.execute().unwrap();
        assert!(!sub.exists());
    }

    #[test]
    fn execute_on_missing_path_reports_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let t = DeleteTarget {
            path: dir.path().join("gone"),
            name: OsString::from("gone"),
            size: 0,
            is_dir: false,
        };
        assert!(t.execute().is_err());
    }

    #[test]
    fn scan_does_not_reinsert_path_already_added_by_watcher() {
        let mut app = App::new(Path::new("/r"), Instant::now());
        app.apply_scan(ScanMsg::Dir {
            path: PathBuf::from("/r"),
            entries: vec![entry("a", 0, true)],
            denied: false,
        });
        // Watcher observes a file created mid-scan and inserts it first.
        app.apply_delta(changed("/r/a/new.bin", 500), Instant::now());
        assert_eq!(app.tree.get(app.tree.root).size, 500);
        // Scanner's listing of /r/a arrives later, still containing new.bin.
        app.apply_scan(ScanMsg::Dir {
            path: PathBuf::from("/r/a"),
            entries: vec![entry("new.bin", 500, false)],
            denied: false,
        });
        // No double count, no duplicate child.
        assert_eq!(app.tree.get(app.tree.root).size, 500);
        let a = app.tree.lookup(Path::new("/r/a")).unwrap();
        assert_eq!(app.tree.get(a).children.len(), 1);
    }

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
}
