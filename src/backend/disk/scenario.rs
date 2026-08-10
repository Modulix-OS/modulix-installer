//! Disk fixtures shared by `#[cfg(test)]` and `--fake-disk=<scenario>`, so
//! the same scenario a test asserts against can be clicked through in the UI
//! by hand. See the partitioning plan (step 7) for the table these mirror.

use super::ntfs::{NtfsBlocker, NtfsProbe};
use super::{DiskInfo, PartitionInfo, PartitionKind, TableKind};
use std::collections::HashMap;

const fn mib(n: u64) -> u64 {
    n * 1024 * 1024
}

const fn gib(n: u64) -> u64 {
    n * 1024 * 1024 * 1024
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskScenario {
    Linux,
    Empty,
    Windows,
    WindowsFull,
    BitLocker,
    DirtyNtfs,
    FreeSpaceTail,
    Fragmented,
    Tiny,
    MultiDisk,
    Mbr,
}

impl DiskScenario {
    pub const ALL: [DiskScenario; 11] = [
        DiskScenario::Linux,
        DiskScenario::Empty,
        DiskScenario::Windows,
        DiskScenario::WindowsFull,
        DiskScenario::BitLocker,
        DiskScenario::DirtyNtfs,
        DiskScenario::FreeSpaceTail,
        DiskScenario::Fragmented,
        DiskScenario::Tiny,
        DiskScenario::MultiDisk,
        DiskScenario::Mbr,
    ];

    pub fn name(self) -> &'static str {
        match self {
            DiskScenario::Linux => "linux",
            DiskScenario::Empty => "empty",
            DiskScenario::Windows => "windows",
            DiskScenario::WindowsFull => "windows-full",
            DiskScenario::BitLocker => "bitlocker",
            DiskScenario::DirtyNtfs => "dirty-ntfs",
            DiskScenario::FreeSpaceTail => "freespace-tail",
            DiskScenario::Fragmented => "fragmented",
            DiskScenario::Tiny => "tiny",
            DiskScenario::MultiDisk => "multi-disk",
            DiskScenario::Mbr => "mbr",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scenario| scenario.name() == s)
    }

    pub fn fixture(self) -> DiskFixture {
        match self {
            DiskScenario::Linux => linux(),
            DiskScenario::Empty => empty(),
            DiskScenario::Windows => windows(false),
            DiskScenario::WindowsFull => windows(true),
            DiskScenario::BitLocker => bitlocker(),
            DiskScenario::DirtyNtfs => dirty_ntfs(),
            DiskScenario::FreeSpaceTail => freespace_tail(),
            DiskScenario::Fragmented => fragmented(),
            DiskScenario::Tiny => tiny(),
            DiskScenario::MultiDisk => multi_disk(),
            DiskScenario::Mbr => mbr(),
        }
    }
}

pub struct DiskFixture {
    pub disks: Vec<DiskInfo>,
    pub partitions: Vec<PartitionInfo>,
    pub ntfs: HashMap<String, NtfsProbe>,
    /// Fixture data for `FakeDiskBackend::probe_usage` — `(partition_path,
    /// used_bytes)`, mirroring what `dumpe2fs`/`ntfsresize` would report on
    /// real hardware. Not consulted by `engine::plan` itself.
    pub usage: HashMap<String, u64>,
}

const DISK0: &str = "/dev/fake0";
const DISK1: &str = "/dev/fake1";
const DISK2: &str = "/dev/fake2";
const DISK3: &str = "/dev/fake3";

fn disk(
    path: &str,
    model: &str,
    size_bytes: u64,
    is_removable: bool,
    has_windows: bool,
) -> DiskInfo {
    DiskInfo {
        path: path.to_string(),
        model: model.to_string(),
        size_bytes,
        is_removable,
        has_windows,
        table: TableKind::Gpt,
    }
}

