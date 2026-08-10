use super::layout::ALIGNMENT;
use super::scenario::DiskScenario;
use super::{DiskBackend, DiskInfo, FormatFs, NtfsProbe, PartitionInfo, PartitionKind, TableKind};
use crate::mx;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

struct FakeState {
    disks: Vec<DiskInfo>,
    partitions: Vec<PartitionInfo>,
    ntfs: HashMap<String, NtfsProbe>,
    usage: HashMap<String, u64>,
}

/// In-memory disk backend for `--fake` and for tests. `new()` is the
/// `linux` scenario (one 250 GB disk, EFI + ext4 root, no Windows); pass
/// `--fake-disk=<scenario>` (see `scenario::DiskScenario`) to exercise the
/// others.
pub struct FakeDiskBackend {
    state: Mutex<FakeState>,
}

impl FakeDiskBackend {
    pub fn new() -> Self {
        Self::with_scenario(DiskScenario::Linux)
    }

    pub fn with_scenario(scenario: DiskScenario) -> Self {
        let fixture = scenario.fixture();
        Self {
            state: Mutex::new(FakeState {
                disks: fixture.disks,
                partitions: fixture.partitions,
                ntfs: fixture.ntfs,
                usage: fixture.usage,
            }),
        }
    }
}

impl Default for FakeDiskBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn next_partition_index(partitions: &[PartitionInfo], disk: &str) -> u32 {
    let prefix = format!("{disk}p");
    partitions
        .iter()
        .filter_map(|p| p.path.strip_prefix(&prefix)?.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1
}

fn overlaps(partitions: &[PartitionInfo], disk: &str, start: u64, size: u64) -> bool {
    let end = start + size;
    partitions
        .iter()
        .filter(|p| p.disk_path == disk)
        .any(|p| start < p.start_bytes + p.size_bytes && p.start_bytes < end)
}

#[async_trait]
impl DiskBackend for FakeDiskBackend {
    async fn list_disks(&self) -> mx::Result<Vec<DiskInfo>> {
        Ok(self.state.lock().unwrap().disks.clone())
    }

