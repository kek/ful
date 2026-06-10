//! dum application state: applies scan/delta messages, handles navigation.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};

use crate::activity::ActivityMap;
use crate::scanner::ScanMsg;
use crate::tree::{NodeId, Tree};
use crate::watcher::{DeltaKind, DeltaMsg};

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