fn part(
    disk_path: &str,
    index: u32,
    start_bytes: u64,
    size_bytes: u64,
    fs_type: Option<&str>,
    label: Option<&str>,
    kind: Option<PartitionKind>,
) -> PartitionInfo {
    PartitionInfo {
        path: format!("{disk_path}p{index}"),
        disk_path: disk_path.to_string(),
        fs_type: fs_type.map(str::to_string),
        label: label.map(str::to_string),
        size_bytes,
        start_bytes,
        type_guid: kind.map(|k| k.type_guid().to_string()),
        uuid: None,
        is_esp: matches!(kind, Some(PartitionKind::Esp)),
        used_bytes: None,
    }
}

fn linux() -> DiskFixture {
    let esp = part(
        DISK0,
        1,
        mib(1),
        mib(512),
        Some("vfat"),
        Some("EFI"),
        Some(PartitionKind::Esp),
    );
    let root = part(
        DISK0,
        2,
        mib(513),
        gib(249),
        Some("ext4"),
        Some("Root"),
        Some(PartitionKind::LinuxRoot),
    );
    let mut usage = HashMap::new();
    usage.insert(root.path.clone(), gib(150));
    DiskFixture {
        disks: vec![disk(
            DISK0,
            "Modulix Fake NVMe 250GB",
            gib(250),
            false,
            false,
        )],
        partitions: vec![esp, root],
        ntfs: HashMap::new(),
        usage,
    }
}

fn empty() -> DiskFixture {
    DiskFixture {
        disks: vec![DiskInfo {
            table: TableKind::None,
            ..disk(DISK0, "Fake Blank SSD 500GB", gib(500), false, false)
        }],
        partitions: vec![],
        ntfs: HashMap::new(),
        usage: HashMap::new(),
    }
}

/// `full` selects the "not enough room to shrink" variant (min resize size
/// 395 GiB out of a 400 GiB volume, versus 210 GiB when there's slack).
fn windows(full: bool) -> DiskFixture {
    let esp = part(
        DISK0,
        1,
        mib(1),
        mib(100),
        Some("vfat"),
        Some("EFI"),
        Some(PartitionKind::Esp),
    );
    let msr = part(DISK0, 2, mib(101), mib(16), None, Some("MSR"), None);
    let ntfs_start = mib(117);
    let ntfs_size = gib(400);
    let ntfs = part(
        DISK0,
        3,
        ntfs_start,
        ntfs_size,
        Some("ntfs"),
        Some("Windows"),
        None,
    );
    let winre_start = ntfs_start + ntfs_size;
    let winre = part(
        DISK0,
        4,
        winre_start,
        mib(700),
        Some("ntfs"),
        Some("WinRE"),
        None,
    );
    let disk_size = winre_start + mib(700) + mib(1);

    let mut probes = HashMap::new();
    let ntfs_used_bytes = if full { gib(395) } else { gib(110) };
    let ntfs_path = ntfs.path.clone();
    probes.insert(
        ntfs_path.clone(),
        NtfsProbe {
            min_size_bytes: ntfs_used_bytes,
            current_size_bytes: ntfs_size,
            blockers: vec![],
            used_bytes: Some(ntfs_used_bytes),
        },
    );

    DiskFixture {
        disks: vec![disk(
            DISK0,
            "Fake Windows Laptop SSD",
            disk_size,
            false,
            true,
        )],
        partitions: vec![esp, msr, ntfs, winre],
        ntfs: probes,
        usage: HashMap::from([(ntfs_path, ntfs_used_bytes)]),
    }
}

fn bitlocker() -> DiskFixture {
    let mut fixture = windows(false);
    let ntfs_path = fixture.partitions[2].path.clone();
    fixture.ntfs.insert(
        ntfs_path,
        NtfsProbe {
            min_size_bytes: 0,
            current_size_bytes: gib(400),
            blockers: vec![NtfsBlocker::BitLocker],
            used_bytes: None,
        },
    );
    fixture
}

