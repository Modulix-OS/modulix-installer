use crate::backend::disk::short_device_name;
use crate::engine::tasks::{best_effort, capture, report};
use crate::engine::{ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;

/// Label of the EFI boot entry this task owns, as shown by the firmware's
/// boot menu and by `efibootmgr`.
const ENTRY_LABEL: &str = "ModulixOS";

/// Loader this task points the entry at, in EFI (backslash) notation.
///
/// The removable fallback, not `\EFI\limine\…`: mxpkgs' `modulixos/boot.nix`
/// sets `boot.loader.efi.canTouchEfiVariables` to `false`, which flips nixpkgs'
/// `boot.loader.limine.efiInstallAsRemovable` default to `true`, so limine
/// installs itself here.
#[cfg(target_arch = "x86_64")]
const LOADER_PATH: &str = r"\EFI\BOOT\BOOTX64.EFI";
#[cfg(target_arch = "aarch64")]
const LOADER_PATH: &str = r"\EFI\BOOT\BOOTAA64.EFI";
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("no EFI removable fallback loader name is known for this architecture");

/// Registers the firmware boot entry for the installed system: a single
/// `ModulixOS` entry, first in `BootOrder`, every pre-existing entry
/// (Windows Boot Manager, UEFI Shell, PXE…) left untouched.
///
/// This task is the *only* writer of EFI boot variables in the whole chain.
/// mxpkgs' `modulixos/boot.nix` — the configuration of the *installed* system,
/// so the setting survives every rebuild — turns off nixpkgs' own NVRAM write,
/// for two reasons:
///
/// * nixpkgs' `limine-install.py` hardcodes the label `Limine` and offers no
///   option to change it, so the name could only be fixed up afterwards —
///   and it re-creates its entry on every `nixos-rebuild`, which would make
///   the rename drift back (or pile up duplicates) on the installed system.
///   With the NVRAM write disabled, nothing ever rewrites this entry: the
///   name persists by construction.
/// * The boot order is therefore imposed **once**, here, at install time. A
///   user who later puts Windows back in front in their firmware is never
///   contradicted on subsequent boots.
///
/// Every step is best-effort and never fails the install: the system is
/// already on disk by the time this runs, and the loader is also reachable
/// through the removable fallback path the firmware probes on its own.
pub struct EfiEntryTask;

#[async_trait]
impl Task for EfiEntryTask {
    fn label(&self) -> String {
        "Registering the boot entry".to_string()
    }

    fn weight(&self) -> u32 {
        1
    }

    /// # Pre-conditions
    /// `NixosInstallTask` has run, so limine's loader is already on the ESP,
    /// and `/mnt` is still mounted (`PostInstallTask` has not released it).
    ///
    /// # Post-conditions
    /// On a UEFI install, exactly one `ModulixOS` entry exists and heads
    /// `BootOrder`; no other entry is deleted or reordered. On anything else
    /// — BIOS boot, a missing ESP, an `efibootmgr` failure — the reason is in
    /// the install log and the install still succeeds.
    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        if !ctx.uefi {
            log(tx, "BIOS boot: no EFI boot entry to register").await;
            return Ok(());
        }

        let Some(esp) = ctx.state.lock().await.efi_partition.clone() else {
            log(tx, "no ESP was recorded: EFI boot entry skipped").await;
            return Ok(());
        };

        let node = match ctx.backends.disk.device_node(&esp).await {
            Ok(node) => node,
            Err(e) => {
                report(tx, "efi boot entry", Err(e.to_string())).await;
                return Ok(());
            }
        };

        let (disk, part) = match esp_location(&node).await {
            Ok(location) => location,
            Err(e) => {
                report(tx, "efi boot entry", Err(e)).await;
                return Ok(());
            }
        };

        let listing = match capture("efibootmgr", &[]).await {
            Ok(listing) => listing,
            Err(e) => {
                report(tx, "efibootmgr", Err(e)).await;
                return Ok(());
            }
        };
        let (entries, previous_order) = parse_efibootmgr(&listing);

        // A second install in the same live session would otherwise end up
        // with two `ModulixOS` entries pointing at two different disks.
        let stale: Vec<String> = entries
            .iter()
            .filter(|e| e.label == ENTRY_LABEL)
            .map(|e| e.num.clone())
            .collect();
        for num in &stale {
            report(
                tx,
                &format!("efibootmgr -b {num} -B"),
                best_effort("efibootmgr", &["-b", num, "-B"]).await,
            )
            .await;
        }

        let created = best_effort(
            "efibootmgr",
            &[
                "--create",
                "--disk",
                &disk,
                "--part",
                &part,
                "--loader",
                LOADER_PATH,
                "--label",
                ENTRY_LABEL,
            ],
        )
        .await;
        report(
            tx,
            &format!("efibootmgr --create {ENTRY_LABEL} ({disk} part {part})"),
            created.clone(),
        )
        .await;
        if created.is_err() {
            return Ok(());
        }

        // `--create` already inserts at the head, but reading the entry back
        // is the only way to learn its boot number, and writing the order out
        // explicitly makes the result deterministic.
        let listing = match capture("efibootmgr", &[]).await {
            Ok(listing) => listing,
            Err(e) => {
                report(tx, "efibootmgr", Err(e)).await;
                return Ok(());
            }
        };
        let (entries, _) = parse_efibootmgr(&listing);
        let Some(ours) = entries.iter().find(|e| e.label == ENTRY_LABEL) else {
            report(
                tx,
                "efi boot entry",
                Err(format!("{ENTRY_LABEL} is not in the boot record")),
            )
            .await;
            return Ok(());
        };

        let order = new_boot_order(&ours.num, &previous_order, &stale);
        let joined = order.join(",");
        report(
            tx,
            &format!("efibootmgr -o {joined}"),
            best_effort("efibootmgr", &["-o", &joined]).await,
        )
        .await;

        log(tx, &format!("boot order: {joined}")).await;
        Ok(())
    }
}

