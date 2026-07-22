//! Raw disk readings and the source that produces them.

use std::path::Path;

/// One raw reading for a single mounted filesystem.
/// `read_bytes`/`written_bytes` are *cumulative* counters; rates are derived
/// by the `App` from the delta between two samples.
#[derive(Debug, Clone, PartialEq)]
pub struct DiskSample {
    pub mount: String,
    pub device: String, // short node, e.g. "disk3s1" (/dev/ stripped)
    pub fs_type: String,
    pub total: u64,
    pub available: u64,
    pub read_bytes: u64,
    pub written_bytes: u64,
}

/// Produces a snapshot of all mounted filesystems on each call.
pub trait DataSource {
    fn sample(&mut self) -> Vec<DiskSample>;
}

/// Live data source backed by the `sysinfo` crate.
pub struct SysinfoSource {
    disks: sysinfo::Disks,
}

impl SysinfoSource {
    pub fn new() -> Self {
        SysinfoSource {
            disks: sysinfo::Disks::new_with_refreshed_list(),
        }
    }
}

impl DataSource for SysinfoSource {
    fn sample(&mut self) -> Vec<DiskSample> {
        // Refresh existing disks (true = drop disks no longer present).
        self.disks.refresh(true);
        self.disks
            .list()
            .iter()
            .map(|d| {
                let usage = d.usage();
                DiskSample {
                    mount: d.mount_point().to_string_lossy().into_owned(),
                    device: short_device(&d.name().to_string_lossy()),
                    fs_type: d.file_system().to_string_lossy().into_owned(),
                    total: d.total_space(),
                    available: available_space(d.mount_point(), d.available_space()),
                    read_bytes: usage.total_read_bytes,
                    written_bytes: usage.total_written_bytes,
                }
            })
            .collect()
    }
}

/// Strip the directory portion of a device path: `/dev/disk3s1` -> `disk3s1`.
fn short_device(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_string()
}

/// Space available to an unprivileged caller, matching `df`'s "Available".
///
/// `sysinfo`'s `available_space()` disagrees with `df`: on macOS it reports
/// `AvailableCapacityForImportantUsage`, which folds in purgeable space (trash,
/// system caches, evictable snapshots) and so reads far higher than `df` — on a
/// ~926 GiB volume it reported ~83 GiB free where `df` showed ~46 GiB. `df`
/// instead reports the kernel's `f_bavail` (blocks free to a non-root process,
/// excluding the root-reserved margin). We query `statvfs` and reproduce that
/// figure, falling back to `sysinfo`'s value only if the syscall fails.
fn available_space(mount: &Path, sysinfo_available: u64) -> u64 {
    #[cfg(unix)]
    {
        statvfs_available(mount).unwrap_or(sysinfo_available)
    }
    #[cfg(not(unix))]
    {
        let _ = mount;
        sysinfo_available
    }
}

/// Available bytes from `statvfs` fields: available-to-caller blocks times the
/// fundamental block size, matching `df`. Pure so the arithmetic is testable
/// (`f_frsize` is the block-count unit; if a platform reports it as 0 we fall
/// back to `f_bsize`). Saturating, since the product can exceed `u64` in theory.
fn available_bytes(frsize: u64, bsize: u64, bavail: u64) -> u64 {
    let unit = if frsize != 0 { frsize } else { bsize };
    bavail.saturating_mul(unit)
}

/// Query `statvfs(2)` for `mount` and return the caller-available bytes, or
/// `None` if the syscall fails or the path can't be represented as a C string.
#[cfg(unix)]
fn statvfs_available(mount: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let cpath = CString::new(mount.as_os_str().as_bytes()).ok()?;
    // SAFETY: `cpath` is a valid NUL-terminated C string that outlives the call;
    // `statvfs` fills the zeroed struct and returns 0 on success, negative on error.
    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(cpath.as_ptr(), &mut stat) == 0 {
            Some(available_bytes(
                stat.f_frsize as u64,
                stat.f_bsize as u64,
                stat.f_bavail as u64,
            ))
        } else {
            None
        }
    }
}

/// Deterministic source for tests: returns each batch in order, then empty.
#[cfg(test)]
pub struct FakeSource {
    batches: Vec<Vec<DiskSample>>,
    idx: usize,
}

#[cfg(test)]
impl FakeSource {
    pub fn new(batches: Vec<Vec<DiskSample>>) -> Self {
        FakeSource { batches, idx: 0 }
    }
}

#[cfg(test)]
impl DataSource for FakeSource {
    fn sample(&mut self) -> Vec<DiskSample> {
        let b = self.batches.get(self.idx).cloned().unwrap_or_default();
        self.idx += 1;
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_device_strips_dev_prefix() {
        assert_eq!(short_device("/dev/disk3s1"), "disk3s1");
        assert_eq!(short_device("tmpfs"), "tmpfs");
    }

    #[test]
    fn available_bytes_uses_frsize_times_bavail() {
        // df-style: available blocks * fundamental block size.
        assert_eq!(available_bytes(4096, 1_048_576, 12_316_486), 50_448_326_656);
    }

    #[test]
    fn available_bytes_falls_back_to_bsize_when_frsize_zero() {
        assert_eq!(available_bytes(0, 4096, 10), 40_960);
    }

    #[test]
    fn available_bytes_saturates_instead_of_overflowing() {
        assert_eq!(available_bytes(u64::MAX, 0, u64::MAX), u64::MAX);
    }

    #[cfg(unix)]
    #[test]
    fn statvfs_available_reports_positive_space_for_root() {
        // The root filesystem always exists and has some free space; this guards
        // the FFI plumbing (path encoding, struct layout, return-code check).
        let avail = statvfs_available(std::path::Path::new("/"));
        assert!(matches!(avail, Some(n) if n > 0), "got {avail:?}");
    }

    #[test]
    fn fake_source_returns_batches_then_empty() {
        let mut s = FakeSource::new(vec![vec![sample("a")], vec![]]);
        assert_eq!(s.sample().len(), 1);
        assert_eq!(s.sample().len(), 0);
        assert_eq!(s.sample().len(), 0); // past the end -> empty
    }

    fn sample(dev: &str) -> DiskSample {
        DiskSample {
            mount: format!("/{dev}"),
            device: dev.to_string(),
            fs_type: "apfs".to_string(),
            total: 100,
            available: 50,
            read_bytes: 0,
            written_bytes: 0,
        }
    }
}