fn dirty_ntfs() -> DiskFixture {
    let mut fixture = windows(false);
    let ntfs_path = fixture.partitions[2].path.clone();
    fixture.ntfs.insert(
        ntfs_path,
        NtfsProbe {
            min_size_bytes: gib(210),
            current_size_bytes: gib(400),
            blockers: vec![NtfsBlocker::Hibernated, NtfsBlocker::DirtyVolume],
            used_bytes: None,
        },
    );
    fixture
}

fn freespace_tail() -> DiskFixture {
    let esp = part(
        DISK0,
        1,
        mib(1),
        mib(512),
        Some("vfat"),
        Some("EFI"),
        Some(PartitionKind::Esp),
    );
    let ntfs_start = mib(513);
    let ntfs_size = gib(200);
    let ntfs = part(
        DISK0,
        2,
        ntfs_start,
        ntfs_size,
        Some("ntfs"),
        Some("Windows"),
        None,
    );
    let gap = gib(120);
    let disk_size = ntfs_start + ntfs_size + gap + mib(1);

    DiskFixture {
        disks: vec![disk(DISK0, "Fake Dual-Boot SSD", disk_size, false, true)],
        partitions: vec![esp, ntfs],
        ntfs: HashMap::new(),
        usage: HashMap::new(),
    }
}

/// Existing Linux install chopped up by five partitions and five gaps of
/// wildly different sizes — exercises `FreeSpace` mode picking the *largest*
/// gap out of several candidates (versus `freespace_tail`'s single trailing
/// one) and manual-mode creation into a gap that isn't at either end of the
/// disk. The 400 GiB gap between `Extra` and `Data` stays the largest one on
/// purpose — `fragmented_manual_create_root_in_middle_gap` in
/// `engine::plan`'s tests asserts that exact size.
fn fragmented() -> DiskFixture {
    let esp = part(
        DISK0,
        1,
        mib(1),
        mib(512),
        Some("vfat"),
        Some("EFI"),
        Some(PartitionKind::Esp),
    );
    let root_start = mib(513);
    let root_size = gib(100);
    let root = part(
        DISK0,
        2,
        root_start,
        root_size,
        Some("ext4"),
        Some("Root"),
        Some(PartitionKind::LinuxRoot),
    );
    let root_end = root_start + root_size;

    let gap1 = gib(5);
    let old_start = root_end + gap1;
    let old_size = gib(20);
    let old_distro = part(
        DISK0,
        3,
        old_start,
        old_size,
        Some("ext4"),
        Some("OldDistro"),
        None,
    );
    let old_end = old_start + old_size;

    let gap2 = mib(300);
    let swap_start = old_end + gap2;
    let swap_size = gib(8);
    let swap = part(
        DISK0,
        4,
        swap_start,
        swap_size,
        Some("swap"),
        Some("Swap"),
        None,
    );
    let swap_end = swap_start + swap_size;

    let gap3 = gib(10);
    let extra_start = swap_end + gap3;
    let extra_size = gib(30);
    let extra = part(
        DISK0,
        5,
        extra_start,
        extra_size,
        Some("ext4"),
        Some("Extra"),
        None,
    );
    let extra_end = extra_start + extra_size;

    let big_gap = gib(400);
    let data_start = extra_end + big_gap;
    let data_size = gib(50);
    let data = part(
        DISK0,
        6,
        data_start,
        data_size,
        Some("ext4"),
        Some("Data"),
        None,
    );
    let data_end = data_start + data_size;

    let gap4 = mib(700);
    let tail_start = data_end + gap4;
    let tail_size = gib(2);
    let tail = part(
        DISK0,
        7,
        tail_start,
        tail_size,
        Some("ext4"),
        Some("Tail"),
        None,
    );
    let disk_size = tail_start + tail_size + mib(1);

    let usage = HashMap::from([
        (root.path.clone(), gib(60)),
        (old_distro.path.clone(), gib(15)),
        (extra.path.clone(), gib(25)),
        (data.path.clone(), gib(30)),
        (tail.path.clone(), gib(1)),
    ]);
    DiskFixture {
        disks: vec![disk(DISK0, "Fake Fragmented SSD", disk_size, false, false)],
        partitions: vec![esp, root, old_distro, swap, extra, data, tail],
        ntfs: HashMap::new(),
        usage,
    }
}

