//! In-memory `DiskBackend` driving `--fake-disks=`. Ports the fake disk
//! backend `eb6840b` removed (`git show eb6840b^:src/backend/disk/fake.rs`),
//! updated for the two contract changes the real backend grew since:
//! `partition_uuid` (not part of the trait back then) and a `usage` map
//! fixtures now carry instead of the backend inventing it.

use super::fault::{Fault, format_fault_for, mkfs_failure_message};
use super::{FaultPlan, require_device_node};
use crate::backend::disk::layout::ALIGNMENT;
use crate::backend::disk::scenario::DiskScenario;
use crate::backend::disk::{
    DiskBackend, DiskInfo, FormatFs, NtfsProbe, PartitionInfo, PartitionKind, TableKind,
};
use crate::mx;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

struct FakeState {
    disks: Vec<DiskInfo>,
    partitions: Vec<PartitionInfo>,
    ntfs: HashMap<String, NtfsProbe>,
    usage: HashMap<String, u64>,
}

/// Renumbers every scenario's disks/partitions into one consistent
/// `/dev/fakeN` namespace and merges their fixtures — each scenario name in
/// `--fake-disks=a,b,c` contributes the first disk of its fixture (all four,
/// for `multi-disk`), in the order given.
fn compose(scenarios: &[DiskScenario]) -> FakeState {
    let mut disks = Vec::new();
    let mut partitions = Vec::new();
    let mut ntfs = HashMap::new();
    let mut usage = HashMap::new();
    let mut next_index = 0usize;

    for scenario in scenarios {
        let fixture = scenario.fixture();
        let mut disk_rename: HashMap<String, String> = HashMap::new();
        for d in &fixture.disks {
            disk_rename.insert(d.path.clone(), format!("/dev/fake{next_index}"));
            next_index += 1;
        }

        for mut d in fixture.disks {
            d.path = disk_rename[&d.path].clone();
            disks.push(d);
        }

        for mut p in fixture.partitions {
            let old_path = p.path.clone();
            let old_disk = p.disk_path.clone();
            let new_disk = disk_rename
                .get(&old_disk)
                .cloned()
                .unwrap_or_else(|| old_disk.clone());
            let suffix = old_path.strip_prefix(&old_disk).unwrap_or(&old_path);
            let new_path = format!("{new_disk}{suffix}");

            if let Some(probe) = fixture.ntfs.get(&old_path) {
                ntfs.insert(new_path.clone(), probe.clone());
            }
            if let Some(&bytes) = fixture.usage.get(&old_path) {
                usage.insert(new_path.clone(), bytes);
            }

            p.disk_path = new_disk;
            p.path = new_path;
            partitions.push(p);
        }
    }

    FakeState {
        disks,
        partitions,
        ntfs,
        usage,
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

/// Derives a stable, fixture-looking UUID from a device path, so repeated
/// calls for the same partition always agree (real udisks2 would too).
fn deterministic_uuid(path: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in path.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (hash >> 32) as u32,
        (hash >> 16) as u16,
        (hash as u16 & 0x0fff) | 0x4000,
        ((hash >> 48) as u16 & 0x3fff) | 0x8000,
        hash & 0xffff_ffff_ffff
    )
}

pub struct FakeDiskBackend {
    state: Mutex<FakeState>,
    faults: Arc<FaultPlan>,
}

impl FakeDiskBackend {
    pub fn new(scenarios: &[DiskScenario], faults: Arc<FaultPlan>) -> Self {
        Self {
            state: Mutex::new(compose(scenarios)),
            faults,
        }
    }
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
        require_device_node(path)?;
        Ok(path.to_string())
    }

    async fn resolve_device(&self, device_node: &str) -> mx::Result<String> {
        require_device_node(device_node)?;
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

    async fn partition_uuid(&self, path: &str) -> mx::Result<String> {
        if self.faults.is(Fault::UuidMissing) {
            return Err(mx::Error::Backend(format!(
                "udisks2 reports no UUID for {path}"
            )));
        }
        Ok(format!("/dev/disk/by-uuid/{}", deterministic_uuid(path)))
    }

    async fn create_table(&self, disk: &str, table: TableKind) -> mx::Result<()> {
        if self.faults.is(Fault::PartitionCreateTable) {
            return Err(mx::Error::Backend(
                "udisks2 Format failed: device or resource busy".into(),
            ));
        }
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
        if self.faults.is(Fault::PartitionCreate) {
            return Err(mx::Error::Backend(
                "no space left for the requested partition".into(),
            ));
        }

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
        if self.faults.is(Fault::PartitionResizeNtfs) {
            return Err(mx::Error::Backend(
                "ntfsresize refused: volume needs checking".into(),
            ));
        }

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
        if self.faults.fault == Some(format_fault_for(fs)) {
            return Err(mx::Error::Backend(mkfs_failure_message(fs)));
        }

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

    async fn mount(&self, partition: &str, target: &str) -> mx::Result<()> {
        let fault = if target.ends_with("/boot") {
            Fault::MountEsp
        } else if target.ends_with("/home") {
            return Ok(());
        } else {
            Fault::MountRoot
        };
        if self.faults.is(fault) {
            return Err(mx::Error::Backend(format!(
                "mount {partition} on {target} failed: Device or resource busy"
            )));
        }
        Ok(())
    }

    async fn unmount(&self, _partition: &str) -> mx::Result<()> {
        if self.faults.is(Fault::PartitionUnmountBusy) {
            return Err(mx::Error::Backend("target is busy".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debugsim::FaultPlan;

    fn backend(scenarios: &[DiskScenario]) -> FakeDiskBackend {
        FakeDiskBackend::new(scenarios, Arc::new(FaultPlan { fault: None }))
    }

    #[tokio::test]
    async fn composes_disks_into_a_single_numbered_namespace() {
        let backend = backend(&[DiskScenario::Empty, DiskScenario::Linux]);
        let disks = backend.list_disks().await.unwrap();
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].path, "/dev/fake0");
        assert_eq!(disks[1].path, "/dev/fake1");

        let partitions = backend.list_partitions("/dev/fake1").await.unwrap();
        assert_eq!(partitions.len(), 2);
        assert!(partitions.iter().all(|p| p.disk_path == "/dev/fake1"));
        assert!(
            partitions
                .iter()
                .any(|p| p.path == "/dev/fake1p1" && p.is_esp)
        );
    }

    #[tokio::test]
    async fn multi_disk_contributes_all_four_disks() {
        let backend = backend(&[DiskScenario::MultiDisk]);
        let disks = backend.list_disks().await.unwrap();
        assert_eq!(disks.len(), 4);
        assert_eq!(disks[3].path, "/dev/fake3");
    }

    #[tokio::test]
    async fn renumbering_keeps_scenarios_after_multi_disk_distinct() {
        let backend = backend(&[DiskScenario::MultiDisk, DiskScenario::Windows]);
        let disks = backend.list_disks().await.unwrap();
        assert_eq!(disks.len(), 5);
        assert_eq!(disks[4].path, "/dev/fake4");
        let partitions = backend.list_partitions("/dev/fake4").await.unwrap();
        assert!(!partitions.is_empty());
        assert!(partitions.iter().all(|p| p.disk_path == "/dev/fake4"));
    }

    #[tokio::test]
    async fn rejects_overlapping_partition() {
        let backend = backend(&[DiskScenario::Linux]);
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
        let backend = backend(&[DiskScenario::Empty]);
        let err = backend
            .create_partition("/dev/fake0", 1, ALIGNMENT, PartitionKind::LinuxGeneric)
            .await
            .unwrap_err();
        assert!(matches!(err, mx::Error::Backend(_)));
    }

    #[tokio::test]
    async fn free_gaps_sees_the_composed_layout() {
        let backend = backend(&[DiskScenario::Windows]);
        let disks = backend.list_disks().await.unwrap();
        let partitions = backend.list_partitions(&disks[0].path).await.unwrap();
        let gaps = crate::backend::disk::layout::free_gaps(disks[0].size_bytes, &partitions);
        // ESP+MSR+NTFS+WinRE leave only the 1 MiB GPT header reserve before
        // the ESP free — composing scenarios must not invent or lose gaps.
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].start_bytes, 0);
        assert_eq!(gaps[0].size_bytes, 1024 * 1024);
    }

    #[tokio::test]
    async fn format_fault_trips_only_the_matching_filesystem() {
        let faults = Arc::new(FaultPlan {
            fault: Some(Fault::FormatSwap),
        });
        let backend = FakeDiskBackend::new(&[DiskScenario::Empty], faults);
        let path = backend
            .create_partition("/dev/fake0", 0, ALIGNMENT, PartitionKind::LinuxGeneric)
            .await
            .unwrap();
        // ext4 is unaffected by a swap-targeted fault.
        backend
            .format_partition(&path, FormatFs::Ext4, None)
            .await
            .unwrap();
        let err = backend
            .format_partition(&path, FormatFs::LinuxSwap, None)
            .await
            .unwrap_err();
        assert!(matches!(err, mx::Error::Backend(_)));
    }

    #[tokio::test]
    async fn uuid_missing_fault_fails_partition_uuid() {
        let faults = Arc::new(FaultPlan {
            fault: Some(Fault::UuidMissing),
        });
        let backend = FakeDiskBackend::new(&[DiskScenario::Linux], faults);
        let err = backend.partition_uuid("/dev/fake0p2").await.unwrap_err();
        assert!(matches!(err, mx::Error::Backend(_)));
    }
}
