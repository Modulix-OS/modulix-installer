//! Catalog of injectable faults for `--fake-error=<id>`. Each entry either
//! trips one `DiskBackend`/`CryptBackend` method on `debugsim::disks`'s and
//! `debugsim::crypt`'s fakes — so the **real** task fails with its own real
//! message — or is checked by hand inside one `debugsim::tasks` fake task
//! double, for the stages that have no backend of their own
//! (`nixos-install`, `chpasswd`, …).

use crate::backend::disk::FormatFs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    PartitionUnmountBusy,
    PartitionCreateTable,
    PartitionCreate,
    PartitionResizeNtfs,
    FormatEsp,
    FormatRoot,
    FormatSwap,
    EncryptLuksFormat,
    EncryptLuksOpen,
    TpmNoDevice,
    UuidMissing,
    MountRoot,
    MountEsp,
    MountSwapon,
    ConfigInit,
    InstallEval,
    InstallBuild,
    InstallDownload,
    InstallDiskFull,
    EfiEfibootmgr,
    PasswdChpasswd,
    FlatpakRemoteAdd,
    PostUmountBusy,
}

impl Fault {
    pub const ALL: [Fault; 23] = [
        Fault::PartitionUnmountBusy,
        Fault::PartitionCreateTable,
        Fault::PartitionCreate,
        Fault::PartitionResizeNtfs,
        Fault::FormatEsp,
        Fault::FormatRoot,
        Fault::FormatSwap,
        Fault::EncryptLuksFormat,
        Fault::EncryptLuksOpen,
        Fault::TpmNoDevice,
        Fault::UuidMissing,
        Fault::MountRoot,
        Fault::MountEsp,
        Fault::MountSwapon,
        Fault::ConfigInit,
        Fault::InstallEval,
        Fault::InstallBuild,
        Fault::InstallDownload,
        Fault::InstallDiskFull,
        Fault::EfiEfibootmgr,
        Fault::PasswdChpasswd,
        Fault::FlatpakRemoteAdd,
        Fault::PostUmountBusy,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Fault::PartitionUnmountBusy => "partition:unmount-busy",
            Fault::PartitionCreateTable => "partition:create-table",
            Fault::PartitionCreate => "partition:create",
            Fault::PartitionResizeNtfs => "partition:resize-ntfs",
            Fault::FormatEsp => "format:esp",
            Fault::FormatRoot => "format:root",
            Fault::FormatSwap => "format:swap",
            Fault::EncryptLuksFormat => "encrypt:luks-format",
            Fault::EncryptLuksOpen => "encrypt:luks-open",
            Fault::TpmNoDevice => "tpm:no-device",
            Fault::UuidMissing => "uuid:missing",
            Fault::MountRoot => "mount:root",
            Fault::MountEsp => "mount:esp",
            Fault::MountSwapon => "mount:swapon",
            Fault::ConfigInit => "config:init",
            Fault::InstallEval => "install:eval",
            Fault::InstallBuild => "install:build",
            Fault::InstallDownload => "install:download",
            Fault::InstallDiskFull => "install:disk-full",
            Fault::EfiEfibootmgr => "efi:efibootmgr",
            Fault::PasswdChpasswd => "passwd:chpasswd",
            Fault::FlatpakRemoteAdd => "flatpak:remote-add",
            Fault::PostUmountBusy => "post:umount-busy",
        }
    }

    /// One-line description for `--fake-error=list`. Notes the two faults
    /// that must **not** fail the install, since they trip best-effort steps.
    pub fn description(self) -> &'static str {
        match self {
            Fault::PartitionUnmountBusy => {
                "unmount a target-disk partition before repartitioning fails with \"target is busy\""
            }
            Fault::PartitionCreateTable => {
                "creating the GPT/DOS partition table fails (udisks2 Format)"
            }
            Fault::PartitionCreate => "creating a new partition fails with \"no space left\"",
            Fault::PartitionResizeNtfs => "shrinking the Windows NTFS partition is refused",
            Fault::FormatEsp => "mkfs.vfat on the ESP exits with status 1",
            Fault::FormatRoot => "mkfs.<fs> on the root partition exits with status 1",
            Fault::FormatSwap => "mkswap on the swap partition exits with status 1",
            Fault::EncryptLuksFormat => {
                "cryptsetup luksFormat exits with status 4 (the historical double-definition bug)"
            }
            Fault::EncryptLuksOpen => "cryptsetup luksOpen refuses the passphrase",
            Fault::TpmNoDevice => "systemd-cryptenroll finds no TPM2 device",
            Fault::UuidMissing => "udisks2 reports no UUID for a formatted partition",
            Fault::MountRoot => "mounting the root partition on /mnt fails",
            Fault::MountEsp => "mounting the ESP on /mnt/boot fails",
            Fault::MountSwapon => "swapon on the swap partition fails",
            Fault::ConfigInit => {
                "modulix-core-utils' init() fails to commit the configuration repo"
            }
            Fault::InstallEval => {
                "nixos-install fails Nix evaluation (duplicate LUKS device definition)"
            }
            Fault::InstallBuild => "nixos-install fails to build a derivation",
            Fault::InstallDownload => "nixos-install fails to fetch a flake input (no network)",
            Fault::InstallDiskFull => "nixos-install fails with \"No space left on device\"",
            Fault::EfiEfibootmgr => "efibootmgr fails (NON-BLOCKING: the install still succeeds)",
            Fault::PasswdChpasswd => "chpasswd fails to set the account passwords",
            Fault::FlatpakRemoteAdd => {
                "flatpak remote-add cannot reach dl.flathub.org (NON-BLOCKING: the install still succeeds)"
            }
            Fault::PostUmountBusy => {
                "the final umount fails (NON-BLOCKING: the install still succeeds)"
            }
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.id() == s)
    }

    /// Picks one fault pseudo-randomly, for `--fake-error=random`. Good
    /// enough for "exercise some failure path by hand" — not used by any
    /// test, which always pins an exact id.
    pub fn random() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        Self::ALL[(nanos as usize) % Self::ALL.len()]
    }
}

