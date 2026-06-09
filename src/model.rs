//! Display-ready mount row and real-vs-pseudo filesystem classification.

use crate::datasource::DiskSample;

const PSEUDO_FS: [&str; 8] = [
    "devfs", "autofs", "tmpfs", "proc", "sysfs", "overlay", "squashfs", "nullfs",
];

/// A filesystem is "pseudo" (hidden by default) if it has no capacity or its
/// type is a known virtual filesystem.
pub fn is_pseudo_fs(fs_type: &str, total: u64) -> bool {
    if total == 0 {
        return true;
    }
    let fs = fs_type.to_ascii_lowercase();
    PSEUDO_FS.contains(&fs.as_str())
}

/// Display-ready row: derived fields plus optional per-second I/O rates.
#[derive(Debug, Clone, PartialEq)]
pub struct MountRow {
    pub mount: String,
    pub device: String,
    pub fs_type: String,
    pub used: u64,
    pub free: u64,
    pub total: u64,
    pub used_pct: f64,
    pub read_per_s: Option<f64>,
    pub write_per_s: Option<f64>,
}

impl MountRow {
    pub fn from_sample(s: &DiskSample, read_per_s: Option<f64>, write_per_s: Option<f64>) -> Self {
        let used = s.total.saturating_sub(s.available);
        let used_pct = if s.total == 0 {
            0.0
        } else {
            used as f64 / s.total as f64 * 100.0
        };
        MountRow {
            mount: s.mount.clone(),
            device: s.device.clone(),
            fs_type: s.fs_type.clone(),
            used,
            free: s.available,
            total: s.total,
            used_pct,
            read_per_s,
            write_per_s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(fs: &str, total: u64, available: u64) -> DiskSample {
        DiskSample {
            mount: "/".to_string(),
            device: "disk1".to_string(),
            fs_type: fs.to_string(),
            total,
            available,
            read_bytes: 0,
            written_bytes: 0,
        }
    }

    #[test]
    fn pseudo_when_zero_total_or_known_pseudo_fs() {
        assert!(is_pseudo_fs("apfs", 0)); // zero total -> pseudo
        assert!(is_pseudo_fs("devfs", 100));
        assert!(is_pseudo_fs("TMPFS", 100)); // case-insensitive
    }

    #[test]
    fn real_fs_with_capacity_is_not_pseudo() {
        assert!(!is_pseudo_fs("apfs", 100));
        assert!(!is_pseudo_fs("exfat", 100));
    }

    #[test]
    fn from_sample_derives_used_and_pct() {
        let row = MountRow::from_sample(&sample("apfs", 1000, 250), Some(5.0), None);
        assert_eq!(row.used, 750);
        assert_eq!(row.free, 250);
        assert_eq!(row.total, 1000);
        assert!((row.used_pct - 75.0).abs() < 1e-9);
        assert_eq!(row.read_per_s, Some(5.0));
        assert_eq!(row.write_per_s, None);
    }

    #[test]
    fn from_sample_handles_zero_total() {
        let row = MountRow::from_sample(&sample("devfs", 0, 0), None, None);
        assert_eq!(row.used, 0);
        assert_eq!(row.used_pct, 0.0);
    }
}