/// One entry of `efibootmgr`'s listing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BootEntry {
    /// Boot number as printed, four hexadecimal digits (`"0001"`).
    num: String,
    /// Entry label (`"ModulixOS"`, `"Windows Boot Manager"`).
    label: String,
}

/// Resolves the disk and partition number `efibootmgr --create` needs from a
/// partition device node.
///
/// * `device_node` - the ESP's device node, e.g. `/dev/nvme0n1p3`.
///
/// # Returns
/// `(disk device node, partition number)`, e.g. `("/dev/nvme0n1", "3")`.
///
/// Both are read out of sysfs rather than derived by stripping a suffix, so
/// `sda3`, `nvme0n1p3` and `mmcblk0p3` all work without a naming heuristic —
/// the same approach nixpkgs' `limine-install.py` takes.
///
/// # Errors
/// A human-readable reason if `device_node` is not a partition, or if sysfs
/// does not expose its parent disk.
async fn esp_location(device_node: &str) -> Result<(String, String), String> {
    let name = short_device_name(device_node);
    let sysfs = format!("/sys/class/block/{name}");

    let number = tokio::fs::read_to_string(format!("{sysfs}/partition"))
        .await
        .map_err(|e| format!("{device_node} is not a partition: {e}"))?
        .trim()
        .to_string();

    let canonical = tokio::fs::canonicalize(&sysfs)
        .await
        .map_err(|e| format!("{sysfs} cannot be resolved: {e}"))?;
    let disk = canonical
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("sysfs exposes no parent disk for {device_node}"))?;

    Ok((format!("/dev/{disk}"), number))
}

/// Parses `efibootmgr`'s listing.
///
/// * `output` - what `efibootmgr` printed on stdout, with or without `-v`.
///
/// # Returns
/// Every boot entry in listing order, and the boot numbers of the
/// `BootOrder:` line (empty when the firmware prints none).
///
/// `BootCurrent:`, `BootNext:` and `Timeout:` lines are ignored, as is the
/// device path `-v` appends after the label.
fn parse_efibootmgr(output: &str) -> (Vec<BootEntry>, Vec<String>) {
    let mut entries = Vec::new();
    let mut order = Vec::new();

    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("BootOrder:") {
            order = rest
                .trim()
                .split(',')
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .collect();
            continue;
        }

        let Some(rest) = line.strip_prefix("Boot") else {
            continue;
        };
        // `BootCurrent:`/`BootNext:` never pass this: a boot number is
        // exactly four hexadecimal digits.
        if rest.len() < 4 || !rest[..4].chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let (num, rest) = rest.split_at(4);
        let rest = rest.strip_prefix('*').unwrap_or(rest);
        let label = rest.split('\t').next().unwrap_or(rest).trim();
        entries.push(BootEntry {
            num: num.to_string(),
            label: label.to_string(),
        });
    }

    (entries, order)
}