fn tiny() -> DiskFixture {
    DiskFixture {
        disks: vec![DiskInfo {
            table: TableKind::None,
            ..disk(DISK0, "Fake Tiny USB Stick", gib(8), false, false)
        }],
        partitions: vec![],
        ntfs: HashMap::new(),
        usage: HashMap::new(),
    }
}

/// Four disks at once, so the "Target disk" dropdown has something real to
/// switch between: a blank drive (index 0), the install USB itself — must
/// get filtered by the live-medium guard, not just flagged removable
/// (index 1), a Windows laptop SSD (index 2), and a disk with an existing
/// Linux install already on it (index 3).
fn multi_disk() -> DiskFixture {
    let nvme = DiskInfo {
        table: TableKind::None,
        ..disk(DISK0, "Fake NVMe 1TB", gib(1024), false, false)
    };
    let usb_esp = part(
        DISK1,
        1,
        mib(1),
        mib(500),
        Some("vfat"),
        Some("MODULIX-ISO"),
        None,
    );
    let usb = disk(DISK1, "Fake Install USB Key 64GB", gib(64), true, false);

    let windows_esp = part(
        DISK2,
        1,
        mib(1),
        mib(100),
        Some("vfat"),
        Some("EFI"),
        Some(PartitionKind::Esp),
    );
    let windows_msr = part(DISK2, 2, mib(101), mib(16), None, Some("MSR"), None);
    let windows_ntfs_start = mib(117);
    let windows_ntfs_size = gib(300);
    let windows_ntfs = part(
        DISK2,
        3,
        windows_ntfs_start,
        windows_ntfs_size,
        Some("ntfs"),
        Some("Windows"),
        None,
    );
    let windows_size = windows_ntfs_start + windows_ntfs_size + mib(1);
    let windows = disk(
        DISK2,
        "Fake Windows Laptop SSD (secondary)",
        windows_size,
        false,
        true,
    );
    let mut ntfs_probes = HashMap::new();
    ntfs_probes.insert(
        windows_ntfs.path.clone(),
        NtfsProbe {
            min_size_bytes: gib(150),
            current_size_bytes: windows_ntfs_size,
            blockers: vec![],
            used_bytes: Some(gib(150)),
        },
    );

    let existing_esp = part(
        DISK3,
        1,
        mib(1),
        mib(512),
        Some("vfat"),
        Some("EFI"),
        Some(PartitionKind::Esp),
    );
    let existing_root = part(
        DISK3,
        2,
        mib(513),
        gib(119),
        Some("ext4"),
        Some("Root"),
        Some(PartitionKind::LinuxRoot),
    );
    let existing = disk(DISK3, "Fake Existing Linux SSD", gib(120), false, false);

    let usage = HashMap::from([
        (windows_ntfs.path.clone(), gib(150)),
        (existing_root.path.clone(), gib(80)),
    ]);

    DiskFixture {
        disks: vec![nvme, usb, windows, existing],
        partitions: vec![
            usb_esp,
            windows_esp,
            windows_msr,
            windows_ntfs,
            existing_esp,
            existing_root,
        ],
        ntfs: ntfs_probes,
        usage,
    }
}

fn mbr() -> DiskFixture {
    let size = gib(250);
    let quarter = size / 4;
    let partitions = (0..4)
        .map(|i| {
            part(
                DISK0,
                i + 1,
                quarter * i as u64,
                quarter,
                Some("ext4"),
                Some(&format!("part{}", i + 1)),
                None,
            )
        })
        .collect();

    DiskFixture {
        disks: vec![DiskInfo {
            table: TableKind::Dos,
            ..disk(DISK0, "Fake Old BIOS Disk 250GB", size, false, false)
        }],
        partitions,
        ntfs: HashMap::new(),
        usage: HashMap::new(),
    }
}
