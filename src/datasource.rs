//! Raw disk readings and the source that produces them.

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
                    available: d.available_space(),
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
