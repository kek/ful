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
    /// Space used by *this volume alone*, matching `df`'s per-volume "Used".
    /// On a shared APFS container this is far smaller than `total - available`,
    /// which would count every sibling volume in the container too.
    pub used: u64,
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
                let mount = d.mount_point();
                let total = d.total_space();
                let available = available_space(mount, d.available_space());
                DiskSample {
                    mount: mount.to_string_lossy().into_owned(),
                    device: short_device(&d.name().to_string_lossy()),
                    fs_type: d.file_system().to_string_lossy().into_owned(),
                    total,
                    available,
                    used: used_space(mount, total.saturating_sub(available)),
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

/// Space used by the volume mounted at `mount`, matching `df`'s per-volume
/// "Used" column, falling back to `fallback` (usually `total - available`) if
/// the platform query fails.
///
/// The naive `total - available` overcounts on shared containers: on macOS an
/// APFS *container* holds several volumes that pool their free space, so
/// `total` is the whole container and `total - available` charges this volume
/// for space its siblings occupy (e.g. reading ~93% for a volume `df` shows at
/// 15%). macOS exposes the volume's own footprint via `getattrlist`'s
/// `ATTR_VOL_SPACEUSED`; other Unixes have one filesystem per device, so the
/// volume's `statvfs` used-block count (`f_blocks - f_bfree`) is already
/// per-volume and matches `df` directly.
fn used_space(mount: &Path, fallback: u64) -> u64 {
    #[cfg(target_os = "macos")]
    {
        if let Some(used) = volume_space_used(mount) {
            return used;
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(used) = statvfs_used(mount) {
            return used;
        }
    }
    let _ = mount;
    fallback
}

/// Used bytes from `statvfs` fields the `df` way: total blocks minus free
/// blocks, times the fundamental block size. Pure so the arithmetic is testable
/// (`f_frsize` is the block-count unit; fall back to `f_bsize` when a platform
/// reports it as 0). Saturating, since the product can exceed `u64` in theory.
#[cfg(unix)]
#[cfg_attr(target_os = "macos", allow(dead_code))] // used only off macOS; tested everywhere
fn used_bytes_from_statvfs(frsize: u64, bsize: u64, blocks: u64, bfree: u64) -> u64 {
    let unit = if frsize != 0 { frsize } else { bsize };
    blocks.saturating_sub(bfree).saturating_mul(unit)
}

/// Query `statvfs(2)` for `mount` and return the volume's used bytes, or `None`
/// if the syscall fails. Only meaningful where each filesystem owns its device;
/// on shared-container filesystems (macOS APFS) these fields are container-wide,
/// which is why macOS uses `volume_space_used` instead.
#[cfg(all(unix, not(target_os = "macos")))]
fn statvfs_used(mount: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let cpath = CString::new(mount.as_os_str().as_bytes()).ok()?;
    // SAFETY: `cpath` is a valid NUL-terminated C string that outlives the call;
    // `statvfs` fills the zeroed struct and returns 0 on success, negative on error.
    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(cpath.as_ptr(), &mut stat) == 0 {
            Some(used_bytes_from_statvfs(
                stat.f_frsize as u64,
                stat.f_bsize as u64,
                stat.f_blocks as u64,
                stat.f_bfree as u64,
            ))
        } else {
            None
        }
    }
}

/// Bytes used by *this* APFS volume, via `getattrlist`'s `ATTR_VOL_SPACEUSED`.
///
/// This is the per-volume figure `df` prints as "Used" — unlike `statvfs`'s
/// `f_blocks`/`f_bfree`, which report the shared container and so read the same
/// (whole-container) usage for every volume in it. Returns `None` if the
/// syscall fails or the path can't be represented as a C string.
#[cfg(target_os = "macos")]
fn volume_space_used(mount: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let cpath = CString::new(mount.as_os_str().as_bytes()).ok()?;

    let mut list: libc::attrlist = unsafe { std::mem::zeroed() };
    list.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    list.volattr = libc::ATTR_VOL_INFO | libc::ATTR_VOL_SPACEUSED;

    // Reply layout for a single off_t attribute: a leading u32 length followed
    // by the 8-byte value, packed on a 4-byte boundary (hence the byte read
    // rather than a typed struct deref, which would assume 8-byte alignment).
    let mut buf = [0u8; 16];

    // SAFETY: `cpath` outlives the call; `list` and `buf` are valid, sized
    // exactly as told to `getattrlist`, which returns 0 on success.
    let rc = unsafe {
        libc::getattrlist(
            cpath.as_ptr(),
            &mut list as *mut libc::attrlist as *mut libc::c_void,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len() as libc::size_t,
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    let used = i64::from_ne_bytes(buf[4..12].try_into().ok()?);
    (used >= 0).then_some(used as u64)
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
            used: 50,
            read_bytes: 0,
            written_bytes: 0,
        }
    }

    #[cfg(unix)]
    #[test]
    fn used_bytes_from_statvfs_is_used_blocks_times_frsize() {
        // df-style: (total blocks - free blocks) * fundamental block size.
        // e.g. a 1 GiB volume with 256 MiB free at 4 KiB blocks -> 768 MiB used.
        let frsize = 4096;
        let blocks = 262_144; // 1 GiB / 4 KiB
        let bfree = 65_536; // 256 MiB / 4 KiB
        assert_eq!(
            used_bytes_from_statvfs(frsize, 1_048_576, blocks, bfree),
            805_306_368 // 768 MiB
        );
    }

    #[cfg(unix)]
    #[test]
    fn used_bytes_from_statvfs_falls_back_to_bsize_when_frsize_zero() {
        assert_eq!(used_bytes_from_statvfs(0, 4096, 10, 3), 28_672); // 7 * 4096
    }

    #[cfg(unix)]
    #[test]
    fn used_bytes_from_statvfs_saturates_instead_of_overflowing() {
        assert_eq!(used_bytes_from_statvfs(u64::MAX, 0, u64::MAX, 0), u64::MAX);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn volume_space_used_reports_positive_space_for_root() {
        // The root volume always exists and holds the OS; this guards the
        // getattrlist FFI plumbing (attrlist setup, buffer decode, return check).
        let used = volume_space_used(std::path::Path::new("/"));
        assert!(matches!(used, Some(n) if n > 0), "got {used:?}");
    }
}
