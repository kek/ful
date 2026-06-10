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
