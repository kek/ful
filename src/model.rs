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
        MountRow {
            mount: s.mount.clone(),
            device: s.device.clone(),
            fs_type: s.fs_type.clone(),
            used: s.used,
            free: s.available,
            total: s.total,
            used_pct: df_usage_percent(s.used, s.available),
            read_per_s,
            write_per_s,
        }
    }
}

/// Usage percent computed `df`'s way: `used / (used + available)`, rounded up.
///
/// `df` bases capacity on the space visible to the volume — used plus the
/// caller-available bytes — not the raw `total`, so it excludes the
/// root-reserved margin (and, on a shared APFS container, the space pooled by
/// sibling volumes). It rounds *up*, so a volume that is 14.4% full prints 15%;
/// we return that integer as an `f64` so the `{:.0}` display matches `df`
/// exactly. A zero denominator (no used, no available) yields 0%.
pub fn df_usage_percent(used: u64, available: u64) -> f64 {
    let denom = used.saturating_add(available);
    if denom == 0 {
        return 0.0;
    }
    (used as f64 / denom as f64 * 100.0).ceil()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(fs: &str, total: u64, available: u64) -> DiskSample {
        // Single-volume default: used is whatever the volume itself occupies.
        sample_used(fs, total, available, total.saturating_sub(available))
    }

    fn sample_used(fs: &str, total: u64, available: u64, used: u64) -> DiskSample {
        DiskSample {
            mount: "/".to_string(),
            device: "disk1".to_string(),
            fs_type: fs.to_string(),
            total,
            available,
            used,
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

    #[test]
    fn from_sample_uses_per_volume_used_not_container_total() {
        // Real shared-APFS root numbers: `total` is the whole ~926 GiB container,
        // but this volume itself uses only ~12 GiB. `df` shows Used 12 GiB, 15%;
        // the old `total - available` would have charged it ~93%.
        let s = sample_used("apfs", 994_662_584_320, 74_685_190_144, 12_572_438_528);
        let row = MountRow::from_sample(&s, None, None);
        assert_eq!(row.used, 12_572_438_528); // df's per-volume Used, not 920 GiB
        assert_eq!(row.free, 74_685_190_144);
        assert_eq!(row.total, 994_662_584_320); // container total left intact
        assert_eq!(row.used_pct, 15.0); // df Capacity, ceil(14.4%)
    }

    #[test]
    fn df_usage_percent_rounds_up_like_df() {
        // ceil(14.407%) = 15, matching df's per-volume root capacity.
        assert_eq!(df_usage_percent(12_572_438_528, 74_685_190_144), 15.0);
        // A volume that is exactly full reads 100%.
        assert_eq!(df_usage_percent(100, 0), 100.0);
        // The tiniest sliver of usage still rounds up to 1%, as df does.
        assert_eq!(df_usage_percent(1, 1_000_000), 1.0);
        // Empty (no used, no available) is 0%, not a division by zero.
        assert_eq!(df_usage_percent(0, 0), 0.0);
    }
}
