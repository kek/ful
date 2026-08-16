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
    fn dir_rename_reports_removed_old_and_changed_new() {
        // The rename contract the app's subtree-rescan relies on: the OS emits
        // no events for the children, only the two endpoints of the rename.
        let td = tempfile::tempdir().unwrap();
        let root = td.path().canonicalize().unwrap();
        fs::create_dir(root.join("old")).unwrap();
        fs::write(root.join("old/data.bin"), vec![0u8; 150_000]).unwrap();
        let (tx, rx) = mpsc::channel();
        {
            let root = root.clone();
            std::thread::spawn(move || watch(root, tx));
        }
        std::thread::sleep(Duration::from_millis(500)); // FSEvents warmup

        fs::rename(root.join("old"), root.join("new")).unwrap();

        let new = root.join("new");
        let d = wait_for(&rx, 10, |d| {
            d.path == new && matches!(d.kind, DeltaKind::Changed)
        })
        .expect("expected Changed for rename target");
        assert!(d.is_dir);
        let old = root.join("old");
        wait_for(&rx, 10, |d| {
            d.path == old && matches!(d.kind, DeltaKind::Removed)
        })
        .expect("expected Removed for rename source");
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