/// Builds the `BootOrder` to write: our entry first, everything else kept in
/// the order the firmware already had it.
///
/// * `ours` - boot number of the `ModulixOS` entry.
/// * `previous` - `BootOrder` as read before the entry was created.
/// * `removed` - boot numbers deleted by this task, which must not come back.
///
/// # Returns
/// `ours`, then the surviving entries of `previous`, each at most once.
///
/// # Post-conditions
/// No pre-existing entry is dropped: an installed Windows keeps its boot
/// option, one position further down.
fn new_boot_order(ours: &str, previous: &[String], removed: &[String]) -> Vec<String> {
    let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    let mut order = vec![ours.to_string()];
    for num in previous {
        if same(num, ours)
            || removed.iter().any(|r| same(r, num))
            || order.iter().any(|kept| same(kept, num))
        {
            continue;
        }
        order.push(num.clone());
    }
    order
}

/// Sends one plain line to the install log.
///
/// * `tx` - progress sink.
/// * `line` - message to log.
async fn log(tx: &ProgressSink, line: &str) {
    let _ = tx.send(ProgressEvent::Log(line.to_string())).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = "BootCurrent: 0003\n\
Timeout: 1 seconds\n\
BootOrder: 0003,0001,0000\n\
Boot0000* Windows Boot Manager\tHD(1,GPT,...)/File(\\EFI\\Microsoft\\Boot\\bootmgfw.efi)\n\
Boot0001  UEFI Shell\tHD(2,GPT,...)\n\
Boot0003* ModulixOS\tHD(3,GPT,...)/File(\\EFI\\BOOT\\BOOTX64.EFI)\n";

    #[test]
    fn parses_entries_and_order() {
        let (entries, order) = parse_efibootmgr(LISTING);
        assert_eq!(order, vec!["0003", "0001", "0000"]);
        assert_eq!(
            entries,
            vec![
                BootEntry {
                    num: "0000".into(),
                    label: "Windows Boot Manager".into()
                },
                BootEntry {
                    num: "0001".into(),
                    label: "UEFI Shell".into()
                },
                BootEntry {
                    num: "0003".into(),
                    label: "ModulixOS".into()
                },
            ]
        );
    }

    #[test]
    fn parses_a_listing_without_verbose_device_paths() {
        let (entries, order) = parse_efibootmgr("BootOrder: 0001\nBoot0001* Limine\n");
        assert_eq!(order, vec!["0001"]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "Limine");
    }

    #[test]
    fn tolerates_a_firmware_printing_no_boot_order() {
        let (entries, order) = parse_efibootmgr("BootCurrent: 0001\nBoot0001* ModulixOS\n");
        assert!(order.is_empty());
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn our_entry_comes_first_and_the_others_survive() {
        let previous = vec!["0000".to_string(), "0001".to_string()];
        assert_eq!(
            new_boot_order("0003", &previous, &[]),
            vec!["0003", "0000", "0001"]
        );
    }

    #[test]
    fn a_reinstall_does_not_resurrect_its_own_deleted_entry() {
        let previous = vec!["0002".to_string(), "0000".to_string()];
        let removed = vec!["0002".to_string()];
        assert_eq!(
            new_boot_order("0004", &previous, &removed),
            vec!["0004", "0000"]
        );
    }

    #[test]
    fn our_entry_is_never_listed_twice() {
        let previous = vec!["0003".to_string(), "0000".to_string()];
        assert_eq!(new_boot_order("0003", &previous, &[]), vec!["0003", "0000"]);
    }
}
