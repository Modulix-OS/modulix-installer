pub mod layout;
pub mod ntfs;
/// Disk fixtures driving `engine::plan`'s scenario matrix. Test-only: they
/// used to back the `--fake-disk=` CLI flag, which no longer exists.
#[cfg(test)]
pub mod scenario;
mod udisks2;
pub mod usage;

pub use ntfs::{NtfsBlocker, NtfsProbe};
pub use udisks2::Udisks2Backend;

use crate::mx;
use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableKind {
    Gpt,
    Dos,
    /// No partition table at all — a blank disk. The udisks2 backend must
    /// enumerate these too (filtering on `PartitionTable` alone hides them),
    /// otherwise a vendor-fresh drive is invisible in the target-disk list.
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiskInfo {
    /// udisks2 object path or `/dev/...` device path, backend-defined.
    pub path: String,
    pub model: String,
    pub size_bytes: u64,
    pub is_removable: bool,
    pub has_windows: bool,
    pub table: TableKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartitionInfo {
    pub path: String,
    pub disk_path: String,
    pub fs_type: Option<String>,
    pub label: Option<String>,
    pub size_bytes: u64,
    pub start_bytes: u64,
    pub type_guid: Option<String>,
    pub uuid: Option<String>,
    pub is_esp: bool,
    /// Bytes in use inside the filesystem, when a cheap unmounted probe
    /// exists (NTFS, ext2/3/4). `None` = unknown, the disk bar draws the
    /// segment flat. `list_partitions` always leaves this `None`; it is
    /// filled asynchronously by `probe_usage` after the layout is already on
    /// screen.
    pub used_bytes: Option<u64>,
}

/// Last segment of a `path` (`/dev/nvme0n1p1` or the udisks2 object path
/// `/org/freedesktop/UDisks2/block_devices/nvme0n1p1`, backend-defined —
/// see `PartitionInfo`/`DiskInfo::path`) — the short device name to show in
/// the UI instead of the full path.
pub fn short_device_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatFs {
    Ext4,
    Fat32,
    LinuxSwap,
    Btrfs,
    Xfs,
}

impl FormatFs {
    pub fn as_mkfs_type(self) -> &'static str {
        match self {
            FormatFs::Ext4 => "ext4",
            FormatFs::Fat32 => "vfat",
            FormatFs::LinuxSwap => "swap",
            FormatFs::Btrfs => "btrfs",
            FormatFs::Xfs => "xfs",
        }
    }
}

/// Role a newly created partition plays, used to pick its GPT type GUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionKind {
    Esp,
    LinuxRoot,
    LinuxSwap,
    LinuxGeneric,
}

impl PartitionKind {
    pub fn type_guid(self) -> &'static str {
        match self {
            PartitionKind::Esp => "C12A7328-F81F-11D2-BA4B-00A0C93EC93B",
            PartitionKind::LinuxRoot => "4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709",
            PartitionKind::LinuxSwap => "0657FD6D-A4AB-43C4-84E5-0933C84B4F4F",
            PartitionKind::LinuxGeneric => "0FC63DAF-8483-4772-8E79-3D69D8477DE4",
        }
    }
}

/// Disk/partition enumeration and manipulation. Real impl talks to udisks2
/// over D-Bus; every operation that writes to a device must receive a path
/// that came out of `list_disks`/`list_partitions` — never one built by
/// concatenating user input (security requirement).
#[async_trait]
pub trait DiskBackend: Send + Sync {
    async fn list_disks(&self) -> mx::Result<Vec<DiskInfo>>;
    async fn list_partitions(&self, disk: &str) -> mx::Result<Vec<PartitionInfo>>;
    /// Resolves a `list_disks`/`list_partitions` path into a real `/dev/...`
    /// device node. On the real backend `DiskInfo::path`/`PartitionInfo::path`
    /// are udisks2 object paths, not device nodes — but `cryptsetup`,
    /// `mkswap`, `swapon`, `ntfsresize` all need the latter. The fake backend
    /// already uses `/dev/...` paths, so it returns the input unchanged.
    async fn device_node(&self, path: &str) -> mx::Result<String>;
    /// Inverse of `device_node`, for a `/dev/...` node that only starts
    /// existing at runtime rather than coming from `list_partitions` — the
    /// LUKS mapper device `CryptBackend::luks_open` creates. Real backend:
    /// polls udisks2 (which notices new kernel devices via udev, not
    /// instantaneously) for a Block object backing it and returns *that*
    /// object path, so `format_partition`/`mount` keep working on it like on
    /// any other partition. Fake backend: registers a synthetic partition
    /// entry for it and returns the node unchanged.
    async fn resolve_device(&self, device_node: &str) -> mx::Result<String>;
    async fn create_table(&self, disk: &str, table: TableKind) -> mx::Result<()>;
    async fn create_partition(
        &self,
        disk: &str,
        start_bytes: u64,
        size_bytes: u64,
        kind: PartitionKind,
    ) -> mx::Result<String>;
    async fn delete_partition(&self, partition: &str) -> mx::Result<()>;
    async fn resize_partition(&self, partition: &str, new_size_bytes: u64) -> mx::Result<()>;
    async fn format_partition(
        &self,
        partition: &str,
        fs: FormatFs,
        label: Option<&str>,
    ) -> mx::Result<()>;
    async fn probe_ntfs(&self, partition: &str) -> mx::Result<NtfsProbe>;
    /// Cheap unmounted used-space probe, `fs_type`-dependent (see
    /// `PartitionInfo::used_bytes`). `Ok(None)` for any filesystem without a
    /// cheap probe — not an error, just "unknown".
    async fn probe_usage(&self, path: &str, fs_type: Option<&str>) -> mx::Result<Option<u64>>;
    async fn mount(&self, partition: &str, target: &str) -> mx::Result<()>;
    async fn unmount(&self, partition: &str) -> mx::Result<()>;
}
