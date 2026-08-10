//! Pure partitioning planner — the single source of truth for what step 7
//! is about to do to a disk. Shared by the UI (live validation + the "after"
//! preview bar) and `engine::tasks::PartitionTask` (execution), so the two
//! can never diverge: whatever the preview shows is exactly what gets
//! written.

use crate::backend::disk::layout::{ALIGNMENT, FreeGap, align_down, align_up};
use crate::backend::disk::{
    DiskInfo, FormatFs, NtfsBlocker, NtfsProbe, PartitionInfo, PartitionKind, TableKind,
    short_device_name,
};
use crate::config::{PartitionMode, PartitioningConfig, SwapMode};
use crate::engine::sizing::compute_swap_bytes;
use crate::i18n::tr;

/// Floor for a Linux root, `AlongsideWindows`/`FreeSpace` modes — below this
/// the OS wouldn't have room to actually work with (this planner's own rule
/// of thumb).
pub const MIN_LINUX_BYTES: u64 = 25 * 1024 * 1024 * 1024;
/// UEFI spec floor for a usable FAT32 ESP.
pub const MIN_ESP_BYTES: u64 = 100 * 1024 * 1024;
/// Size for a freshly created ESP (generous headroom for multiple kernels).
/// Modulix always gets its own ESP — never reuses/reformats the Windows one
/// (dual-boot goes through a limine `extraEntries` instead, iteration 2).
pub const NEW_ESP_BYTES: u64 = 1024 * 1024 * 1024;

pub struct PlanInput {
    pub disk: DiskInfo,
    pub partitions: Vec<PartitionInfo>,
    pub gaps: Vec<FreeGap>,
    /// `(partition_path, probe)` for the Windows NTFS partition on `disk`,
    /// if any — required for `AlongsideWindows`, unused otherwise.
    pub ntfs: Option<(String, NtfsProbe)>,
    pub cfg: PartitioningConfig,
    pub ram_bytes: u64,
    pub uefi: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlanOp {
    CreateTable {
        disk: String,
        table: TableKind,
    },
    ShrinkNtfs {
        path: String,
        new_size_bytes: u64,
    },
    Create {
        role: PartitionRole,
        start_bytes: u64,
        size_bytes: u64,
        kind: PartitionKind,
        fs: FormatFs,
    },
    UseExisting {
        path: String,
        role: PartitionRole,
        format: Option<FormatFs>,
    },
}

/// Role an *operation* (`PlanOp::Create`/`UseExisting`) assigns to a
/// partition. Deliberately has no "leave untouched" variant — a kept
/// partition isn't the result of any op, it's simply absent from `ops`; see
/// [`PreviewRole::Keep`] for how that's represented in the *display* preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionRole {
    Esp,
    Root,
    Swap,
    Home,
}

fn role_label(role: PartitionRole) -> String {
    match role {
        PartitionRole::Esp => tr("EFI System Partition"),
        PartitionRole::Root => "Modulix OS".to_string(),
        PartitionRole::Swap => tr("Swap"),
        PartitionRole::Home => tr("Home"),
    }
}

/// Display role for a preview segment — a superset of [`PartitionRole`] (the
/// enum of *operations*) with [`PreviewRole::Keep`] (an existing partition
/// the plan leaves untouched) and [`PreviewRole::Free`] (unallocated space),
/// neither of which is ever something the plan writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewRole {
    Esp,
    Root,
    Swap,
    Home,
    Keep,
    Free,
}

