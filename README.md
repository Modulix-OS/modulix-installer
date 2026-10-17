# modulixos-installer

Installer for Modulix OS (NixOS-based) — a Rust + GTK4/libadwaita fullscreen
app, runs as root inside a `sway` kiosk session. Architecture details:
`CLAUDE.md`.

```bash
cargo run                                   # no flags: connects the REAL backends
```

`cargo run` with no flags enumerates **your real disks** — never press
Install on a dev machine, the confirmation dialog is the only thing between
the wizard and your partition table.

## Debug options

Available in a debug build only (`cargo run`, not `cargo run --release` nor
the live ISO — `#[cfg(debug_assertions)]` end to end, `src/debugsim/`). A
release build refuses to start if handed any of these flags.

| Flag | Role |
|---|---|
| `--fake-disks=<scenario>[,<scenario>...]` | Replaces the real disks with a synthetic set (`/dev/fakeN`) — no real device is ever touched. Several scenarios compose into one disk set. |
| `--fake-error=<id>` | Injects one specific fault at a given pipeline stage. |
| `--fake-error=random` | Picks one fault at random from the catalog. |
| `--fake-error=list` | Prints the fault catalog and exits (no display needed). |
| `--fake-instant` | Zeroes the fake install's simulated sleeps (~3s → near-instant). |

If any of these flags is given without `--fake-disks=`, the default scenario
is `multi-disk`.

### Disk scenarios (`--fake-disks=`)

| Name | Description |
|---|---|
| `empty` | Blank disk, no partition table. |
| `linux` | Linux already installed (ESP + ext4 root). |
| `windows` | Windows disk (ESP + MSR + NTFS + WinRE), room to shrink. |
| `windows-full` | Same Windows disk, but NTFS nearly full (shrink refused). |
| `bitlocker` | Windows disk with BitLocker active (blocks "alongside Windows" mode). |
| `dirty-ntfs` | Windows disk with hibernation/dirty volume (blocks shrinking). |
| `free-space-tail` | Windows + a large free gap at the end of the disk (alias `half-partitioned`). |
| `fragmented` | Fragmented Linux disk, 7 partitions / 5 gaps of varying sizes. |
| `tiny` | Tiny disk (8 GB), too small to install on. |
| `multi-disk` | 4 disks at once: blank, install USB key, Windows, existing Linux. |
| `mbr` | MBR/DOS-partitioned disk (needs conversion to GPT). |

Combining several scenarios: `--fake-disks=empty,windows,bitlocker` — each
name contributes disk index 0 of its fixture (except `multi-disk`, which
contributes all 4 of its disks), renumbered `/dev/fake0`, `/dev/fake1`, …

### Fault catalog (`--fake-error=`)

| id | Effect |
|---|---|
| `partition:unmount-busy` | Unmounting a target-disk partition before repartitioning fails ("target is busy"). |
| `partition:create-table` | Creating the partition table fails (udisks2 Format). |
| `partition:create` | Creating a new partition fails ("no space left"). |
| `partition:resize-ntfs` | Shrinking the Windows NTFS partition is refused. |
| `format:esp` | `mkfs.vfat` on the ESP fails (exit 1). |
| `format:root` | `mkfs.<fs>` on the root partition fails (exit 1). |
| `format:swap` | `mkswap` on the swap partition fails (exit 1). |
| `encrypt:luks-format` | `cryptsetup luksFormat` fails with status 4 (the historical double-definition bug). |
| `encrypt:luks-open` | `cryptsetup luksOpen` refuses the passphrase. |
| `tpm:no-device` | `systemd-cryptenroll` finds no TPM2 device. |
| `uuid:missing` | udisks2 reports no UUID for a formatted partition. |
| `mount:root` | Mounting the root partition on `/mnt` fails. |
| `mount:esp` | Mounting the ESP on `/mnt/boot` fails. |
| `mount:swapon` | `swapon` on the swap partition fails. |
| `config:init` | `modulix-core-utils`' `init()` fails to commit the configuration repo. |
| `install:eval` | `nixos-install` fails Nix evaluation (duplicate LUKS device definition). |
| `install:build` | `nixos-install` fails to build a derivation. |
| `install:download` | `nixos-install` fails to fetch a flake input (no network). |
| `install:disk-full` | `nixos-install` fails with "No space left on device". |
| `efi:efibootmgr` | `efibootmgr` fails — **non-blocking**, the install still succeeds. |
| `passwd:chpasswd` | `chpasswd` fails to set the account passwords. |
| `post:umount-busy` | The final umount fails — **non-blocking**, the install still succeeds. |

### Examples

```bash
cargo run -- --fake-error=list                           # catalog, then exit
cargo run -- --fake-disks=multi-disk                      # 4 disks, succeeds in ~3s
cargo run -- --fake-disks=empty,fragmented,bitlocker
cargo run -- --fake-disks=bitlocker                        # "alongside Windows" mode must be blocked
cargo run -- --fake-disks=windows --fake-error=encrypt:luks-format
cargo run -- --fake-disks=linux --fake-error=install:eval --fake-instant
cargo run -- --fake-disks=empty --fake-error=efi:efibootmgr   # must SUCCEED despite the fault
```

Release guard — must refuse to start:

```bash
cargo run --release -- --fake-disks=empty   # explicit error, non-zero exit code
```