/// Text printed by `--fake-error=list`.
pub fn catalog_text() -> String {
    let mut out = String::from("available --fake-error ids:\n");
    for fault in Fault::ALL {
        out.push_str(&format!("  {:<24} {}\n", fault.id(), fault.description()));
    }
    out.push_str("  random                   picks one of the above\n");
    out
}

/// Which of root/esp/swap `FormatTask`'s `format_partition` call is for,
/// inferred from the filesystem being written — the fake backend sees no
/// `PartitionRole`, only a `FormatFs`.
pub fn format_fault_for(fs: FormatFs) -> Fault {
    match fs {
        FormatFs::Fat32 => Fault::FormatEsp,
        FormatFs::LinuxSwap => Fault::FormatSwap,
        FormatFs::Ext4 | FormatFs::Btrfs | FormatFs::Xfs => Fault::FormatRoot,
    }
}

/// The `mkfs`/`mkswap` failure line a real backend would print for `fs`.
pub fn mkfs_failure_message(fs: FormatFs) -> String {
    match fs {
        FormatFs::Fat32 => "mkfs.vfat exited with status 1".to_string(),
        FormatFs::LinuxSwap => "mkswap exited with status 1".to_string(),
        other => format!("mkfs.{} exited with status 1", other.as_mkfs_type()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fault_round_trips_through_its_id() {
        for fault in Fault::ALL {
            assert_eq!(Fault::parse(fault.id()), Some(fault));
        }
    }

    #[test]
    fn parse_rejects_unknown_id() {
        assert_eq!(Fault::parse("not:a-fault"), None);
    }

    #[test]
    fn format_fault_mapping_matches_fs() {
        assert_eq!(format_fault_for(FormatFs::Fat32), Fault::FormatEsp);
        assert_eq!(format_fault_for(FormatFs::LinuxSwap), Fault::FormatSwap);
        assert_eq!(format_fault_for(FormatFs::Ext4), Fault::FormatRoot);
    }
}
