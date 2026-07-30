//! Initial filesystem walk: streams directory listings to the UI thread.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::Metadata;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

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

/// Allocated size of a file, charging a multiply-linked inode only the first
/// time it's seen in this scan — matching `du`, which counts each inode once.
/// `seen` holds the `(dev, ino)` of hard-linked files already counted.
#[cfg(unix)]
fn counted_size(md: &Metadata, seen: &mut HashSet<(u64, u64)>) -> u64 {
    use std::os::unix::fs::MetadataExt;
    // Only nlink > 1 can appear twice, so single-linked files skip the set.
    if md.nlink() > 1 && !seen.insert((md.dev(), md.ino())) {
        return 0; // an inode we've already charged this scan
    }
    allocated_size(md)
}

#[cfg(not(unix))]
fn counted_size(md: &Metadata, _seen: &mut HashSet<(u64, u64)>) -> u64 {
    allocated_size(md)
}

/// Walk `root` depth-first, streaming one `Dir` message per directory,
/// then `Done`. Symlinks are not followed. Send errors mean the UI is
/// gone — just stop.
pub fn scan(root: PathBuf, tx: Sender<ScanMsg>) {
    let (mut dirs, mut files, mut errors) = (0u64, 0u64, 0u64);
    // Hard-linked inodes already counted, so each is charged once per scan.
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
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
                size: if is_dir { 0 } else { counted_size(&md, &mut seen) },
                is_dir,
            });
        }
        if tx.send(ScanMsg::Dir { path: dir, entries, denied: false }).is_err() {
            return;
        }
    }
    let _ = tx.send(ScanMsg::Done { dirs, files, errors });
}

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

    /// Sum the recorded size of every file entry across all `Dir` messages —
    /// the same rollup the tree performs, so this is what the UI would report.
    fn total_file_bytes(msgs: &[ScanMsg]) -> u64 {
        msgs.iter()
            .filter_map(|m| match m {
                ScanMsg::Dir { entries, .. } => {
                    Some(entries.iter().map(|e| e.size).sum::<u64>())
                }
                _ => None,
            })
            .sum()
    }

    #[cfg(unix)]
    #[test]
    fn hard_link_counts_the_inode_once() {
        // A file and a hard link to it share one inode; `du` charges its blocks
        // once, so the scan total must equal a single copy — not double.
        let td = tempfile::tempdir().unwrap();
        let orig = td.path().join("orig.bin");
        fs::write(&orig, vec![0u8; 100_000]).unwrap();
        fs::hard_link(&orig, td.path().join("link.bin")).unwrap();

        let msgs = run_scan(td.path().to_path_buf());
        let one_copy = allocated_size(&fs::metadata(&orig).unwrap());
        assert_eq!(total_file_bytes(&msgs), one_copy);
    }

    #[cfg(unix)]
    #[test]
    fn distinct_files_of_equal_size_are_both_counted() {
        // Two independent inodes that happen to be the same size must both
        // count — the dedup keys on (dev, ino), never on size.
        let td = tempfile::tempdir().unwrap();
        fs::write(td.path().join("a.bin"), vec![0u8; 100_000]).unwrap();
        fs::write(td.path().join("b.bin"), vec![7u8; 100_000]).unwrap();

        let msgs = run_scan(td.path().to_path_buf());
        let one_copy = allocated_size(&fs::metadata(td.path().join("a.bin")).unwrap());
        assert_eq!(total_file_bytes(&msgs), one_copy * 2);
    }

    #[cfg(unix)]
    #[test]
    fn denied_directory_is_flagged_and_counted() {
        // Fails as root, which ignores mode 0o000 and reads the directory anyway:
        // nothing is denied and `errors` stays 0. That's the test being unrunnable
        // as root, not a scanner bug — CI's runners are non-root and pass. So if
        // this fails, check `id -u` first: a plain `docker run` is root by default
        // (`--user "$(id -u)"` fixes it).
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