impl From<PartitionRole> for PreviewRole {
    fn from(role: PartitionRole) -> Self {
        match role {
            PartitionRole::Esp => PreviewRole::Esp,
            PartitionRole::Root => PreviewRole::Root,
            PartitionRole::Swap => PreviewRole::Swap,
            PartitionRole::Home => PreviewRole::Home,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreviewSegment {
    pub label: String,
    pub size_bytes: u64,
    /// Offset from the start of the disk — lets the UI lay "before" and
    /// "after" bars out proportionally to the *whole disk*, not just to the
    /// sum of the segments each one happens to carry.
    pub start_bytes: u64,
    pub role: PreviewRole,
    pub fs_type: Option<String>,
    /// True on the root segment when `PartitioningConfig::encryption_enabled`
    /// — `EncryptTask` only ever encrypts the root (`tasks/encrypt.rs`).
    pub encrypted: bool,
    /// Copied from `PartitionInfo::used_bytes` for `PreviewRole::Keep`
    /// segments only — a freshly created partition is always empty.
    pub used_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartitionPlan {
    pub ops: Vec<PlanOp>,
    /// The disk's full layout after `ops` runs, left to right — feeds the
    /// "after" `DiskBar`.
    pub preview: Vec<PreviewSegment>,
    /// Paths that get destroyed (deleted, or reformatted-in-place) — feeds
    /// the destructive-confirmation dialog's explicit list.
    pub erases: Vec<String>,
}

/// Non-fatal advisory surfaced alongside a successful plan — unlike
/// `PlanError`, doesn't block "Next"; the UI shows it as a dismissible
/// warning (e.g. a banner with a "Continue anyway" affordance).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanWarning {
    /// The target disk is removable — likely the install medium itself, or
    /// at least not what most users mean by "my computer's disk".
    RemovableTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlanError {
    NoDisk,
    DiskTooSmall { needed: u64, available: u64 },
    NoWindows,
    NtfsBlocked(NtfsBlocker),
    ShrinkTooSmall,
    NoFreeSpace,
    GapTooSmall,
    NoEsp,
    NoRoot,
    MultipleRoots,
    DuplicateMountPoint,
    SwapTooSmallForHibernation,
    MissingPassphrase,
    PassphraseMismatch,
    MbrNeedsConversion,
    StaleLayout,
}

impl PlanError {
    pub fn msgid(&self) -> String {
        match self {
            PlanError::NoDisk => tr("Select a target disk"),
            PlanError::DiskTooSmall { .. } => tr("This disk is too small for Modulix OS"),
            PlanError::NoWindows => tr("No Windows installation was found on this disk"),
            PlanError::NtfsBlocked(NtfsBlocker::BitLocker) => tr(
                "To dual-boot with Windows, this partition needs BitLocker suspended first: suspend it in Windows, or shrink the partition from Windows Disk Management, then restart the installer.",
            ),
            PlanError::NtfsBlocked(NtfsBlocker::Hibernated) => tr(
                "To dual-boot with Windows, restart Windows fully first (not just sign out) and try again",
            ),
            PlanError::NtfsBlocked(NtfsBlocker::DirtyVolume) => tr(
                "To dual-boot with Windows, this partition needs a disk check first — boot into Windows and run \"chkdsk\"",
            ),
            PlanError::NtfsBlocked(NtfsBlocker::ResizeUnsupported) => tr(
                "This Windows partition can't be resized, so dual-boot with Windows isn't possible",
            ),
            PlanError::ShrinkTooSmall => {
                tr("Not enough space would be freed up to install Modulix OS")
            }
            PlanError::NoFreeSpace => tr("Select a free space region to install into"),
            PlanError::GapTooSmall => tr("This free space region is too small"),
            PlanError::NoEsp => tr("An EFI System Partition of at least 100 MiB is required"),
            PlanError::NoRoot => tr("Assign exactly one partition as the root (/) filesystem"),
            PlanError::MultipleRoots => tr("Only one partition can be assigned as root (/)"),
            PlanError::DuplicateMountPoint => {
                tr("Only one partition can be assigned to /boot, /home, or swap")
            }
            PlanError::SwapTooSmallForHibernation => tr(
                "The swap partition must be at least as large as this machine's RAM for hibernation",
            ),
            PlanError::MissingPassphrase => tr("Enter an encryption passphrase"),
            PlanError::PassphraseMismatch => tr("Confirm the encryption passphrase"),
            PlanError::MbrNeedsConversion => tr(
                "This disk uses an old MBR partition table — convert it to GPT to use free space",
            ),
            PlanError::StaleLayout => {
                tr("The disk layout changed — refresh the disk and try again")
            }
        }
    }
}

pub fn plan(input: &PlanInput) -> Result<PartitionPlan, PlanError> {
    if input.disk.size_bytes == 0 {
        return Err(PlanError::NoDisk);
    }

    match input.cfg.mode {
        PartitionMode::EntireDisk => plan_entire_disk(input),
        PartitionMode::AlongsideWindows => plan_alongside_windows(input),
        PartitionMode::FreeSpace => plan_free_space(input),
        PartitionMode::Manual => plan_manual(input),
    }
}

/// Checks constraints that aren't about the disk layout itself — currently
/// just the encryption passphrase — so callers can paint the "after" preview
/// (from `plan()`) even while this fails, instead of blanking it out.
/// `passphrase_confirmed` mirrors `PasswordConfirmEntry::is_valid()`: false
/// whenever the confirmation field is empty or doesn't match, blocking both
/// the same way `steps/user.rs` already blocks the account password.
pub fn validate_config(
    cfg: &PartitioningConfig,
    passphrase_confirmed: bool,
) -> Result<(), PlanError> {
    if cfg.encryption_enabled {
        if cfg.encryption_passphrase.trim().is_empty() {
            return Err(PlanError::MissingPassphrase);
        }
        if !passphrase_confirmed {
            return Err(PlanError::PassphraseMismatch);
        }
    }
    Ok(())
}

/// Advisory warnings for an already-successful plan — currently just the
/// removable-disk heads-up (see [`PlanWarning`]).
pub fn warnings(input: &PlanInput) -> Vec<PlanWarning> {
    let mut warnings = Vec::new();
    if input.disk.is_removable {
        warnings.push(PlanWarning::RemovableTarget);
    }
    warnings
}

fn swap_bytes_for(input: &PlanInput) -> u64 {
    compute_swap_bytes(input.cfg.swap_mode, input.ram_bytes)
}

/// Bounds on `PartitioningConfig::shrink_to_bytes` that
/// `plan_alongside_windows` will actually accept, ESP + swap overhead
/// already deducted — the single source of truth for the UI's shrink slider
/// (`steps/partitioning.rs`'s `refresh_windows_scale`), so the slider's range
/// can never drift from what `plan()` itself will produce. `None` when no
/// value would leave room for a `MIN_LINUX_BYTES` root — the caller should
/// report `PlanError::ShrinkTooSmall` and disable the slider.
pub fn shrink_bounds(
    ntfs_size_bytes: u64,
    probe: &NtfsProbe,
    swap_bytes: u64,
    uefi: bool,
) -> Option<ShrinkBounds> {
    let esp_bytes = if uefi { NEW_ESP_BYTES } else { 0 };
    let min = align_up(probe.min_size_bytes);
    let max = align_down(
        ntfs_size_bytes
            .saturating_sub(esp_bytes)
            .saturating_sub(swap_bytes)
            .saturating_sub(MIN_LINUX_BYTES),
    );
    (max > min).then_some(ShrinkBounds { min, max })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShrinkBounds {
    pub min: u64,
    pub max: u64,
}

fn plan_entire_disk(input: &PlanInput) -> Result<PartitionPlan, PlanError> {
    let swap_bytes = align_up(swap_bytes_for(input));
    let esp_bytes = NEW_ESP_BYTES;
    let disk_size = input.disk.size_bytes;
    let usable = disk_size.saturating_sub(crate::backend::disk::layout::GPT_TAIL_RESERVE);

    let esp_start = ALIGNMENT;
    let esp_end = esp_start + esp_bytes;
    let root_start = esp_end;
    let root_size = usable.saturating_sub(root_start).saturating_sub(swap_bytes);

    if root_size < MIN_LINUX_BYTES {
        return Err(PlanError::DiskTooSmall {
            needed: root_start + MIN_LINUX_BYTES + swap_bytes,
            available: disk_size,
        });
    }

    let mut ops = vec![
        PlanOp::CreateTable {
            disk: input.disk.path.clone(),
            table: TableKind::Gpt,
        },
        PlanOp::Create {
            role: PartitionRole::Esp,
            start_bytes: esp_start,
            size_bytes: esp_bytes,
            kind: PartitionKind::Esp,
            fs: FormatFs::Fat32,
        },
        PlanOp::Create {
            role: PartitionRole::Root,
            start_bytes: root_start,
            size_bytes: root_size,
            kind: PartitionKind::LinuxRoot,
            fs: FormatFs::Ext4,
        },
    ];
    if swap_bytes > 0 {
        ops.push(PlanOp::Create {
            role: PartitionRole::Swap,
            start_bytes: root_start + root_size,
            size_bytes: swap_bytes,
            kind: PartitionKind::LinuxSwap,
            fs: FormatFs::LinuxSwap,
        });
    }

    let erases: Vec<String> = input.partitions.iter().map(|p| p.path.clone()).collect();
    let preview = build_preview(input, &ops, &erases);

    Ok(PartitionPlan {
        ops,
        preview,
        erases,
    })
}

fn plan_alongside_windows(input: &PlanInput) -> Result<PartitionPlan, PlanError> {
    let (ntfs_path, probe) = input.ntfs.as_ref().ok_or(PlanError::NoWindows)?;
    if let Some(blocker) = probe.blockers.first() {
        return Err(PlanError::NtfsBlocked(*blocker));
    }

    let ntfs_partition = input
        .partitions
        .iter()
        .find(|p| &p.path == ntfs_path)
        .ok_or(PlanError::NoWindows)?;

    let swap_bytes = align_up(swap_bytes_for(input));
    let esp_bytes = if input.uefi { NEW_ESP_BYTES } else { 0 };

    let bounds = shrink_bounds(ntfs_partition.size_bytes, probe, swap_bytes, input.uefi)
        .ok_or(PlanError::ShrinkTooSmall)?;
    let shrink_to = align_up(
        input
            .cfg
            .shrink_to_bytes
            .unwrap_or(bounds.min)
            .max(bounds.min),
    );

    let mut ops = vec![PlanOp::ShrinkNtfs {
        path: ntfs_path.clone(),
        new_size_bytes: shrink_to,
    }];

    // Bounded at both ends by the NTFS partition's own extent, not derived
    // from `freed = ntfs.size - shrink_to`: if `ntfs_partition.start_bytes`
    // isn't 1 MiB-aligned, that formula would let ESP+root+swap spill past
    // `ntfs_end` into whatever partition follows (e.g. WinRE).
    let ntfs_end = ntfs_partition.start_bytes + ntfs_partition.size_bytes;
    let region_start = align_up(ntfs_partition.start_bytes + shrink_to);
    let region_end = align_down(ntfs_end);
    let available = region_end.saturating_sub(region_start);
    let root_size = align_down(
        available
            .saturating_sub(swap_bytes)
            .saturating_sub(esp_bytes),
    );
    if root_size < MIN_LINUX_BYTES {
        return Err(PlanError::ShrinkTooSmall);
    }

    let mut cursor = region_start;
    if input.uefi {
        ops.push(PlanOp::Create {
            role: PartitionRole::Esp,
            start_bytes: cursor,
            size_bytes: esp_bytes,
            kind: PartitionKind::Esp,
            fs: FormatFs::Fat32,
        });
        cursor += esp_bytes;
    }

    let root_start = cursor;
    ops.push(PlanOp::Create {
        role: PartitionRole::Root,
        start_bytes: root_start,
        size_bytes: root_size,
        kind: PartitionKind::LinuxRoot,
        fs: FormatFs::Ext4,
    });
    if swap_bytes > 0 {
        ops.push(PlanOp::Create {
            role: PartitionRole::Swap,
            start_bytes: root_start + root_size,
            size_bytes: swap_bytes,
            kind: PartitionKind::LinuxSwap,
            fs: FormatFs::LinuxSwap,
        });
    }

    let erases = Vec::new();
    let preview = build_preview(input, &ops, &erases);

    Ok(PartitionPlan {
        ops,
        preview,
        erases,
    })
}

fn plan_free_space(input: &PlanInput) -> Result<PartitionPlan, PlanError> {
    if input.uefi && input.disk.table == TableKind::Dos {
        return Err(PlanError::MbrNeedsConversion);
    }

    let gap_id = input
        .cfg
        .selected_free_space_id
        .as_deref()
        .ok_or(PlanError::NoFreeSpace)?;
    let gap = input
        .gaps
        .iter()
        .find(|g| g.id == gap_id)
        .ok_or(PlanError::NoFreeSpace)?;

    let swap_bytes = align_up(swap_bytes_for(input));
    let esp_bytes = if input.uefi { NEW_ESP_BYTES } else { 0 };

    let needed = esp_bytes + swap_bytes + MIN_LINUX_BYTES;
    if gap.size_bytes < needed {
        return Err(PlanError::GapTooSmall);
    }

    let mut ops = Vec::new();
    let mut cursor = gap.start_bytes;

    if input.uefi {
        ops.push(PlanOp::Create {
            role: PartitionRole::Esp,
            start_bytes: cursor,
            size_bytes: esp_bytes,
            kind: PartitionKind::Esp,
            fs: FormatFs::Fat32,
        });
        cursor = align_up(cursor + esp_bytes);
    }

    let root_size = align_down(gap.start_bytes + gap.size_bytes - cursor - swap_bytes);
    if root_size < MIN_LINUX_BYTES {
        return Err(PlanError::GapTooSmall);
    }
    ops.push(PlanOp::Create {
        role: PartitionRole::Root,
        start_bytes: cursor,
        size_bytes: root_size,
        kind: PartitionKind::LinuxRoot,
        fs: FormatFs::Ext4,
    });
    cursor += root_size;

    if swap_bytes > 0 {
        ops.push(PlanOp::Create {
            role: PartitionRole::Swap,
            start_bytes: cursor,
            size_bytes: swap_bytes,
            kind: PartitionKind::LinuxSwap,
            fs: FormatFs::LinuxSwap,
        });
    }

    let erases = Vec::new();
    let preview = build_preview(input, &ops, &erases);

    Ok(PartitionPlan {
        ops,
        preview,
        erases,
    })
}

/// `Manual` mode entry — an existing partition assigned to a mount point.
/// Resizing/deleting an *existing* partition is still delegated to GParted
/// (see step 7's manual page); this struct only ever produces
/// [`PlanOp::UseExisting`] ops. Carving a *new* partition out of free space
/// is [`ManualNew`] instead, which produces [`PlanOp::Create`].
#[derive(Debug, Clone, PartialEq)]
pub struct ManualEntry {
    pub path: String,
    pub mount_point: ManualMountPoint,
    pub format: bool,
    pub fs: FormatFs,
}

/// A partition the user asked to carve out of a still-free gap — `gap_id`
/// matches a [`crate::backend::disk::layout::FreeGap::id`] from the same
/// `PlanInput`; see [`PlanError::StaleLayout`] for what happens when it no
/// longer does. When more than one `New` item shares a `gap_id`, their order
/// inside `PartitioningConfig::manual` is also their layout order inside
/// that gap (first item starts at the gap's start).
#[derive(Debug, Clone, PartialEq)]
pub struct ManualNew {
    pub gap_id: String,
    pub size_bytes: u64,
    pub mount_point: ManualMountPoint,
    pub fs: FormatFs,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ManualItem {
    Existing(ManualEntry),
    New(ManualNew),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualMountPoint {
    Root,
    Boot,
    Home,
    Swap,
}

fn mount_point_role(mp: ManualMountPoint) -> PartitionRole {
    match mp {
        ManualMountPoint::Root => PartitionRole::Root,
        ManualMountPoint::Boot => PartitionRole::Esp,
        ManualMountPoint::Home => PartitionRole::Home,
        ManualMountPoint::Swap => PartitionRole::Swap,
    }
}

fn mount_point_kind(mp: ManualMountPoint) -> PartitionKind {
    match mp {
        ManualMountPoint::Root => PartitionKind::LinuxRoot,
        ManualMountPoint::Boot => PartitionKind::Esp,
        ManualMountPoint::Home => PartitionKind::LinuxGeneric,
        ManualMountPoint::Swap => PartitionKind::LinuxSwap,
    }
}

/// Size a freshly created `/boot` always gets, whatever the config asked for
/// — same policy as every other mode's ESP (see [`NEW_ESP_BYTES`]).
pub const NEW_BOOT_BYTES: u64 = NEW_ESP_BYTES;

/// Filesystems selectable for `mp` in the manual-creation dialog. `Boot`/
/// `Swap` each have exactly one — locked, not just defaulted — because
/// Modulix's ESP is always FAT32 and swap is never anything else.
pub fn fs_choices(mp: ManualMountPoint) -> &'static [FormatFs] {
    match mp {
        ManualMountPoint::Root | ManualMountPoint::Home => {
            &[FormatFs::Ext4, FormatFs::Btrfs, FormatFs::Xfs]
        }
        ManualMountPoint::Boot => &[FormatFs::Fat32],
        ManualMountPoint::Swap => &[FormatFs::LinuxSwap],
    }
}

/// Floor for a newly created partition's size, `mp`-dependent — feeds the
/// manual-creation dialog's size `SpinRow` range.
pub fn min_size_bytes(mp: ManualMountPoint) -> u64 {
    match mp {
        ManualMountPoint::Root => MIN_LINUX_BYTES,
        ManualMountPoint::Boot => NEW_BOOT_BYTES,
        ManualMountPoint::Home | ManualMountPoint::Swap => crate::backend::disk::layout::ALIGNMENT,
    }
}

/// Whether `mp` must always be formatted, existing or new. `/`, `/boot`, and
/// swap need a filesystem Modulix controls to even boot; only `/home` is
/// legitimately reusable as-is (an existing Linux `/home` from a previous
/// install).
pub fn format_is_forced(mp: ManualMountPoint) -> bool {
    !matches!(mp, ManualMountPoint::Home)
}

fn plan_manual(input: &PlanInput) -> Result<PartitionPlan, PlanError> {
    // A path can go stale between step 7 loading the disk and the user
    // clicking "Next" (e.g. deleted in GParted since) — ignored rather than
    // failing outright; if it was the root, `root_size` stays `None` below
    // and `NoRoot` is the error the user sees, which is the honest one.
    let mut existing: Vec<&ManualEntry> = Vec::new();
    let mut new_items: Vec<&ManualNew> = Vec::new();
    for item in &input.cfg.manual {
        match item {
            ManualItem::Existing(e) if input.partitions.iter().any(|p| p.path == e.path) => {
                existing.push(e);
            }
            ManualItem::Existing(_) => {}
            ManualItem::New(n) => new_items.push(n),
        }
    }

    let root_count = existing
        .iter()
        .filter(|e| e.mount_point == ManualMountPoint::Root)
        .count()
        + new_items
            .iter()
            .filter(|n| n.mount_point == ManualMountPoint::Root)
            .count();
    if root_count > 1 {
        return Err(PlanError::MultipleRoots);
    }
    for mp in [
        ManualMountPoint::Boot,
        ManualMountPoint::Home,
        ManualMountPoint::Swap,
    ] {
        let count = existing.iter().filter(|e| e.mount_point == mp).count()
            + new_items.iter().filter(|n| n.mount_point == mp).count();
        if count > 1 {
            return Err(PlanError::DuplicateMountPoint);
        }
    }

    if input.uefi && input.disk.table == TableKind::Dos && !new_items.is_empty() {
        return Err(PlanError::MbrNeedsConversion);
    }

    let mut ops = Vec::new();
    let mut erases = Vec::new();
    let mut root_size = None;
    let mut has_esp = false;

    for entry in &existing {
        let part = input
            .partitions
            .iter()
            .find(|p| p.path == entry.path)
            .expect("filtered to only existing paths above");
        let format = entry.format || format_is_forced(entry.mount_point);

        match entry.mount_point {
            ManualMountPoint::Root => root_size = Some(part.size_bytes),
            // Always reformatted as FAT32 if picked (`format_is_forced`),
            // so the only thing left to check is that it's big enough.
            ManualMountPoint::Boot if part.size_bytes >= MIN_ESP_BYTES => {
                has_esp = true;
            }
            ManualMountPoint::Swap
                if input.cfg.swap_mode == SwapMode::Hibernation
                    && part.size_bytes < input.ram_bytes =>
            {
                return Err(PlanError::SwapTooSmallForHibernation);
            }
            _ => {}
        }

        if format {
            erases.push(entry.path.clone());
        }
        ops.push(PlanOp::UseExisting {
            path: entry.path.clone(),
            role: mount_point_role(entry.mount_point),
            format: format.then_some(entry.fs),
        });
    }

    // Cumulative offset per gap — later items sharing a `gap_id` stack after
    // earlier ones, in the order they appear in `input.cfg.manual`.
    let mut cursors: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
    for new in &new_items {
        let gap = input
            .gaps
            .iter()
            .find(|g| g.id == new.gap_id)
            .ok_or(PlanError::StaleLayout)?;
        let cursor = *cursors
            .entry(new.gap_id.as_str())
            .or_insert(gap.start_bytes);
        let size = if new.mount_point == ManualMountPoint::Boot {
            NEW_BOOT_BYTES
        } else {
            align_up(new.size_bytes)
        };
        let end = cursor + size;
        if end > gap.start_bytes + gap.size_bytes {
            return Err(PlanError::GapTooSmall);
        }

        match new.mount_point {
            ManualMountPoint::Root => root_size = Some(size),
            ManualMountPoint::Boot => has_esp = true,
            ManualMountPoint::Swap
                if input.cfg.swap_mode == SwapMode::Hibernation && size < input.ram_bytes =>
            {
                return Err(PlanError::SwapTooSmallForHibernation);
            }
            _ => {}
        }

        ops.push(PlanOp::Create {
            role: mount_point_role(new.mount_point),
            start_bytes: cursor,
            size_bytes: size,
            kind: mount_point_kind(new.mount_point),
            fs: new.fs,
        });
        cursors.insert(new.gap_id.as_str(), end);
    }

    // Only reachable on a blank disk (nothing pre-existing to destroy), and
    // only worth writing if there's actually something to create into it.
    if input.disk.table == TableKind::None && !new_items.is_empty() {
        ops.insert(
            0,
            PlanOp::CreateTable {
                disk: input.disk.path.clone(),
                table: TableKind::Gpt,
            },
        );
    }

    let root_size = root_size.ok_or(PlanError::NoRoot)?;
    if root_size < MIN_LINUX_BYTES {
        return Err(PlanError::DiskTooSmall {
            needed: MIN_LINUX_BYTES,
            available: root_size,
        });
    }
    if input.uefi && !has_esp {
        return Err(PlanError::NoEsp);
    }

    let preview = build_preview(input, &ops, &erases);

    Ok(PartitionPlan {
        ops,
        preview,
        erases,
    })
}

/// Reconstructs the disk's full layout after `ops` runs, sorted left to
/// right, from `input.partitions` plus whatever `ops` changes — then pads out
/// every gap left uncovered (between segments, and from the last segment to
/// `disk.size_bytes - GPT_TAIL_RESERVE`) with `PreviewRole::Free` segments, so
/// the preview always covers the *whole* disk. That's what makes the
/// "before"/"after" bars proportionally comparable: `DiskBar` normalizes
/// widths on the sum of segments it's handed, so a preview missing its free
/// space would stretch to fill the bar instead of showing it to scale.
fn build_preview(input: &PlanInput, ops: &[PlanOp], erases: &[String]) -> Vec<PreviewSegment> {
    struct Seg {
        path: Option<String>,
        start: u64,
        size: u64,
        label: String,
        role: PreviewRole,
        fs_type: Option<String>,
        used_bytes: Option<u64>,
    }

    let mut segs: Vec<Seg> = input
        .partitions
        .iter()
        .filter(|p| !erases.contains(&p.path))
        .map(|p| Seg {
            path: Some(p.path.clone()),
            start: p.start_bytes,
            size: p.size_bytes,
            label: p
                .label
                .clone()
                .unwrap_or_else(|| short_device_name(&p.path).to_string()),
            role: PreviewRole::Keep,
            fs_type: p.fs_type.clone(),
            used_bytes: p.used_bytes,
        })
        .collect();

    for op in ops {
        match op {
            PlanOp::ShrinkNtfs {
                path,
                new_size_bytes,
            } => {
                if let Some(seg) = segs.iter_mut().find(|s| s.path.as_deref() == Some(path)) {
                    seg.size = *new_size_bytes;
                }
            }
            PlanOp::UseExisting { path, role, format } => {
                if let Some(seg) = segs.iter_mut().find(|s| s.path.as_deref() == Some(path)) {
                    seg.role = (*role).into();
                    if let Some(fs) = format {
                        seg.fs_type = Some(fs.as_mkfs_type().to_string());
                        seg.label = role_label(*role);
                    }
                }
            }
            PlanOp::Create {
                role,
                start_bytes,
                size_bytes,
                fs,
                ..
            } => {
                segs.push(Seg {
                    path: None,
                    start: *start_bytes,
                    size: *size_bytes,
                    label: role_label(*role),
                    role: (*role).into(),
                    fs_type: Some(fs.as_mkfs_type().to_string()),
                    used_bytes: None,
                });
            }
            PlanOp::CreateTable { .. } => {}
        }
    }

    segs.sort_by_key(|s| s.start);

    let synthetic: Vec<PartitionInfo> = segs
        .iter()
        .map(|s| PartitionInfo {
            path: s.path.clone().unwrap_or_default(),
            disk_path: input.disk.path.clone(),
            fs_type: None,
            label: None,
            size_bytes: s.size,
            start_bytes: s.start,
            type_guid: None,
            uuid: None,
            is_esp: false,
            used_bytes: None,
        })
        .collect();
    for gap in crate::backend::disk::layout::free_gaps(input.disk.size_bytes, &synthetic) {
        segs.push(Seg {
            path: None,
            start: gap.start_bytes,
            size: gap.size_bytes,
            label: tr("Free space"),
            role: PreviewRole::Free,
            fs_type: None,
            used_bytes: None,
        });
    }
    segs.sort_by_key(|s| s.start);

    segs.into_iter()
        .map(|s| {
            let encrypted = s.role == PreviewRole::Root && input.cfg.encryption_enabled;
            let used_bytes = (s.role == PreviewRole::Keep)
                .then_some(s.used_bytes)
                .flatten();
            PreviewSegment {
                label: s.label,
                size_bytes: s.size,
                start_bytes: s.start,
                role: s.role,
                fs_type: s.fs_type,
                encrypted,
                used_bytes,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::disk::PartitionKind as PK;

    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;

    fn disk(size_bytes: u64, table: TableKind, is_removable: bool, has_windows: bool) -> DiskInfo {
        DiskInfo {
            path: "/dev/fake0".into(),
            model: "Test disk".into(),
            size_bytes,
            is_removable,
            has_windows,
            table,
        }
    }

    fn part(
        path: &str,
        start: u64,
        size: u64,
        kind: Option<PK>,
        fs: Option<&str>,
    ) -> PartitionInfo {
        PartitionInfo {
            path: path.into(),
            disk_path: "/dev/fake0".into(),
            fs_type: fs.map(str::to_string),
            label: None,
            size_bytes: size,
            start_bytes: start,
            type_guid: kind.map(|k| k.type_guid().to_string()),
            uuid: None,
            is_esp: matches!(kind, Some(PK::Esp)),
            used_bytes: None,
        }
    }

    fn base_cfg(mode: PartitionMode) -> PartitioningConfig {
        PartitioningConfig {
            mode,
            target_disk: Some("/dev/fake0".into()),
            swap_mode: SwapMode::None,
            ..Default::default()
        }
    }

    fn base_input(
        disk_info: DiskInfo,
        partitions: Vec<PartitionInfo>,
        cfg: PartitioningConfig,
    ) -> PlanInput {
        let gaps = crate::backend::disk::layout::free_gaps(disk_info.size_bytes, &partitions);
        PlanInput {
            disk: disk_info,
            partitions,
            gaps,
            ntfs: None,
            cfg,
            ram_bytes: 8 * GIB,
            uefi: true,
        }
    }

    #[test]
    fn entire_disk_happy_path() {
        let input = base_input(
            disk(250 * GIB, TableKind::None, false, false),
            vec![],
            base_cfg(PartitionMode::EntireDisk),
        );
        let result = plan(&input).unwrap();
        assert!(matches!(result.ops[0], PlanOp::CreateTable { .. }));
        assert_eq!(result.preview.len(), 3); // head Free gap (1 MiB alignment slack) + ESP + Root
        assert!(result.erases.is_empty());
    }

    #[test]
    fn entire_disk_too_small() {
        let input = base_input(
            disk(GIB, TableKind::None, false, false),
            vec![],
            base_cfg(PartitionMode::EntireDisk),
        );
        assert!(matches!(plan(&input), Err(PlanError::DiskTooSmall { .. })));
    }

    #[test]
    fn entire_disk_erases_existing_partitions() {
        let input = base_input(
            disk(250 * GIB, TableKind::Gpt, false, false),
            vec![part(
                "/dev/fake0p1",
                MIB,
                512 * MIB,
                Some(PK::Esp),
                Some("vfat"),
            )],
            base_cfg(PartitionMode::EntireDisk),
        );
        let result = plan(&input).unwrap();
        assert_eq!(result.erases, vec!["/dev/fake0p1".to_string()]);
    }

    #[test]
    fn entire_disk_with_hibernation_swap() {
        let mut cfg = base_cfg(PartitionMode::EntireDisk);
        cfg.swap_mode = SwapMode::Hibernation;
        let input = base_input(disk(250 * GIB, TableKind::None, false, false), vec![], cfg);
        let result = plan(&input).unwrap();
        assert_eq!(result.preview.len(), 4); // head Free gap + ESP + Root + Swap
        assert!(result.preview.iter().any(|s| s.role == PreviewRole::Swap));
    }

    #[test]
    fn validate_config_rejects_empty_passphrase_when_encryption_enabled() {
        let mut cfg = base_cfg(PartitionMode::EntireDisk);
        cfg.encryption_enabled = true;
        assert_eq!(
            validate_config(&cfg, true),
            Err(PlanError::MissingPassphrase)
        );
    }

    #[test]
    fn validate_config_accepts_empty_passphrase_when_encryption_disabled() {
        let cfg = base_cfg(PartitionMode::EntireDisk);
        assert_eq!(validate_config(&cfg, false), Ok(()));
    }

    #[test]
    fn validate_config_rejects_unconfirmed_passphrase() {
        let mut cfg = base_cfg(PartitionMode::EntireDisk);
        cfg.encryption_enabled = true;
        cfg.encryption_passphrase = "hunter2".into();
        assert_eq!(
            validate_config(&cfg, false),
            Err(PlanError::PassphraseMismatch)
        );
    }

    #[test]
    fn validate_config_accepts_confirmed_passphrase() {
        let mut cfg = base_cfg(PartitionMode::EntireDisk);
        cfg.encryption_enabled = true;
        cfg.encryption_passphrase = "hunter2".into();
        assert_eq!(validate_config(&cfg, true), Ok(()));
    }

    #[test]
    fn plan_stays_populated_with_encryption_enabled_and_no_passphrase() {
        // Regression test: `plan()` must never blank the preview just because
        // the passphrase is missing — that's `validate_config`'s job, not
        // the partitioning planner's. See `steps/partitioning.rs`'s
        // `build_revalidate`, which paints `after_bar` from this even while
        // `validate_config` blocks "Next".
        let mut cfg = base_cfg(PartitionMode::EntireDisk);
        cfg.encryption_enabled = true;
        let input = base_input(disk(250 * GIB, TableKind::None, false, false), vec![], cfg);
        let result = plan(&input).unwrap();
        assert!(!result.preview.is_empty());
        let root = result
            .preview
            .iter()
            .find(|s| s.role == PreviewRole::Root)
            .expect("root segment");
        assert!(root.encrypted);
    }

    #[test]
    fn preview_covers_the_whole_disk_and_is_sorted() {
        let input = base_input(
            disk(250 * GIB, TableKind::None, false, false),
            vec![],
            base_cfg(PartitionMode::EntireDisk),
        );
        let result = plan(&input).unwrap();
        let total: u64 = result.preview.iter().map(|s| s.size_bytes).sum();
        assert_eq!(
            total,
            250 * GIB - crate::backend::disk::layout::GPT_TAIL_RESERVE
        );
        let starts: Vec<u64> = result.preview.iter().map(|s| s.start_bytes).collect();
        let mut sorted = starts.clone();
        sorted.sort();
        assert_eq!(starts, sorted);
        for w in starts.windows(2) {
            assert!(w[0] < w[1]);
        }
    }

    #[test]
    fn no_disk_when_disk_is_zero_sized() {
        let input = base_input(
            disk(0, TableKind::None, false, false),
            vec![],
            base_cfg(PartitionMode::EntireDisk),
        );
        assert_eq!(plan(&input), Err(PlanError::NoDisk));
    }

    fn windows_partitions() -> Vec<PartitionInfo> {
        vec![
            part("/dev/fake0p1", MIB, 100 * MIB, Some(PK::Esp), Some("vfat")),
            part("/dev/fake0p2", 101 * MIB, 16 * MIB, None, None),
            part("/dev/fake0p3", 117 * MIB, 400 * GIB, None, Some("ntfs")),
        ]
    }

    #[test]
    fn alongside_windows_happy_path() {
        let mut cfg = base_cfg(PartitionMode::AlongsideWindows);
        cfg.shrink_to_bytes = Some(210 * GIB);
        let mut input = base_input(
            disk(420 * GIB, TableKind::Gpt, false, true),
            windows_partitions(),
            cfg,
        );
        input.ntfs = Some((
            "/dev/fake0p3".into(),
            NtfsProbe {
                min_size_bytes: 210 * GIB,
                current_size_bytes: 400 * GIB,
                blockers: vec![],
                used_bytes: None,
            },
        ));
        let result = plan(&input).unwrap();
        assert!(matches!(result.ops[0], PlanOp::ShrinkNtfs { .. }));
        assert!(result.erases.is_empty());
    }

    #[test]
    fn alongside_windows_without_windows_errors() {
        let cfg = base_cfg(PartitionMode::AlongsideWindows);
        let input = base_input(disk(250 * GIB, TableKind::Gpt, false, false), vec![], cfg);
        assert_eq!(plan(&input), Err(PlanError::NoWindows));
    }

    #[test]
    fn alongside_windows_bitlocker_blocks() {
        let cfg = base_cfg(PartitionMode::AlongsideWindows);
        let mut input = base_input(
            disk(420 * GIB, TableKind::Gpt, false, true),
            windows_partitions(),
            cfg,
        );
        input.ntfs = Some((
            "/dev/fake0p3".into(),
            NtfsProbe {
                min_size_bytes: 0,
                current_size_bytes: 400 * GIB,
                blockers: vec![NtfsBlocker::BitLocker],
                used_bytes: None,
            },
        ));
        assert_eq!(
            plan(&input),
            Err(PlanError::NtfsBlocked(NtfsBlocker::BitLocker))
        );
    }

    #[test]
    fn alongside_windows_shrink_too_small() {
        let mut cfg = base_cfg(PartitionMode::AlongsideWindows);
        cfg.shrink_to_bytes = Some(395 * GIB);
        let mut input = base_input(
            disk(420 * GIB, TableKind::Gpt, false, true),
            windows_partitions(),
            cfg,
        );
        input.ntfs = Some((
            "/dev/fake0p3".into(),
            NtfsProbe {
                min_size_bytes: 395 * GIB,
                current_size_bytes: 400 * GIB,
                blockers: vec![],
                used_bytes: None,
            },
        ));
        assert_eq!(plan(&input), Err(PlanError::ShrinkTooSmall));
    }

    #[test]
    fn shrink_bounds_none_when_windows_is_nearly_full() {
        // Mirrors `DiskScenario::WindowsFull`: min resize size (395 GiB) too
        // close to the volume's own size (400 GiB) to leave room for ESP +
        // `MIN_LINUX_BYTES` — no slider position should ever be offered.
        let probe = NtfsProbe {
            min_size_bytes: 395 * GIB,
            current_size_bytes: 400 * GIB,
            blockers: vec![],
            used_bytes: None,
        };
        assert_eq!(shrink_bounds(400 * GIB, &probe, 0, true), None);
    }

    #[test]
    fn shrink_bounds_some_with_max_above_min() {
        let probe = NtfsProbe {
            min_size_bytes: 210 * GIB,
            current_size_bytes: 400 * GIB,
            blockers: vec![],
            used_bytes: None,
        };
        let bounds = shrink_bounds(400 * GIB, &probe, 0, true).expect("shrinkable");
        assert!(bounds.max > bounds.min);
        assert_eq!(bounds.min, 210 * GIB);
    }

    #[test]
    fn shrink_bounds_hibernation_swap_lowers_max_versus_no_swap() {
        let probe = NtfsProbe {
            min_size_bytes: 210 * GIB,
            current_size_bytes: 400 * GIB,
            blockers: vec![],
            used_bytes: None,
        };
        let no_swap = shrink_bounds(400 * GIB, &probe, 0, true).unwrap();
        let hibernation_swap_bytes = align_up(compute_swap_bytes(SwapMode::Hibernation, 16 * GIB));
        let with_swap = shrink_bounds(400 * GIB, &probe, hibernation_swap_bytes, true).unwrap();
        assert!(with_swap.max < no_swap.max);
    }

    #[test]
    fn alongside_windows_any_position_in_shrink_bounds_plans_ok() {
        // Regression for the "colored bar empties near the end of the
        // slider's course" symptom: every position `shrink_bounds` reports
        // as valid must make `plan()` succeed, for every swap mode.
        for swap_mode in [SwapMode::None, SwapMode::Standard, SwapMode::Hibernation] {
            let mut cfg = base_cfg(PartitionMode::AlongsideWindows);
            cfg.swap_mode = swap_mode;
            let mut input = base_input(
                disk(420 * GIB, TableKind::Gpt, false, true),
                windows_partitions(),
                cfg,
            );
            let probe = NtfsProbe {
                min_size_bytes: 210 * GIB,
                current_size_bytes: 400 * GIB,
                blockers: vec![],
                used_bytes: None,
            };
            input.ntfs = Some(("/dev/fake0p3".into(), probe.clone()));

            let swap_bytes = align_up(compute_swap_bytes(swap_mode, input.ram_bytes));
            let bounds = shrink_bounds(400 * GIB, &probe, swap_bytes, input.uefi)
                .expect("shrinkable for every swap mode in this fixture");

            for shrink_to in [bounds.min, (bounds.min + bounds.max) / 2, bounds.max] {
                input.cfg.shrink_to_bytes = Some(shrink_to);
                let result = plan(&input);
                assert!(
                    result.is_ok(),
                    "swap_mode={swap_mode:?} shrink_to={shrink_to} failed: {result:?}"
                );
            }
        }
    }

    #[test]
    fn alongside_windows_stays_within_ntfs_extent_when_start_is_misaligned() {
        // `ntfs_start` deliberately 4 KiB short of 1 MiB alignment — with the
        // old `freed = ntfs.size - shrink_to` formula, ESP + root (+ swap)
        // would spill up to 1 MiB past `ntfs_end`, onto whatever partition
        // follows (WinRE here).
        let ntfs_start = 117 * MIB + 4096;
        let ntfs_size = 400 * GIB;
        let ntfs_end = ntfs_start + ntfs_size;
        let partitions = vec![
            part("/dev/fake0p1", MIB, 100 * MIB, Some(PK::Esp), Some("vfat")),
            part("/dev/fake0p2", 101 * MIB, 16 * MIB, None, None),
            part("/dev/fake0p3", ntfs_start, ntfs_size, None, Some("ntfs")),
            part("/dev/fake0p4", ntfs_end, 700 * MIB, None, Some("ntfs")),
        ];
        let mut cfg = base_cfg(PartitionMode::AlongsideWindows);
        cfg.shrink_to_bytes = Some(210 * GIB);
        let mut input = base_input(
            disk(420 * GIB, TableKind::Gpt, false, true),
            partitions,
            cfg,
        );
        input.ntfs = Some((
            "/dev/fake0p3".into(),
            NtfsProbe {
                min_size_bytes: 210 * GIB,
                current_size_bytes: ntfs_size,
                blockers: vec![],
                used_bytes: None,
            },
        ));
        let result = plan(&input).unwrap();
        for op in &result.ops {
            if let PlanOp::Create {
                start_bytes,
                size_bytes,
                ..
            } = op
            {
                assert!(
                    start_bytes + size_bytes <= ntfs_end,
                    "op ends at {} past ntfs_end {ntfs_end}",
                    start_bytes + size_bytes
                );
            }
        }
    }

    #[test]
    fn free_space_happy_path() {
        let mut cfg = base_cfg(PartitionMode::FreeSpace);
        let partitions = vec![
            part("/dev/fake0p1", MIB, 512 * MIB, Some(PK::Esp), Some("vfat")),
            part("/dev/fake0p2", 513 * MIB, 200 * GIB, None, Some("ntfs")),
        ];
        let disk_info = disk(320 * GIB, TableKind::Gpt, false, true);
        let gaps = crate::backend::disk::layout::free_gaps(disk_info.size_bytes, &partitions);
        let largest_gap = gaps.iter().max_by_key(|g| g.size_bytes).unwrap();
        cfg.selected_free_space_id = Some(largest_gap.id.clone());
        let input = PlanInput {
            disk: disk_info,
            partitions,
            gaps,
            ntfs: None,
            cfg,
            ram_bytes: 8 * GIB,
            uefi: true,
        };
        let result = plan(&input).unwrap();
        assert!(result.erases.is_empty());
        assert!(result.ops.iter().any(|op| matches!(
            op,
            PlanOp::Create {
                role: PartitionRole::Esp,
                ..
            }
        )));
    }

    #[test]
    fn free_space_no_selection_errors() {
        let input = base_input(
            disk(250 * GIB, TableKind::None, false, false),
            vec![],
            base_cfg(PartitionMode::FreeSpace),
        );
        assert_eq!(plan(&input), Err(PlanError::NoFreeSpace));
    }

    #[test]
    fn free_space_on_mbr_needs_conversion() {
        let mut cfg = base_cfg(PartitionMode::FreeSpace);
        cfg.selected_free_space_id = Some("0+1000".into());
        let input = base_input(disk(250 * GIB, TableKind::Dos, false, false), vec![], cfg);
        assert_eq!(plan(&input), Err(PlanError::MbrNeedsConversion));
    }

    fn manual_partitions() -> Vec<PartitionInfo> {
        vec![
            part("/dev/fake0p1", MIB, 512 * MIB, Some(PK::Esp), Some("vfat")),
            part("/dev/fake0p2", 513 * MIB, 40 * GIB, None, None),
        ]
    }

    fn manual_root_and_boot() -> Vec<ManualItem> {
        vec![
            ManualItem::Existing(ManualEntry {
                path: "/dev/fake0p1".into(),
                mount_point: ManualMountPoint::Boot,
                format: false,
                fs: FormatFs::Fat32,
            }),
            ManualItem::Existing(ManualEntry {
                path: "/dev/fake0p2".into(),
                mount_point: ManualMountPoint::Root,
                format: true,
                fs: FormatFs::Ext4,
            }),
        ]
    }

    #[test]
    fn manual_happy_path() {
        let mut cfg = base_cfg(PartitionMode::Manual);
        cfg.manual = manual_root_and_boot();
        let input = base_input(
            disk(250 * GIB, TableKind::Gpt, false, false),
            manual_partitions(),
            cfg,
        );
        let result = plan(&input).unwrap();
        assert_eq!(result.ops.len(), 2);
    }

    #[test]
    fn manual_no_root_errors() {
        let mut cfg = base_cfg(PartitionMode::Manual);
        cfg.manual = vec![manual_root_and_boot().remove(0)]; // boot only
        let input = base_input(
            disk(250 * GIB, TableKind::Gpt, false, false),
            manual_partitions(),
            cfg,
        );
        assert_eq!(plan(&input), Err(PlanError::NoRoot));
    }

    #[test]
    fn manual_multiple_roots_errors() {
        let mut cfg = base_cfg(PartitionMode::Manual);
        let mut entries = manual_root_and_boot();
        entries.push(ManualItem::Existing(ManualEntry {
            path: "/dev/fake0p3".into(),
            mount_point: ManualMountPoint::Root,
            format: true,
            fs: FormatFs::Ext4,
        }));
        cfg.manual = entries;
        let mut partitions = manual_partitions();
        partitions.push(part("/dev/fake0p3", 41 * GIB, 30 * GIB, None, None));
        let input = base_input(
            disk(250 * GIB, TableKind::Gpt, false, false),
            partitions,
            cfg,
        );
        assert_eq!(plan(&input), Err(PlanError::MultipleRoots));
    }

    #[test]
    fn manual_duplicate_home_errors() {
        let mut cfg = base_cfg(PartitionMode::Manual);
        let mut entries = manual_root_and_boot();
        entries.push(ManualItem::Existing(ManualEntry {
            path: "/dev/fake0p3".into(),
            mount_point: ManualMountPoint::Home,
            format: false,
            fs: FormatFs::Ext4,
        }));
        entries.push(ManualItem::Existing(ManualEntry {
            path: "/dev/fake0p4".into(),
            mount_point: ManualMountPoint::Home,
            format: false,
            fs: FormatFs::Ext4,
        }));
        cfg.manual = entries;
        let mut partitions = manual_partitions();
        partitions.push(part("/dev/fake0p3", 41 * GIB, 30 * GIB, None, Some("ext4")));
        partitions.push(part("/dev/fake0p4", 71 * GIB, 30 * GIB, None, Some("ext4")));
        let input = base_input(
            disk(250 * GIB, TableKind::Gpt, false, false),
            partitions,
            cfg,
        );
        assert_eq!(plan(&input), Err(PlanError::DuplicateMountPoint));
    }

    #[test]
    fn manual_swap_too_small_for_hibernation() {
        let mut cfg = base_cfg(PartitionMode::Manual);
        cfg.swap_mode = SwapMode::Hibernation;
        let mut entries = manual_root_and_boot();
        entries.push(ManualItem::Existing(ManualEntry {
            path: "/dev/fake0p3".into(),
            mount_point: ManualMountPoint::Swap,
            format: true,
            fs: FormatFs::LinuxSwap,
        }));
        cfg.manual = entries;
        let mut partitions = manual_partitions();
        partitions.push(part("/dev/fake0p3", 200 * GIB, GIB, None, Some("swap")));
        let input = base_input(
            disk(250 * GIB, TableKind::Gpt, false, false),
            partitions,
            cfg,
        );
        assert_eq!(plan(&input), Err(PlanError::SwapTooSmallForHibernation));
    }

    #[test]
    fn manual_root_too_small_errors() {
        let mut cfg = base_cfg(PartitionMode::Manual);
        cfg.manual = manual_root_and_boot();
        let partitions = vec![
            part("/dev/fake0p1", MIB, 512 * MIB, Some(PK::Esp), Some("vfat")),
            part("/dev/fake0p2", 513 * MIB, GIB, None, None), // under MIN_LINUX_BYTES
        ];
        let input = base_input(
            disk(250 * GIB, TableKind::Gpt, false, false),
            partitions,
            cfg,
        );
        assert!(matches!(plan(&input), Err(PlanError::DiskTooSmall { .. })));
    }

    #[test]
    fn manual_boot_on_non_esp_big_enough_is_reformatted_and_succeeds() {
        let mut cfg = base_cfg(PartitionMode::Manual);
        cfg.manual = manual_root_and_boot();
        // Plain NTFS, not a real ESP — but big enough, and always
        // reformatted as FAT32 regardless (`format_is_forced`), so it still
        // satisfies the UEFI ESP requirement.
        let partitions = vec![
            part("/dev/fake0p1", MIB, 512 * MIB, None, Some("ntfs")),
            part("/dev/fake0p2", 513 * MIB, 40 * GIB, None, None),
        ];
        let input = base_input(
            disk(250 * GIB, TableKind::Gpt, false, false),
            partitions,
            cfg,
        );
        let result = plan(&input).unwrap();
        assert!(result.erases.contains(&"/dev/fake0p1".to_string()));
    }

    #[test]
    fn manual_boot_too_small_for_esp_errors() {
        let mut cfg = base_cfg(PartitionMode::Manual);
        cfg.manual = manual_root_and_boot();
        let partitions = vec![
            part("/dev/fake0p1", MIB, 50 * MIB, Some(PK::Esp), Some("vfat")), // under MIN_ESP_BYTES
            part("/dev/fake0p2", 52 * MIB, 40 * GIB, None, None),
        ];
        let input = base_input(
            disk(250 * GIB, TableKind::Gpt, false, false),
            partitions,
            cfg,
        );
        assert_eq!(plan(&input), Err(PlanError::NoEsp));
    }

    #[test]
    fn manual_unknown_path_is_ignored() {
        let mut cfg = base_cfg(PartitionMode::Manual);
        cfg.manual = manual_root_and_boot();
        cfg.manual.push(ManualItem::Existing(ManualEntry {
            path: "/dev/fake0p9".into(), // stale — not in `partitions`
            mount_point: ManualMountPoint::Home,
            format: true,
            fs: FormatFs::Ext4,
        }));
        let input = base_input(
            disk(250 * GIB, TableKind::Gpt, false, false),
            manual_partitions(),
            cfg,
        );
        let result = plan(&input).unwrap();
        assert_eq!(result.ops.len(), 2);
    }

    #[test]
    fn manual_existing_root_format_false_still_erases() {
        let mut cfg = base_cfg(PartitionMode::Manual);
        cfg.manual = vec![
            ManualItem::Existing(ManualEntry {
                path: "/dev/fake0p1".into(),
                mount_point: ManualMountPoint::Boot,
                format: false,
                fs: FormatFs::Fat32,
            }),
            ManualItem::Existing(ManualEntry {
                path: "/dev/fake0p2".into(),
                mount_point: ManualMountPoint::Root,
                format: false,
                fs: FormatFs::Ext4,
            }),
        ];
        let input = base_input(
            disk(250 * GIB, TableKind::Gpt, false, false),
            manual_partitions(),
            cfg,
        );
        let result = plan(&input).unwrap();
        assert!(result.erases.contains(&"/dev/fake0p2".to_string()));
        assert!(result.ops.iter().any(|op| matches!(
            op,
            PlanOp::UseExisting { path, format: Some(FormatFs::Ext4), .. } if path == "/dev/fake0p2"
        )));
    }

    #[test]
    fn manual_existing_home_format_false_is_kept() {
        let mut cfg = base_cfg(PartitionMode::Manual);
        cfg.manual = vec![
            ManualItem::Existing(ManualEntry {
                path: "/dev/fake0p1".into(),
                mount_point: ManualMountPoint::Boot,
                format: false,
                fs: FormatFs::Fat32,
            }),
            ManualItem::Existing(ManualEntry {
                path: "/dev/fake0p2".into(),
                mount_point: ManualMountPoint::Root,
                format: true,
                fs: FormatFs::Ext4,
            }),
            ManualItem::Existing(ManualEntry {
                path: "/dev/fake0p3".into(),
                mount_point: ManualMountPoint::Home,
                format: false,
                fs: FormatFs::Ext4,
            }),
        ];
        let mut partitions = manual_partitions();
        partitions.push(part(
            "/dev/fake0p3",
            41 * GIB,
            100 * GIB,
            None,
            Some("ext4"),
        ));
        let input = base_input(
            disk(250 * GIB, TableKind::Gpt, false, false),
            partitions,
            cfg,
        );
        let result = plan(&input).unwrap();
        assert!(!result.erases.contains(&"/dev/fake0p3".to_string()));
        assert!(result.ops.iter().any(|op| matches!(
            op,
            PlanOp::UseExisting { path, format: None, .. } if path == "/dev/fake0p3"
        )));
    }

    #[test]
    fn manual_create_root_and_boot_on_blank_disk() {
        let disk_info = disk(250 * GIB, TableKind::None, false, false);
        let gaps = crate::backend::disk::layout::free_gaps(disk_info.size_bytes, &[]);
        let gap_id = gaps[0].id.clone();
        let mut cfg = base_cfg(PartitionMode::Manual);
        cfg.manual = vec![
            ManualItem::New(ManualNew {
                gap_id: gap_id.clone(),
                size_bytes: NEW_BOOT_BYTES,
                mount_point: ManualMountPoint::Boot,
                fs: FormatFs::Fat32,
            }),
            ManualItem::New(ManualNew {
                gap_id,
                size_bytes: 50 * GIB,
                mount_point: ManualMountPoint::Root,
                fs: FormatFs::Ext4,
            }),
        ];
        let input = PlanInput {
            disk: disk_info,
            partitions: vec![],
            gaps,
            ntfs: None,
            cfg,
            ram_bytes: 8 * GIB,
            uefi: true,
        };
        let result = plan(&input).unwrap();
        assert!(matches!(
            result.ops[0],
            PlanOp::CreateTable {
                table: TableKind::Gpt,
                ..
            }
        ));
        let boot = result
            .ops
            .iter()
            .find_map(|op| match op {
                PlanOp::Create {
                    role: PartitionRole::Esp,
                    size_bytes,
                    fs,
                    ..
                } => Some((*size_bytes, *fs)),
                _ => None,
            })
            .expect("boot create op");
        assert_eq!(boot, (NEW_BOOT_BYTES, FormatFs::Fat32));
        assert!(result.ops.iter().any(|op| matches!(
            op,
            PlanOp::Create {
                role: PartitionRole::Root,
                ..
            }
        )));
    }

    #[test]
    fn manual_two_creations_in_one_gap_are_sequential() {
        let disk_info = disk(250 * GIB, TableKind::Gpt, false, false);
        let gaps = crate::backend::disk::layout::free_gaps(disk_info.size_bytes, &[]);
        let gap_id = gaps[0].id.clone();
        let gap_start = gaps[0].start_bytes;
        let mut cfg = base_cfg(PartitionMode::Manual);
        cfg.manual = vec![
            ManualItem::New(ManualNew {
                gap_id: gap_id.clone(),
                size_bytes: MIN_LINUX_BYTES,
                mount_point: ManualMountPoint::Root,
                fs: FormatFs::Ext4,
            }),
            ManualItem::New(ManualNew {
                gap_id,
                size_bytes: 30 * GIB,
                mount_point: ManualMountPoint::Home,
                fs: FormatFs::Ext4,
            }),
        ];
        let input = PlanInput {
            disk: disk_info,
            partitions: vec![],
            gaps,
            ntfs: None,
            cfg,
            ram_bytes: 8 * GIB,
            uefi: false,
        };
        let result = plan(&input).unwrap();
        let (root_start, root_size) = result
            .ops
            .iter()
            .find_map(|op| match op {
                PlanOp::Create {
                    role: PartitionRole::Root,
                    start_bytes,
                    size_bytes,
                    ..
                } => Some((*start_bytes, *size_bytes)),
                _ => None,
            })
            .expect("root create op");
        let home_start = result
            .ops
            .iter()
            .find_map(|op| match op {
                PlanOp::Create {
                    role: PartitionRole::Home,
                    start_bytes,
                    ..
                } => Some(*start_bytes),
                _ => None,
            })
            .expect("home create op");
        assert_eq!(root_start, gap_start);
        assert_eq!(home_start, gap_start + root_size);
        assert!(home_start >= root_start + root_size);
    }

    #[test]
    fn manual_creation_exceeding_gap_errors() {
        let disk_info = disk(10 * GIB, TableKind::Gpt, false, false);
        let gaps = crate::backend::disk::layout::free_gaps(disk_info.size_bytes, &[]);
        let gap_id = gaps[0].id.clone();
        let mut cfg = base_cfg(PartitionMode::Manual);
        cfg.manual = vec![ManualItem::New(ManualNew {
            gap_id,
            size_bytes: 50 * GIB,
            mount_point: ManualMountPoint::Root,
            fs: FormatFs::Ext4,
        })];
        let input = PlanInput {
            disk: disk_info,
            partitions: vec![],
            gaps,
            ntfs: None,
            cfg,
            ram_bytes: 8 * GIB,
            uefi: false,
        };
        assert_eq!(plan(&input), Err(PlanError::GapTooSmall));
    }

    #[test]
    fn manual_unknown_gap_id_is_stale_layout() {
        let mut cfg = base_cfg(PartitionMode::Manual);
        cfg.manual = vec![ManualItem::New(ManualNew {
            gap_id: "stale".into(),
            size_bytes: MIN_LINUX_BYTES,
            mount_point: ManualMountPoint::Root,
            fs: FormatFs::Ext4,
        })];
        let input = base_input(disk(250 * GIB, TableKind::Gpt, false, false), vec![], cfg);
        assert_eq!(plan(&input), Err(PlanError::StaleLayout));
    }

    #[test]
    fn entire_disk_creates_1024_mib_esp() {
        let input = base_input(
            disk(250 * GIB, TableKind::None, false, false),
            vec![],
            base_cfg(PartitionMode::EntireDisk),
        );
        let result = plan(&input).unwrap();
        let esp = result
            .ops
            .iter()
            .find_map(|op| match op {
                PlanOp::Create {
                    role: PartitionRole::Esp,
                    size_bytes,
                    ..
                } => Some(*size_bytes),
                _ => None,
            })
            .expect("esp create op");
        assert_eq!(esp, NEW_ESP_BYTES);
    }

    #[test]
    fn free_space_creates_1024_mib_esp() {
        let mut cfg = base_cfg(PartitionMode::FreeSpace);
        let partitions = vec![
            part("/dev/fake0p1", MIB, 512 * MIB, Some(PK::Esp), Some("vfat")),
            part("/dev/fake0p2", 513 * MIB, 200 * GIB, None, Some("ntfs")),
        ];
        let disk_info = disk(320 * GIB, TableKind::Gpt, false, true);
        let gaps = crate::backend::disk::layout::free_gaps(disk_info.size_bytes, &partitions);
        let largest_gap = gaps.iter().max_by_key(|g| g.size_bytes).unwrap();
        cfg.selected_free_space_id = Some(largest_gap.id.clone());
        let input = PlanInput {
            disk: disk_info,
            partitions,
            gaps,
            ntfs: None,
            cfg,
            ram_bytes: 8 * GIB,
            uefi: true,
        };
        let result = plan(&input).unwrap();
        let esp = result
            .ops
            .iter()
            .find_map(|op| match op {
                PlanOp::Create {
                    role: PartitionRole::Esp,
                    size_bytes,
                    ..
                } => Some(*size_bytes),
                _ => None,
            })
            .expect("esp create op");
        assert_eq!(esp, NEW_ESP_BYTES);
    }

    #[test]
    fn alongside_windows_creates_1024_mib_esp() {
        let mut cfg = base_cfg(PartitionMode::AlongsideWindows);
        cfg.shrink_to_bytes = Some(210 * GIB);
        let mut input = base_input(
            disk(420 * GIB, TableKind::Gpt, false, true),
            windows_partitions(),
            cfg,
        );
        input.ntfs = Some((
            "/dev/fake0p3".into(),
            NtfsProbe {
                min_size_bytes: 210 * GIB,
                current_size_bytes: 400 * GIB,
                blockers: vec![],
                used_bytes: None,
            },
        ));
        let result = plan(&input).unwrap();
        let esp = result
            .ops
            .iter()
            .find_map(|op| match op {
                PlanOp::Create {
                    role: PartitionRole::Esp,
                    size_bytes,
                    ..
                } => Some(*size_bytes),
                _ => None,
            })
            .expect("esp create op");
        assert_eq!(esp, NEW_ESP_BYTES);
    }

    #[test]
    fn removable_disk_is_a_warning_not_an_error() {
        let input = base_input(
            disk(32 * GIB, TableKind::None, true, false),
            vec![],
            base_cfg(PartitionMode::EntireDisk),
        );
        assert!(plan(&input).is_ok());
        assert_eq!(warnings(&input), vec![PlanWarning::RemovableTarget]);
    }

    // --- Scenario × mode matrix -------------------------------------------
    //
    // Drives `plan()` off the same `DiskScenario` fixtures `--fake-disk=`
    // exposes in the UI (see `backend::disk::scenario`), so every case here
    // can also be clicked through by hand. `disk_index` picks which of a
    // scenario's disks to target (only `multi-disk` has more than one).

    use crate::backend::disk::scenario::DiskScenario;

    fn scenario_input(scenario: DiskScenario, disk_index: usize, mode: PartitionMode) -> PlanInput {
        let fixture = scenario.fixture();
        let disk_info = fixture.disks[disk_index].clone();
        let partitions: Vec<PartitionInfo> = fixture
            .partitions
            .iter()
            .filter(|p| p.disk_path == disk_info.path)
            .cloned()
            .collect();
        let gaps = crate::backend::disk::layout::free_gaps(disk_info.size_bytes, &partitions);
        let ntfs = partitions
            .iter()
            .find(|p| p.fs_type.as_deref() == Some("ntfs"))
            .map(|p| {
                let probe = fixture.ntfs.get(&p.path).cloned().unwrap_or(NtfsProbe {
                    min_size_bytes: 0,
                    current_size_bytes: 0,
                    blockers: vec![],
                    used_bytes: None,
                });
                (p.path.clone(), probe)
            });

        let mut cfg = PartitioningConfig {
            mode,
            target_disk: Some(disk_info.path.clone()),
            swap_mode: SwapMode::Standard,
            ..Default::default()
        };
        if mode == PartitionMode::AlongsideWindows
            && let Some((_, probe)) = &ntfs
        {
            cfg.shrink_to_bytes = Some(probe.min_size_bytes);
        }
        if mode == PartitionMode::FreeSpace {
            cfg.selected_free_space_id = gaps
                .iter()
                .max_by_key(|g| g.size_bytes)
                .map(|g| g.id.clone());
        }

        PlanInput {
            disk: disk_info,
            partitions,
            gaps,
            ntfs,
            cfg,
            ram_bytes: 8 * GIB,
            uefi: true,
        }
    }

    #[test]
    fn linux_entire_disk_erases_both_existing_partitions() {
        let input = scenario_input(DiskScenario::Linux, 0, PartitionMode::EntireDisk);
        let result = plan(&input).unwrap();
        assert_eq!(result.erases.len(), 2);
    }

    #[test]
    fn linux_alongside_windows_has_no_windows() {
        let input = scenario_input(DiskScenario::Linux, 0, PartitionMode::AlongsideWindows);
        assert_eq!(plan(&input), Err(PlanError::NoWindows));
    }

    #[test]
    fn linux_free_space_gap_too_small() {
        // A fully-partitioned 250 GiB disk leaves only a sub-1 GiB tail gap
        // (GPT alignment slack) — nowhere near `MIN_LINUX_BYTES`.
        let input = scenario_input(DiskScenario::Linux, 0, PartitionMode::FreeSpace);
        assert_eq!(plan(&input), Err(PlanError::GapTooSmall));
    }

    #[test]
    fn linux_manual_with_no_entries_has_no_root() {
        let input = scenario_input(DiskScenario::Linux, 0, PartitionMode::Manual);
        assert_eq!(plan(&input), Err(PlanError::NoRoot));
    }

    #[test]
    fn build_preview_propagates_used_bytes_for_kept_segments_only() {
        let mut existing = part("/dev/fake0p1", MIB, 512 * MIB, Some(PK::Esp), Some("vfat"));
        existing.used_bytes = Some(100 * MIB);
        let partitions = vec![existing];
        let disk_info = disk(250 * GIB, TableKind::Gpt, false, false);
        let gaps = crate::backend::disk::layout::free_gaps(disk_info.size_bytes, &partitions);
        let mut cfg = base_cfg(PartitionMode::FreeSpace);
        cfg.selected_free_space_id = gaps
            .iter()
            .max_by_key(|g| g.size_bytes)
            .map(|g| g.id.clone());
        let input = PlanInput {
            disk: disk_info,
            partitions,
            gaps,
            ntfs: None,
            cfg,
            ram_bytes: 8 * GIB,
            uefi: true,
        };
        let result = plan(&input).unwrap();
        let kept = result
            .preview
            .iter()
            .find(|s| s.role == PreviewRole::Keep)
            .expect("kept esp segment");
        assert_eq!(kept.used_bytes, Some(100 * MIB));
        // Freshly created segments (the new ESP/root) never carry usage.
        assert!(
            result
                .preview
                .iter()
                .filter(|s| s.role != PreviewRole::Keep && s.role != PreviewRole::Free)
                .all(|s| s.used_bytes.is_none())
        );
    }

    #[test]
    fn empty_entire_disk_succeeds_on_a_blank_drive() {
        let input = scenario_input(DiskScenario::Empty, 0, PartitionMode::EntireDisk);
        let result = plan(&input).unwrap();
        assert!(result.erases.is_empty());
    }

    #[test]
    fn empty_free_space_uses_the_whole_disk_as_one_gap() {
        let input = scenario_input(DiskScenario::Empty, 0, PartitionMode::FreeSpace);
        let result = plan(&input).unwrap();
        assert!(result.ops.iter().any(|op| matches!(
            op,
            PlanOp::Create {
                role: PartitionRole::Esp,
                ..
            }
        )));
    }

    #[test]
    fn windows_alongside_windows_succeeds() {
        let input = scenario_input(DiskScenario::Windows, 0, PartitionMode::AlongsideWindows);
        assert!(plan(&input).is_ok());
    }

    #[test]
    fn windows_full_alongside_windows_shrink_too_small() {
        let input = scenario_input(
            DiskScenario::WindowsFull,
            0,
            PartitionMode::AlongsideWindows,
        );
        assert_eq!(plan(&input), Err(PlanError::ShrinkTooSmall));
    }

    #[test]
    fn bitlocker_alongside_windows_blocked() {
        let input = scenario_input(DiskScenario::BitLocker, 0, PartitionMode::AlongsideWindows);
        assert_eq!(
            plan(&input),
            Err(PlanError::NtfsBlocked(NtfsBlocker::BitLocker))
        );
    }

    #[test]
    fn dirty_ntfs_alongside_windows_blocked_on_first_blocker() {
        let input = scenario_input(DiskScenario::DirtyNtfs, 0, PartitionMode::AlongsideWindows);
        assert_eq!(
            plan(&input),
            Err(PlanError::NtfsBlocked(NtfsBlocker::Hibernated))
        );
    }

    #[test]
    fn freespace_tail_free_space_creates_its_own_esp() {
        let input = scenario_input(DiskScenario::FreeSpaceTail, 0, PartitionMode::FreeSpace);
        let result = plan(&input).unwrap();
        assert!(result.ops.iter().any(|op| matches!(
            op,
            PlanOp::Create {
                role: PartitionRole::Esp,
                size_bytes: NEW_ESP_BYTES,
                ..
            }
        )));
    }

    #[test]
    fn fragmented_free_space_uses_the_middle_gap() {
        let input = scenario_input(DiskScenario::Fragmented, 0, PartitionMode::FreeSpace);
        let result = plan(&input).unwrap();
        let root_start = result
            .ops
            .iter()
            .find_map(|op| match op {
                PlanOp::Create {
                    role: PartitionRole::Root,
                    start_bytes,
                    ..
                } => Some(*start_bytes),
                _ => None,
            })
            .expect("root create op");
        // Between the existing root (ends around 100 GiB + 513 MiB in) and
        // the trailing data partition — not at either end of the disk.
        assert!(root_start > 100 * GIB);
        assert!(result.erases.is_empty());
    }

    #[test]
    fn fragmented_manual_create_root_in_middle_gap() {
        let fixture = DiskScenario::Fragmented.fixture();
        let disk_info = fixture.disks[0].clone();
        let gaps =
            crate::backend::disk::layout::free_gaps(disk_info.size_bytes, &fixture.partitions);
        let middle_gap = gaps.iter().max_by_key(|g| g.size_bytes).unwrap();
        assert_eq!(middle_gap.size_bytes, 400 * GIB);

        let mut cfg = base_cfg(PartitionMode::Manual);
        cfg.manual = vec![
            ManualItem::Existing(ManualEntry {
                path: fixture.partitions[0].path.clone(), // existing ESP
                mount_point: ManualMountPoint::Boot,
                format: false,
                fs: FormatFs::Fat32,
            }),
            ManualItem::New(ManualNew {
                gap_id: middle_gap.id.clone(),
                size_bytes: 50 * GIB,
                mount_point: ManualMountPoint::Root,
                fs: FormatFs::Ext4,
            }),
        ];
        let input = PlanInput {
            disk: disk_info,
            partitions: fixture.partitions,
            gaps,
            ntfs: None,
            cfg,
            ram_bytes: 8 * GIB,
            uefi: true,
        };
        let result = plan(&input).unwrap();
        assert!(result.ops.iter().any(|op| matches!(
            op,
            PlanOp::Create {
                role: PartitionRole::Root,
                ..
            }
        )));
    }

    #[test]
    fn tiny_entire_disk_too_small() {
        let input = scenario_input(DiskScenario::Tiny, 0, PartitionMode::EntireDisk);
        assert!(matches!(plan(&input), Err(PlanError::DiskTooSmall { .. })));
    }

    #[test]
    fn tiny_free_space_gap_too_small() {
        let input = scenario_input(DiskScenario::Tiny, 0, PartitionMode::FreeSpace);
        assert_eq!(plan(&input), Err(PlanError::GapTooSmall));
    }

    #[test]
    fn tiny_alongside_windows_has_no_windows() {
        let input = scenario_input(DiskScenario::Tiny, 0, PartitionMode::AlongsideWindows);
        assert_eq!(plan(&input), Err(PlanError::NoWindows));
    }

    #[test]
    fn multi_disk_nvme_entire_disk_succeeds_without_warning() {
        let input = scenario_input(DiskScenario::MultiDisk, 0, PartitionMode::EntireDisk);
        assert!(plan(&input).is_ok());
        assert!(warnings(&input).is_empty());
    }

    #[test]
    fn multi_disk_usb_is_flagged_removable() {
        let input = scenario_input(DiskScenario::MultiDisk, 1, PartitionMode::EntireDisk);
        assert!(plan(&input).is_ok());
        assert_eq!(warnings(&input), vec![PlanWarning::RemovableTarget]);
    }

    #[test]
    fn mbr_free_space_needs_conversion() {
        let input = scenario_input(DiskScenario::Mbr, 0, PartitionMode::FreeSpace);
        assert_eq!(plan(&input), Err(PlanError::MbrNeedsConversion));
    }

    #[test]
    fn mbr_entire_disk_recreates_as_gpt() {
        let input = scenario_input(DiskScenario::Mbr, 0, PartitionMode::EntireDisk);
        let result = plan(&input).unwrap();
        assert!(matches!(
            result.ops[0],
            PlanOp::CreateTable {
                table: TableKind::Gpt,
                ..
            }
        ));
    }
}