    async fn list_partitions(&self, disk: &str) -> mx::Result<Vec<PartitionInfo>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .partitions
            .iter()
            .filter(|p| p.disk_path == disk)
            .cloned()
            .collect())
    }

    async fn device_node(&self, path: &str) -> mx::Result<String> {
        Ok(path.to_string())
    }

    async fn resolve_device(&self, device_node: &str) -> mx::Result<String> {
        let mut state = self.state.lock().unwrap();
        if !state.partitions.iter().any(|p| p.path == device_node) {
            state.partitions.push(PartitionInfo {
                path: device_node.to_string(),
                disk_path: String::new(),
                fs_type: None,
                label: None,
                size_bytes: 0,
                start_bytes: 0,
                type_guid: None,
                uuid: None,
                is_esp: false,
                used_bytes: None,
            });
        }
        Ok(device_node.to_string())
    }

    async fn create_table(&self, disk: &str, table: TableKind) -> mx::Result<()> {
        let mut state = self.state.lock().unwrap();
        let d = state
            .disks
            .iter_mut()
            .find(|d| d.path == disk)
            .ok_or_else(|| mx::Error::Backend(format!("no such disk: {disk}")))?;
        d.table = table;
        state.partitions.retain(|p| p.disk_path != disk);
        Ok(())
    }

    async fn create_partition(
        &self,
        disk: &str,
        start_bytes: u64,
        size_bytes: u64,
        kind: PartitionKind,
    ) -> mx::Result<String> {
        let mut state = self.state.lock().unwrap();

        let disk_size = state
            .disks
            .iter()
            .find(|d| d.path == disk)
            .map(|d| d.size_bytes)
            .ok_or_else(|| mx::Error::Backend(format!("no such disk: {disk}")))?;

        if !start_bytes.is_multiple_of(ALIGNMENT) || !size_bytes.is_multiple_of(ALIGNMENT) {
            return Err(mx::Error::Backend(format!(
                "partition at {start_bytes}+{size_bytes} isn't 1 MiB-aligned"
            )));
        }
        if start_bytes + size_bytes > disk_size {
            return Err(mx::Error::Backend(format!(
                "partition at {start_bytes}+{size_bytes} exceeds disk size {disk_size}"
            )));
        }
        if overlaps(&state.partitions, disk, start_bytes, size_bytes) {
            return Err(mx::Error::Backend(format!(
                "partition at {start_bytes}+{size_bytes} overlaps an existing partition"
            )));
        }

        let index = next_partition_index(&state.partitions, disk);
        let path = format!("{disk}p{index}");
        state.partitions.push(PartitionInfo {
            path: path.clone(),
            disk_path: disk.to_string(),
            fs_type: None,
            label: None,
            size_bytes,
            start_bytes,
            type_guid: Some(kind.type_guid().to_string()),
            uuid: None,
            is_esp: matches!(kind, PartitionKind::Esp),
            used_bytes: None,
        });
        Ok(path)
    }

    async fn delete_partition(&self, partition: &str) -> mx::Result<()> {
        self.state
            .lock()
            .unwrap()
            .partitions
            .retain(|p| p.path != partition);
        Ok(())
    }

    async fn resize_partition(&self, partition: &str, new_size_bytes: u64) -> mx::Result<()> {
        let mut state = self.state.lock().unwrap();
        let (disk_path, start_bytes) = {
            let p = state
                .partitions
                .iter()
                .find(|p| p.path == partition)
                .ok_or_else(|| mx::Error::Backend(format!("no such partition: {partition}")))?;
            (p.disk_path.clone(), p.start_bytes)
        };
        let others: Vec<PartitionInfo> = state
            .partitions
            .iter()
            .filter(|p| p.path != partition)
            .cloned()
            .collect();
        if overlaps(&others, &disk_path, start_bytes, new_size_bytes) {
            return Err(mx::Error::Backend(format!(
                "resizing {partition} to {new_size_bytes} would overlap another partition"
            )));
        }
        let p = state
            .partitions
            .iter_mut()
            .find(|p| p.path == partition)
            .expect("looked up above");
        p.size_bytes = new_size_bytes;
        Ok(())
    }

    async fn format_partition(
        &self,
        partition: &str,
        fs: FormatFs,
        label: Option<&str>,
    ) -> mx::Result<()> {
        let mut state = self.state.lock().unwrap();
        let p = state
            .partitions
            .iter_mut()
            .find(|p| p.path == partition)
            .ok_or_else(|| mx::Error::Backend(format!("no such partition: {partition}")))?;
        p.fs_type = Some(fs.as_mkfs_type().to_string());
        p.label = label.map(str::to_string);
        Ok(())
    }

    async fn probe_ntfs(&self, partition: &str) -> mx::Result<NtfsProbe> {
        self.state
            .lock()
            .unwrap()
            .ntfs
            .get(partition)
            .cloned()
            .ok_or_else(|| mx::Error::Backend(format!("no NTFS probe fixture for {partition}")))
    }

    async fn probe_usage(&self, path: &str, _fs_type: Option<&str>) -> mx::Result<Option<u64>> {
        Ok(self.state.lock().unwrap().usage.get(path).copied())
    }

    async fn mount(&self, _partition: &str, _target: &str) -> mx::Result<()> {
        Ok(())
    }

    async fn unmount(&self, _partition: &str) -> mx::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_overlapping_partition() {
        // The `linux` default scenario has an ESP starting at 1 MiB —
        // requesting a new partition at that same offset must collide.
        let backend = FakeDiskBackend::new();
        let err = backend
            .create_partition(
                "/dev/fake0",
                ALIGNMENT,
                ALIGNMENT,
                PartitionKind::LinuxGeneric,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, mx::Error::Backend(_)));
    }

    #[tokio::test]
    async fn rejects_unaligned_partition() {
        let backend = FakeDiskBackend::with_scenario(DiskScenario::Empty);
        let err = backend
            .create_partition("/dev/fake0", 1, ALIGNMENT, PartitionKind::LinuxGeneric)
            .await
            .unwrap_err();
        assert!(matches!(err, mx::Error::Backend(_)));
    }

    #[tokio::test]
    async fn accepts_aligned_non_overlapping_partition() {
        let backend = FakeDiskBackend::with_scenario(DiskScenario::Empty);
        let path = backend
            .create_partition("/dev/fake0", 0, ALIGNMENT, PartitionKind::LinuxGeneric)
            .await
            .unwrap();
        assert_eq!(path, "/dev/fake0p1");
    }

    #[tokio::test]
    async fn naming_continues_after_delete() {
        let backend = FakeDiskBackend::with_scenario(DiskScenario::Empty);
        let p1 = backend
            .create_partition("/dev/fake0", 0, ALIGNMENT, PartitionKind::LinuxGeneric)
            .await
            .unwrap();
        let p2 = backend
            .create_partition(
                "/dev/fake0",
                ALIGNMENT,
                ALIGNMENT,
                PartitionKind::LinuxGeneric,
            )
            .await
            .unwrap();
        assert_eq!(p1, "/dev/fake0p1");
        assert_eq!(p2, "/dev/fake0p2");
        backend.delete_partition(&p1).await.unwrap();
        let p3 = backend
            .create_partition(
                "/dev/fake0",
                2 * ALIGNMENT,
                ALIGNMENT,
                PartitionKind::LinuxGeneric,
            )
            .await
            .unwrap();
        assert_eq!(p3, "/dev/fake0p3");
    }

    #[tokio::test]
    async fn create_table_clears_partitions() {
        let backend = FakeDiskBackend::new();
        assert!(
            !backend
                .list_partitions("/dev/fake0")
                .await
                .unwrap()
                .is_empty()
        );
        backend
            .create_table("/dev/fake0", TableKind::Gpt)
            .await
            .unwrap();
        assert!(
            backend
                .list_partitions("/dev/fake0")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn probe_ntfs_reads_fixture() {
        let backend = FakeDiskBackend::with_scenario(DiskScenario::Windows);
        let probe = backend.probe_ntfs("/dev/fake0p3").await.unwrap();
        assert!(probe.is_shrinkable());
    }
}
