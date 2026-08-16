//! Background deletion: walks a subtree deleting entry-by-entry so progress
//! can stream to the UI (std::fs::remove_dir_all offers no callbacks).

use std::path::PathBuf;
use std::sync::mpsc::Sender;

use crate::app::DeleteTarget;

pub enum DeleteMsg {
    /// Running count of filesystem objects removed so far.
    Progress { items: u64 },
    /// Everything under (and including) the target is gone. A final Progress
    /// with the exact total always precedes this.
    Done { target: DeleteTarget },
    /// First failure; the walk stops here. Already-deleted entries stay deleted.
    Failed { path: PathBuf, error: String },
}

const PROGRESS_EVERY: u64 = 256;

/// Delete the target, streaming progress. Runs until done or first error;
/// send failures mean the UI is gone — just stop.
pub fn delete(target: DeleteTarget, tx: Sender<DeleteMsg>) {
    let mut items = 0u64;
    match delete_path(&target.path, target.is_dir, &mut items, &tx) {
        Ok(()) => {
            let _ = tx.send(DeleteMsg::Progress { items });
            let _ = tx.send(DeleteMsg::Done { target });
        }
        Err((path, e)) => {
            let _ = tx.send(DeleteMsg::Failed { path, error: e.to_string() });
        }
    }
}

/// Depth-first walk: children first, then the entry itself. Symlinks are
/// removed as links, never followed. Errors carry the path that failed.
fn delete_path(
    path: &std::path::Path,
    is_dir: bool,
    items: &mut u64,
    tx: &Sender<DeleteMsg>,
) -> Result<(), (PathBuf, std::io::Error)> {
    if is_dir {
        let rd = std::fs::read_dir(path).map_err(|e| (path.to_path_buf(), e))?;
        for ent in rd {
            let ent = ent.map_err(|e| (path.to_path_buf(), e))?;
            let child_is_dir = ent
                .file_type()
                .map(|ft| ft.is_dir()) // symlink-to-dir reports false: removed as a link
                .map_err(|e| (ent.path(), e))?;
            delete_path(&ent.path(), child_is_dir, items, tx)?;
        }
        std::fs::remove_dir(path).map_err(|e| (path.to_path_buf(), e))?;
    } else {
        std::fs::remove_file(path).map_err(|e| (path.to_path_buf(), e))?;
    }
    *items += 1;
    if items.is_multiple_of(PROGRESS_EVERY) {
        let _ = tx.send(DeleteMsg::Progress { items: *items });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;
    use std::sync::mpsc;

    fn target(path: PathBuf, is_dir: bool) -> DeleteTarget {
        DeleteTarget { name: OsString::from(path.file_name().unwrap()), path, size: 0, is_dir }
    }

    fn run(t: DeleteTarget) -> Vec<DeleteMsg> {
        let (tx, rx) = mpsc::channel();
        delete(t, tx);
        rx.try_iter().collect()
    }

    #[test]
    fn deletes_a_directory_recursively_and_reports_done() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path().join("victim");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("f1"), b"x").unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/f2"), b"y").unwrap();

        let msgs = run(target(dir.clone(), true));
        assert!(!dir.exists());
        match msgs.last().expect("expected messages") {
            DeleteMsg::Done { target } => assert_eq!(target.path, dir),
            _ => panic!("last message must be Done"),
        }
        assert_eq!(final_progress(&msgs), 4); // f1, sub/f2, sub, victim
    }

    /// The items count of the last Progress message (always sent before Done).
    fn final_progress(msgs: &[DeleteMsg]) -> u64 {
        msgs.iter()
            .rev()
            .find_map(|m| match m {
                DeleteMsg::Progress { items } => Some(*items),
                _ => None,
            })
            .expect("a final Progress must precede Done")
    }

    #[test]
    fn deletes_a_single_file() {
        let td = tempfile::tempdir().unwrap();
        let f = td.path().join("lone.bin");
        fs::write(&f, b"z").unwrap();
        let msgs = run(target(f.clone(), false));
        assert!(!f.exists());
        assert!(matches!(msgs.last(), Some(DeleteMsg::Done { .. })));
        assert_eq!(final_progress(&msgs), 1);
    }

    #[test]
    fn streams_progress_before_done_on_large_trees() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path().join("many");
        fs::create_dir(&dir).unwrap();
        for i in 0..600 {
            fs::write(dir.join(format!("f{i:03}")), b".").unwrap();
        }
        let msgs = run(target(dir.clone(), true));
        assert!(!dir.exists());
        let progress: Vec<u64> = msgs
            .iter()
            .filter_map(|m| match m {
                DeleteMsg::Progress { items } => Some(*items),
                _ => None,
            })
            .collect();
        assert!(progress.len() > 1, "large delete must stream progress along the way");
        assert!(matches!(msgs.last(), Some(DeleteMsg::Done { .. })));
        assert_eq!(final_progress(&msgs), 601); // 600 files + the dir itself
        assert!(progress.iter().all(|&p| p <= 601));
    }

    #[test]
    fn missing_target_reports_failed() {
        let td = tempfile::tempdir().unwrap();
        let msgs = run(target(td.path().join("gone"), false));
        match msgs.last().expect("expected messages") {
            DeleteMsg::Failed { path, error } => {
                assert_eq!(*path, td.path().join("gone"));
                assert!(!error.is_empty());
            }
            _ => panic!("last message must be Failed"),
        }
    }
}
