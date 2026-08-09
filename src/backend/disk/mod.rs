mod fake;
mod udisks2;

pub use fake::FakeDiskBackend;
pub use udisks2::Udisks2Backend;

use crate::mx;
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq)]
pub struct DiskInfo {
    /// udisks2 object path or `/dev/...` device path, backend-defined.
    pub path: String,
    pub model: String,
    pub size_bytes: u64,
    pub is_removable: bool,
    pub has_windows: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartitionInfo {
    pub path: String,
    pub disk_path: String,
    pub fs_type: Option<String>,
    pub label: Option<String>,
    pub size_bytes: u64,
    pub start_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatFs {
    Ext4,
    Fat32,
    LinuxSwap,
}

impl FormatFs {
    pub fn as_mkfs_type(self) -> &'static str {
        match self {
            FormatFs::Ext4 => "ext4",
            FormatFs::Fat32 => "vfat",
            FormatFs::LinuxSwap => "swap",
        }
    }
}

/// Disk/partition enumeration and manipulation. Real impl talks to udisks2
/// over D-Bus; every operation that writes to a device must receive a path
/// that came out of `list_disks`/`list_partitions` — never one built by
/// concatenating user input (security requirement, see CLAUDE.md).
#[async_trait]
pub trait DiskBackend: Send + Sync {
    async fn list_disks(&self) -> mx::Result<Vec<DiskInfo>>;
    async fn list_partitions(&self, disk: &str) -> mx::Result<Vec<PartitionInfo>>;
    async fn create_partition(
        &self,
        disk: &str,
        start_bytes: u64,
        size_bytes: u64,
    ) -> mx::Result<String>;
    async fn delete_partition(&self, partition: &str) -> mx::Result<()>;
    async fn resize_partition(&self, partition: &str, new_size_bytes: u64) -> mx::Result<()>;
    async fn format_partition(&self, partition: &str, fs: FormatFs) -> mx::Result<()>;
    async fn mount(&self, partition: &str, target: &str) -> mx::Result<()>;
}
