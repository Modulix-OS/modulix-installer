use super::{DiskBackend, DiskInfo, FormatFs, PartitionInfo};
use crate::mx;
use async_trait::async_trait;
use std::sync::Mutex;

/// In-memory disk backend for `--fake` and for tests: one 250 GB disk with a
/// small EFI partition and a large ext4 root, no Windows alongside it.
pub struct FakeDiskBackend {
    partitions: Mutex<Vec<PartitionInfo>>,
}

impl FakeDiskBackend {
    pub fn new() -> Self {
        Self {
            partitions: Mutex::new(vec![
                PartitionInfo {
                    path: "/dev/fake0p1".into(),
                    disk_path: "/dev/fake0".into(),
                    fs_type: Some("vfat".into()),
                    label: Some("EFI".into()),
                    size_bytes: 512 * 1024 * 1024,
                    start_bytes: 1024 * 1024,
                },
                PartitionInfo {
                    path: "/dev/fake0p2".into(),
                    disk_path: "/dev/fake0".into(),
                    fs_type: None,
                    label: None,
                    size_bytes: 249 * 1024 * 1024 * 1024,
                    start_bytes: 513 * 1024 * 1024,
                },
            ]),
        }
    }
}

impl Default for FakeDiskBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DiskBackend for FakeDiskBackend {
    async fn list_disks(&self) -> mx::Result<Vec<DiskInfo>> {
        Ok(vec![DiskInfo {
            path: "/dev/fake0".into(),
            model: "Modulix Fake NVMe 250GB".into(),
            size_bytes: 250 * 1024 * 1024 * 1024,
            is_removable: false,
            has_windows: false,
        }])
    }

    async fn list_partitions(&self, disk: &str) -> mx::Result<Vec<PartitionInfo>> {
        Ok(self
            .partitions
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.disk_path == disk)
            .cloned()
            .collect())
    }

    async fn create_partition(
        &self,
        disk: &str,
        start_bytes: u64,
        size_bytes: u64,
    ) -> mx::Result<String> {
        let mut partitions = self.partitions.lock().unwrap();
        let path = format!("{disk}p{}", partitions.len() + 1);
        partitions.push(PartitionInfo {
            path: path.clone(),
            disk_path: disk.to_string(),
            fs_type: None,
            label: None,
            size_bytes,
            start_bytes,
        });
        Ok(path)
    }

    async fn delete_partition(&self, partition: &str) -> mx::Result<()> {
        self.partitions
            .lock()
            .unwrap()
            .retain(|p| p.path != partition);
        Ok(())
    }

    async fn resize_partition(&self, partition: &str, new_size_bytes: u64) -> mx::Result<()> {
        let mut partitions = self.partitions.lock().unwrap();
        let p = partitions
            .iter_mut()
            .find(|p| p.path == partition)
            .ok_or_else(|| mx::Error::Backend(format!("no such partition: {partition}")))?;
        p.size_bytes = new_size_bytes;
        Ok(())
    }

    async fn format_partition(&self, partition: &str, fs: FormatFs) -> mx::Result<()> {
        let mut partitions = self.partitions.lock().unwrap();
        let p = partitions
            .iter_mut()
            .find(|p| p.path == partition)
            .ok_or_else(|| mx::Error::Backend(format!("no such partition: {partition}")))?;
        p.fs_type = Some(fs.as_mkfs_type().to_string());
        Ok(())
    }

    async fn mount(&self, _partition: &str, _target: &str) -> mx::Result<()> {
        Ok(())
    }
}
