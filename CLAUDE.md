# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`modulixos-installer` is the installer for Modulix OS (NixOS-based distro): a single **fullscreen Rust + GTK4/libadwaita app** running as root inside a minimal `sway` kiosk session (Windows-installer style), replacing the older Calamares-based flow.

**Current state: the whole wizard plus a real, destructive install run end to end** — partition → encrypt → format → mount → TPM2 enroll → write the NixOS configuration → `nixos-install` → set passwords → release the target. Steps 7-9 (user, desktop environment, application pack) still have a minimal UI, but their answers now really reach the installed system.

**The installed system's configuration belongs to mxpkgs, not to this repo.** Anything that
describes how Modulix OS behaves once installed — bootloader, limine, kernel, desktop,
defaults — goes in `mxpkgs/modulixos/`. The installer only generates what is specific to
*this* machine and cannot be known in advance (`luks.nix`, `apps.nix`, hostname, users). So a
limine or `boot.loader` change is an mxpkgs change, even when the reason for it lives here.
Caveat: the generated `flake.nix` resolves `github:Modulix-OS/mxpkgs` with no rev, so such a
change reaches installs only once pushed.

**mxpkgs has no ISO/installer module** — `mxpkgs/installer/default.nix` does not exist and never did. The live ISO is defined here, in `nix/iso.nix`, on top of nixpkgs' `installation-cd-minimal.nix`, and imports `modulixos/branding.nix`, `modulixos/boot.nix` and `modulixos/options` from mxpkgs (a flake input) for the Modulix identity and boot configuration. Moving the ISO into mxpkgs is a later step, not a prerequisite.

`branding.nix` carries **no** boot configuration: everything Modulix about the boot lives in `mxpkgs/modulixos/boot.nix`, which `nix/iso.nix` **imports directly** rather than copying — along with `mxpkgs/modulixos/options`, which declares the `mx.mode.server.enable` that `boot.nix` reads (importing `boot.nix` alone fails with `error: attribute 'mode' missing` as soon as anything forces `boot.plymouth.enable`). The import is guarded by two lines, and those are the only reason the copy ever existed:
- `mx.bootloader.enable = false` — the option defaults to `true`, which would enable limine next to the ISO image builder. Nothing *conflicts* (`iso-image.nix` only does `boot.loader.grub.enable = mkImageMediaOverride false` and never claims `system.build.installBootLoader`), so limine would evaluate and build; it would just be dead weight in a closure whose boot actually goes through the isolinux/grub-efi written into the image.
- `boot.tmp.useTmpfs = lib.mkForce false` — `boot.nix` sets it `true`, and the live ISO's root is already a tmpfs overlay.

Everything else comes from `boot.nix` unchanged and must **not** be restated here: the `boot.plymouth` block (theme `modulix` = a `bgrt` clone with the Modulix logo as watermark, so on real hardware the firmware vendor logo shows through by design), the quiet-boot `boot.kernelParams` — including `iommu=pt`, which mxpkgs carries as a fix for some AMD CPUs — `consoleLogLevel`, `initrd.verbose`, and `initrd.systemd.enable` (already `true` on the ISO before the import, so this is not a change). `isoImage.splashImage`/`efiSplashImage` read `config.mx.bootloader.splash.image` for the same reason. Note `vt.global_cursor_default=0` is in the kernel-params list: a failed kiosk start leaves a completely black VT, which is why the tty1 recovery shell below is not optional.

The only behavioural deltas the import introduces are inert: `systemd.network.wait-online.enable = false` (the ISO runs NetworkManager, `systemd.network.enable` is `false`) and a masked `systemd-udev-settle` unit in both stages (the unit no longer exists upstream in this nixpkgs; `waitForGpu` in `nix/kiosk-module.nix` calls `udevadm settle` itself regardless).

One duplication is left on purpose: `system.nixos.label` recomputes `branding.nix`'s `asciiCodeName` `replaceStrings` table, because mxpkgs exports no slug helper — removing it needs an mxpkgs change.

## Commands

```bash
cargo fmt && cargo clippy -- -D warnings   # mandatory before any commit (global rule)
cargo test                                  # pure logic only — swap sizing, partition planner,
                                             # zone.tab/evdev.xml parsing, step-registry is_relevant()
cargo run                                   # no CLI flags at all; connects the REAL backends
```

**There is no simulation mode and no dev-machine workflow with safe disks.** `cargo run` on a
dev box does start (udisks2 and NetworkManager answer on the system bus) and enumerates **your
real disks**. Steps 1-9 are safe to click through; **never press Install** — the confirmation
dialog is the only thing between the wizard and your partition table. Exercising the wizard end
to end is what the VM is for:

```bash
nix build .#iso     # the live ISO (nix/iso.nix)
nix run  .#vm       # boot that ISO in qemu: UEFI, scratch 40 GiB disk, swtpm, virgl on
MODULIXOS_VM_GL=0 nix run .#vm   # same, virtio-gpu without 3D — the libvirt-like case the
                                 # kiosk's software-renderer fallback exists for
```

The install log is written to `/var/log/modulixos-install.log` (readable from tty2 during the
live session, or `journalctl -u modulix-installer-session -b`) and copied to
`/mnt/var/log/modulixos-install.log`, which is the only copy that survives a reboot of the
target machine.

The `lib` handed to `nixosSystem` must be `mxpkgs.lib.extendLib nixpkgs.lib`: every mxpkgs module calls `lib.mkMxDefault`, which plain `nixpkgs.lib` does not have.

## Concurrency model

GTK4's main loop is single-threaded. A multi-thread **tokio** runtime hosts all backend work (D-Bus via `zbus`, subprocess calls). Results flow back to the UI through an `async_channel`, consumed on the GTK side with `glib::spawn_future_local`. Rule: **never block the main loop** — no synchronous D-Bus or process calls from a GTK callback.

## Architecture

### `trait Step` (`src/steps/mod.rs`) — one wizard page

```rust
pub trait Step {
    fn id(&self) -> StepId;
    fn title(&self) -> String;              // re-fetched on language change
    fn icon_name(&self) -> &'static str;    // symbolic SVG from the gresource
    fn widget(&self) -> gtk::Widget;        // built once, cached
    fn is_relevant(&self, cfg: &InstallConfig) -> bool;   // conditional skip
    fn validate(&self) -> StepValidity;     // Ready | Blocked(String)
    fn commit(&self, cfg: &mut InstallConfig) -> mx::Result<()>;
    fn retranslate(&self);                  // called on language change
}
```

Object-safe. `StepValidity` is exposed as a GObject property observed by the "Next" button (enable/disable without polling).

### Backends (`src/backend/`) — one trait per subsystem, one impl each

| Trait | Real impl | Role |
|---|---|---|
| `DiskBackend` | `Udisks2Backend` (zbus) | enumerate disks/partitions/free space, create/delete/resize/format/mount |
| `NetworkBackend` | `NetworkManagerBackend` (zbus) | ethernet, Wi-Fi scan/connect, `Connectivity` → captive-portal detection |
| `LocaleBackend` | `SystemLocaleBackend` | locales (`locale.gen`), timezones (`zone.tab` lat/lon for the map), keyboard layouts (`evdev.xml` from xkeyboard-config) |
| `A11yBackend` | `OrcaBackend` | narrator (speech-dispatcher/orca) — the only method that does anything in the kiosk session; the other four are best-effort GNOME a11y `gsettings` calls (silently `Ok(())` if `gsettings` isn't on `PATH`, real errors otherwise), useful only on a dev machine under a full GNOME session. What actually applies live to the installer itself (high contrast, large text) is `src/a11y.rs`'s `A11ySettings`, not this trait |
| `CryptBackend` | `CryptsetupBackend` | `cryptsetup luksFormat`, `systemd-cryptenroll --tpm2-device` (± `--tpm2-with-pin`) |

There is no second implementation of any of these: the fake backends, the `--fake` flag and the `is_fake` gate are gone. `Backends::new()` is fallible and a failure is **fatal** — `main` shows a fullscreen, selectable error page and exits non-zero rather than degrading into a simulation.

### `trait Task` (`src/engine/mod.rs`) — install pipeline

```rust
#[async_trait]
pub trait Task {
    fn label(&self) -> String;
    fn weight(&self) -> u32;    // for the progress bar
    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()>;
}
```

Pipeline: `PartitionTask` → `EncryptTask` → `FormatTask` → `MountTask` → `EnrollTpmTask` → `InitConfigTask` → `ExtraConfigTask` → `NixosInstallTask` → `EfiEntryTask` → `SetPasswordsTask` → `PostInstallTask`.

- `EncryptTask` must run right after `PartitionTask` and before `FormatTask`/`MountTask` — `luksFormat`/`luksOpen` need the bare partition, not one already `mkfs`'d and mounted on `/mnt`.
- `InitConfigTask` calls `modulix_core_utils::init::init` (synchronous, so `spawn_blocking`) with `config_dir = /mnt/etc/modulix-os`, which writes and commits the config repo but **deliberately skips the rebuild** — it holds core-utils' skip-rebuild lock precisely so the installer drives the build itself.
- `ExtraConfigTask` adds what `init` knows nothing about: `luks.nix` (without it an encrypted install is unbootable) and `apps.nix` (the application pack). Both are new files, so they escape `init`'s immutable seal; only `configuration.nix` is unsealed with `chattr -i`, extended with the extra `imports`, resealed, and everything is committed — Nix ignores untracked files in a git tree.
- `NixosInstallTask` runs `nixos-install --root /mnt --flake /mnt/etc/modulix-os#default --no-root-password` with both streams piped, forwarding each line as `ProgressEvent::Log`. This is why the pipeline does not go through core-utils' `rebuild_config`, which inherits stdout and could never feed the log view.
- `EfiEntryTask` is the only writer of EFI boot variables in the whole chain, and it writes them **once**: it deletes any stale `ModulixOS` entry, runs `efibootmgr --create … --loader \EFI\BOOT\BOOTX64.EFI --label ModulixOS`, then sets `BootOrder` explicitly to its own boot number followed by the previous order verbatim — Windows Boot Manager and friends keep their entry, one position down. Nothing is ever re-imposed afterwards, so a user who puts Windows back in front in their firmware is not contradicted on the next boot. Everything is best-effort: a BIOS boot, a missing ESP or an `efibootmgr` failure lands in the install log and the install still succeeds.
- `SetPasswordsTask` pipes `root:…`/`user:…` into `chpasswd` under `nixos-enter`, so no password — hashed or not — ever reaches the Nix store.
- Every stage always runs for real; `Pipeline::run` aborts on the first error and that error is what the progress page renders.
- Every `ProgressEvent` is teed to `engine::install_log` (file + stderr → journald) by a tokio task before reaching the GTK loop, so a failed install is still diagnosable after the app dies.

`ProgressEvent::Indeterminate(bool)` lets a stage with no fraction of its own (`nixos-install`) put the progress bar in pulse mode.

### Directory layout

```
src/
  main.rs               bootstrap adw::Application, no CLI flags; a backend that cannot be
                        reached is fatal (fullscreen error page, non-zero exit)
  app.rs                AdwApplicationWindow fullscreen, NavigationView, step rail
  a11y.rs                A11ySettings (GObject) — shared narrator/high-contrast/large-text/
                        magnifier/sticky-keys state, live effects on the installer itself
  config.rs             InstallConfig — all answers, Rc<RefCell<_>>
  i18n.rs               gettext + hot reload — see "i18n" below for the LANGUAGE/setlocale split
  steps/                mod.rs (trait + registry) + one file per step;
                        user/ (mod.rs, username.rs, hostname.rs)
  finish/               summary.rs, progress.rs — post-step-9 review (recomputes
                        `plan::plan` against live disk state, destructive-confirmation
                        dialog) + install-progress page; pushed outside the step rail,
                        `StepId::ALL` stays at 9
  backend/              disk/ (mod.rs, layout.rs free-gap math, ntfs.rs shrink-preflight
                        parser, scenario.rs `#[cfg(test)]` fixtures, udisks2.rs)
                        net/ locale/ a11y/ crypt/  (trait + impl)
  engine/               trait Task, Pipeline, ProgressSink, install_log.rs (persistent
                        install log), live_input.rs (shared
                        live-disk-state → `plan::PlanInput` fetch), plan.rs (pure
                        partitioning planner), tasks/ (partition.rs, encrypt.rs, format.rs,
                        mount.rs, enroll_tpm.rs, init_config.rs, extra_config.rs,
                        nixos_install.rs, efi_entry.rs, set_passwords.rs,
                        post_install.rs)
  widgets/              timezone_map.rs, disk_bar.rs, partition_editor.rs, password_entry.rs,
                        ap_row.rs, wifi_dialog.rs, portal_window.rs
data/
  icons/*.svg           one symbolic icon per step — GTK4 symbolic constraint: must be
                        fill-only (no `stroke`, no `fill="none"`); GTK recolors `fill`
                        attributes via the injected `<style>` but never `stroke`, so a
                        stroked path stays the icon's original color instead of following
                        the theme (rings/outlines are drawn as an evenodd fill between two
                        concentric shapes instead)
  screenshots/          gnome.webp, plasma.webp, lxqt.webp
  resources.gresource.xml
nix/
  iso.nix               live ISO: installation-cd-minimal + mxpkgs branding/boot/options + kiosk
  kiosk-module.nix      the sway kiosk session (systemd unit + PAM + tty1 recovery
                        shell), ISO runtime deps
  package.nix           naersk derivation; wrapGAppsHook4 wiring
  vm.nix                `nix run .#vm` qemu runner (UEFI + swtpm + scratch disk)
build.rs                glib_build_tools::compile_resources
```

### i18n: `LANGUAGE` drives messages, `setlocale` drives formatting

`setlocale(LC_ALL, code)` silently no-ops (`NULL`, locale left untouched) for any locale glibc
hasn't *generated* on the running machine — which is most of the ~470 entries the language-step
dropdown offers. `src/i18n.rs` therefore drives message-catalog selection through the `LANGUAGE`
env var instead (gettext resolves it as a directory name under the bound text domain — no
generated locale needed) and only uses `setlocale` best-effort, for formatting (dates, numbers,
collation). Two consequences to keep in mind when touching this code:

- Changing `LANGUAGE` alone doesn't retranslate anything already looked up — glibc caches
  translations behind the exported `_nl_msg_cat_cntr` counter, which `i18n::set_language` bumps by
  hand after every change (the documented GNU gettext runtime-language-switch recipe).
- `LANGUAGE` is ignored outright when `LC_MESSAGES` is `C`/`POSIX` (`C.UTF-8` included), so
  `i18n::init()` must land `LC_MESSAGES` on some real generated locale first. On the real ISO this
  means `nix/kiosk-module.nix` **must set `i18n.supportedLocales` to generate at least one
  non-`C` locale** — with
  none generated, `LANGUAGE` is ignored for the whole session and the language step becomes a
  no-op again, exactly the bug this module fixed.
- `po/` currently ships `fr.po` only — French and English (gettext's `msgid` fallback) are the
  only two languages that visibly differ today, even though the dropdown lists every glibc locale.

### The 9 steps, fixed order

1. **Accessibility** — first page, before anything else. Carries the narrator toggle
   (`Enable narrator`, starts orca immediately — merged in from the former standalone
   narrator step) plus two groups: "Applied now" (high contrast, large text — take effect
   immediately in the installer itself via `src/a11y.rs`) and "Applied to the installed
   system" (screen magnifier, sticky keys — compositor-level settings the kiosk compositor doesn't expose
   to the app, so these are only recorded into `InstallConfig` and best-effort forwarded to
   `gsettings`, not applied live). All five toggles are also reachable from every other page
   via a header-bar accessibility `MenuButton` popover (`<Ctrl><Alt>a`), bound to the same
   shared `A11ySettings` instance so step 1 and the popover never drift out of sync.
2. **Language** — triggers `retranslate()` on every already-built page.
3. **Timezone** — `gtk::DrawingArea` world map, equirectangular projection of `zone.tab` coordinates, click → nearest zone (same approach as Calamares) + region/city fallback list.
4. **Keyboard** — layout + variant from `evdev.xml`, live typing test area, applies the layout immediately.
5. **Network** — ethernet/Wi-Fi via NetworkManager; if `Connectivity == Portal`, opens an `adw::Dialog` with embedded WebKitGTK, auto-closed once `Connectivity == Full`. Fully implemented (`backend/net/network_manager.rs`): open/secured/hidden Wi-Fi, WPA2/WPA3 (transition-safe), live connectivity watch, "Next" gated on `Connectivity` (`Full` → ready, `Limited`/`Unknown` → blocked with a "Continue anyway" override, `Portal`/`None` → hard block). When NM reports no connectivity-check URI there is no way to know the portal's address, and `widgets::portal_window`'s `no_portal_uri_html` says so instead of showing a fake sign-in button.
6. **Partitioning** — Fully implemented. `engine::plan::plan` is the single pure function
   both the UI (live "before"/"after" `DiskBar` preview + blocked-reason row subtitles)
   and the install pipeline (`engine::tasks::PartitionTask`) go through, so the preview
   and what actually gets written can never diverge. 4 modes: erase entire disk / install
   alongside Windows (NTFS shrink preflight in `backend::disk::ntfs`: refuses on
   BitLocker/hibernation/a dirty volume) / use free space / manual. **Modulix always
   creates its own dedicated `NEW_ESP_BYTES` (1024 MiB) FAT32 ESP**, in all four modes —
   the Windows ESP (or any other pre-existing one) is never reused or reformatted;
   dual-boot is handled later by a limine `extraEntries` pointing at the Windows Boot
   Manager (a `modulix-core-utils` config option, **iteration 2, not yet implemented**).
   Manual mode is a real ordered disk editor: `widgets::PartitionEditor` renders one row
   per existing partition, free-space gap, and pending creation, sorted left-to-right by
   disk offset to match the `DiskBar` above it. Each free-space row's "+" button opens an
   `adw::AlertDialog` to carve a new partition (mount point, filesystem, size) straight
   out of that gap — no GParted round-trip needed for that; GParted (launched from a
   button on the manual page, `steps/partitioning.rs`'s `run_gparted`) is only needed to
   resize or delete an *existing* partition. Formatting is
   forced for `/`, `/boot`, and swap (`engine::plan::format_is_forced`) — only `/home` can
   be kept as-is. The editor collects `Vec<plan::ManualItem>` (`Existing` assignment or
   `New` creation) and preserves the user's assignments/pending creations across a
   `set_data()` reload (e.g. after returning from GParted) — all validation lives in
   `plan::plan`, not the widget. Outside manual: swap (none / standard
   / hibernation, sized via `engine::sizing`) + LUKS2 encryption + TPM2 (with or without
   PIN). `backend::disk::scenario::DiskScenario` is a `#[cfg(test)]`
   fixture set driving `engine::plan`'s scenario matrix, so every mode/blocker combination is
   covered by a test rather than by hand.
7. **User** — primary user, computer name (`networking.hostName`, default `modulixos`,
   validated as an RFC 1123 label by `steps::user::hostname`); root gets the same
   password. Neither password is written into the configuration — see `SetPasswordsTask`.
8. **Desktop environment** — gnome / plasma / lxqt, with description and screenshot. No
   outer "Next" button on this page (`Step::shows_next` returns `false`): picking a card
   opens a zoom dialog whose own "Choose this desktop environment" button both commits the
   selection and advances straight to the **application pack** step, via the `AdvanceHook`
   the app wires up (`steps::new_advance_hook`) — see `src/app.rs`.
9. **Application pack** — two big cards: "No applications" (bare system) vs "Base pack"
   (browser, file manager, printing, PDF reader, image viewer, archive manager, text
   editor, media player — the package list is per-DE, hardcoded in `config.rs`, no office
   suite). Same pattern as step 8: no outer "Next" button (`Step::shows_next` returns
   `false`), a card click commits `InstallConfig::app_pack` and advances straight to the
   summary via the same `AdvanceHook`.

Then a summary (its "Install" button uses `suggested-action`, not `destructive-action` — the
confirmation dialog it opens stays destructive) → install-progress screen (an auto-advancing
`widgets::slideshow::Slideshow` fills the main area, collapsed-by-default log in a
`gtk::Expander` that auto-scrolls and opens itself on failure, progress bar pinned to the
bottom, pulsing while `nixos-install` runs) → done.

**Scope**: the install is real end to end. Steps 7-9 still have a minimal UI, but their
answers reach the installed system. Known gap: `limine`'s `extraEntries` for a Windows
dual boot is still unimplemented (a `modulix-core-utils` config option).

## Contract with `modulix-core-utils`

Direct `path` dependency (decided):

```toml
modulix-core-utils = { path = "../modulix-core-utils", features = ["init", "filesystem", "detect-hardware", "user", "locale"] }
```

- **`CONFIG_DIRECTORY`** (`modulix-core-utils/src/lib.rs`) is `/etc/modulix-os/` in release, `<repo>/test/` in debug — a real NixOS fixture repo used by examples/tests. See `modulix-core-utils/CLAUDE.md` for details.
- **`BuildCommand::as_str()`** collapses to `"build-vm"` for every variant in debug builds — a debug run never touches the host, it builds a VM image instead. Only release builds actually run `nixos-install`.
- `init(&InitParams)` does the whole seed in **one transaction** (main config + locale + keyboard + user) and is what `InitConfigTask` calls. It is synchronous (`std::process`, `git2`), so it must go through `spawn_blocking`. There is no `init_all` and none is needed.
- `init()` **never rebuilds**: outside debug it holds core-utils' skip-rebuild lock for its whole run, and its own doc says the installer drives the build. `rebuild_config` is private and unreachable anyway — hence `NixosInstallTask`.
- `InitParams::config_dir` is passed explicitly (`/mnt/etc/modulix-os`) so `resolve_config_path` behaves the same in debug and release; left to `None`, a debug build would write into core-utils' own `test/` tree.
- `--root /mnt` is hardcoded in `rebuild_config` (`modulix-core-utils/src/core/transaction/transaction.rs:226`) — the installer must mount the install target at `/mnt`, not a configurable path.
- `rebuild_config` inherits stdout and only captures stderr for the error message — no live streaming. This is precisely why `NixosInstallTask` spawns `nixos-install` itself with both streams piped.

### Pitfall: `configuration.nix` imports vs. file overwrite

`Transaction::begin()` auto-inserts any newly-created file into `configuration.nix`'s `imports` list — but `init()` then overwrites `configuration.nix` wholesale, discarding that insertion, which is why its `configuration_nix()` lists `./hardware-configuration.nix`, `./fstab.nix`, `./locale.nix`, `./users.nix` explicitly. `ExtraConfigTask` adds its own files to that list for the same reason, after the fact. (Existing side effect this masks today: `flake.nix` is in `add_file`, so `begin()` inserts a nonsensical `./flake.nix` into `imports` — currently hidden only because the overwrite wipes it.)

## Security / data-loss risk points to handle explicitly

- **Passwords must never land in the Nix store in plaintext.** `user::add_no_transaction` writes `initialPassword`, and `init()` passes an empty one. The installer therefore never puts a password in the configuration at all: `SetPasswordsTask` pipes `chpasswd` into `nixos-enter` after the install, writing `/etc/shadow` directly. This depends on `users.mutableUsers` staying `true`.
- **NTFS resize ("alongside Windows" mode)**: refuse if BitLocker is detected (can't be shrunk from Linux), if Windows fast-startup/hibernation left the volume dirty (`hiberfil.sys`, dirty flag), or if `ntfsresize --no-action` fails. Mandatory preflight before any partition-table write.
- **TPM2 + Limine**: Modulix uses `boot.loader.limine`. TPM2 unlock requires `boot.initrd.systemd.enable = true` and `crypttabExtraOpts = [ "tpm2-device=auto" ]`, both of which `ExtraConfigTask` now writes into `luks.nix`. `EnrollTpmTask` runs `systemd-cryptenroll --tpm2-device=auto` for real. **Still unverified**: treat TPM2 unlock as unconfirmed until an encrypted install has actually been booted on real hardware or a VM with Limine.
- **The EFI boot entry is the installer's, not nixpkgs'** — and the half that belongs to the
  installed system lives in **mxpkgs**, not here: `mxpkgs/modulixos/boot.nix` sets
  `boot.loader.efi.canTouchEfiVariables = lib.mkMxDefault false` on purpose. nixpkgs'
  `limine-install.py` hardcodes the label `Limine` (no option exists) and re-creates its
  entry at the head of `BootOrder` on **every** `nixos-rebuild`, so a rename done once would
  drift back or pile up duplicates. With the NVRAM write off, `EfiEntryTask` creates the
  single `ModulixOS` entry at install time and nothing ever rewrites it — the label
  persists, and the boot order is imposed once rather than re-asserted at each boot. Two
  consequences to keep in mind: `boot.loader.limine.efiInstallAsRemovable` is a
  `!canTouchEfiVariables` default, so limine now also installs the firmware-probed fallback
  `ESP/efi/boot/BOOTX64.EFI` (which is what `EfiEntryTask` points the entry at, and a disk
  that survives an NVRAM wipe); `limine.conf` is unaffected, it stays at
  `ESP/limine/limine.conf` either way. The `canTouchEfiVariables is set to false …` warning
  `limine-install.py` prints during the install is expected. **The installed system resolves
  mxpkgs from `github:Modulix-OS/mxpkgs` with no rev** (`modulix-core-utils/src/init.rs`),
  so this half only takes effect once the mxpkgs change is pushed — a local checkout does
  nothing for an install.
- **Hibernation**: swap size ≥ RAM, `boot.resumeDevice` set, and if encryption is enabled the swap must live inside the LUKS container.
- **Root-in-kiosk**: acceptable here (throwaway live ISO, single app), but every `Command::new` must pass arguments as an array — never a shell — and device paths must be validated against the udisks2 enumeration, never built by concatenating user input.
- **WebKitGTK captive-portal window runs as root**: step 6's portal window (`widgets/portal_window.rs`) embeds a third-party-controlled web page (any café's captive portal) in a process running as root inside the kiosk. Mitigations in place: an ephemeral `webkit6::NetworkSession` (no persisted cookies/cache), `enable_developer_extras`/`javascript_can_open_windows_automatically` disabled, and WebKitGTK 6.0's own bubblewrap sandbox for the web process (`set_sandbox_enabled` no longer exists in the 6.0 API — the sandbox is always on). Not a regression versus the Calamares-based flow it replaces (the patched QML network page did the same), but verify on the real ISO that user namespaces are available, or WebKit's web process refuses to start. `webkitgtk_6_0` comes in as a `buildInput` of `nix/package.nix`, so the wrapper carries it.
- **The partitioning step's shell-outs need runtime packages too** (all carried by `nix/kiosk-module.nix` and `nix/package.nix`'s `--prefix PATH`): `ntfs3g` (`ntfsresize`, NTFS shrink
  preflight), `gptfdisk` (udisks2's GPT backend), `util-linux` (`mkswap`/`swapon`, `mount`/
  `umount`), `cryptsetup` (LUKS2), `tpm2-tools` (`systemd-cryptenroll`), `efibootmgr`
  (`EfiEntryTask`, and the only way to inspect or repair the boot record from the tty1
  recovery shell), `udisks2` (the
  `org.freedesktop.UDisks2` D-Bus service itself), `gparted` (manual mode's partition
  editor, needed only to resize or delete a partition the user already created — carving a
  new one out of free space happens in-app, see step 6 above), plus `git` (the config repo),
  `e2fsprogs` (`chattr`, for `ExtraConfigTask`'s unseal/reseal) and `sway` (`swaymsg`, for
  the keyboard step). Also `nixos-install-tools` (`nixos-generate-config` for
  `InitConfigTask`, plus `nixos-install` and `nixos-enter`) and `pciutils`/`usbutils`/`cpuid`
  (core-utils' `DriverConfig::new()`, reached from the same task). On the ISO, the session
  unit gets all of this — including the default-on `nixos-*` tools and `nix` itself — through
  `path = [ config.system.path ]` on `systemd.services."modulix-installer-session"`, not
  through the wrapper; `nix/package.nix`'s `postFixup` only adds a `--suffix PATH` with the
  same tools so `cargo run` / `nix run .#default` on a dev box still resolves them.
  `nix.settings.experimental-features` must include `nix-command` and `flakes`, or
  `nixos-install --flake` refuses to run.
- **GParted is a second toplevel window**: the manual partitioning mode launches it
  full-screen over the running disk (`steps/partitioning.rs`'s `run_gparted`), unmounting
  every partition on the target disk first since GParted refuses a mounted one. This is one
  of the reasons the session is `sway` and not `cage` (which only ever shows one window);
  `nix/kiosk-module.nix` fullscreens it by `app_id`. Still worth checking on a real ISO,
  same as the WebKitGTK captive-portal window above.
- **A failed backend connection must never silently become a simulation**: `main.rs` used to
  fall back to `Backends::fake()` with only an `eprintln!` nobody sees under the kiosk, so the
  installer simulated a whole install and reported success. There is now **no simulation mode
  at all** — no `--fake`, no fake backend impls, no `is_fake` gate, no "Simulation" banner to
  forget. `Backends::new()` failing means `app::build_fatal_window` takes over: a fullscreen
  error page with the full, selectable reason and a single Quit button, and a non-zero exit.
- **A failed install must stay diagnosable after a reboot**: the live ISO root is a RAM
  overlay and journald on it is volatile, so `engine::install_log` mirrors every
  `ProgressEvent` to `/var/log/modulixos-install.log`, to `stderr` (→ journald, tty2 is still
  a getty), and — via `copy_to_target`, guarded on `/mnt` really being a mount point — to
  `/mnt/var/log/modulixos-install.log`, the only copy that survives. On failure the tee does
  the copy because `PostInstallTask` never runs; on success `PostInstallTask` does it just
  before unmounting.
- **Software rendering under a hypervisor**: `nix/kiosk-module.nix`'s `startSession` wrapper
  exports `WLR_RENDERER=pixman`, `GSK_RENDERER=cairo` and
  `WEBKIT_DISABLE_DMABUF_RENDERER=1` when `systemd-detect-virt` succeeds, plus
  `WLR_NO_HARDWARE_CURSORS=1` unconditionally. Without them, virtio-gpu with 3D acceleration
  off (the common libvirt setup) leaves GTK on a Vulkan/`opengl` GSK renderer and wlroots on GLES2
  over llvmpipe: tearing, artefacts and a trailing cursor. Real hardware starts on the
  accelerated paths, but `startSession` deliberately does **not** `exec` sway: if sway exits
  non-zero it retries once with the same software variables, because a machine whose GL
  driver cannot serve wlroots is otherwise indistinguishable from a failed boot (VT back in
  text mode, nothing ever drawn). `nix/iso.nix` also loads
  `virtio_gpu`/`qxl`/`bochs`/`vmwgfx` from the initrd so KMS is up before
  Plymouth instead of switching mode under it — no real-hardware DRM driver is in there, so
  on physical machines KMS still comes up in stage 2 via udev.
- **The DRM handover is a race nothing else orders**: on real hardware udev loads the actual
  driver in stage 2, and amdgpu's probe evicts the framebuffer device the initrd bound
  (`aperture_remove_conflicting_pci_devices`) before its own card appears — a window with no
  `/dev/dri/card*` at all. sway started inside it logs `Found 0 GPUs, cannot create backend`
  and exits, which is indistinguishable from a dead boot. `systemd-udev-settle` is gone and
  `multi-user.target` is *not* ordered before the session, so the module's `ExecStartPre`
  (`waitForGpu`) does `udevadm settle` plus a 15 s poll for a render node, and always exits
  0 — a machine with no render node at all (BMC GPUs, the EFI framebuffer stub) must still
  reach the pixman path, which `startSession` selects up front when `/dev/dri/renderD*` is
  missing. This is the one failure mode that structurally cannot reproduce in the VM, where
  `virtio_gpu` binds in the initrd and is never replaced.
- **A kiosk failure must be visible on the physical screen**: with `getty@tty1` masked and
  the unit logging to the journal only, all four failure paths (`ConditionPathExists` unmet,
  `StandardInput=tty-fail` refused because plymouthd still owns tty1, sway exiting non-zero,
  the installer crashing) rendered as the exact same black VT with a blinking cursor. Hence,
  in `nix/kiosk-module.nix`: `StandardOutput`/`StandardError` are `journal+console`,
  `TTYVTDisallocate` is `no` (the `yes` cage uses reallocates the VT on stop and wipes the
  message), the unit is ordered `After=plymouth-quit-wait.service` (not just
  `plymouth-quit.service`, which merely *asks* plymouthd to stop), and
  `OnFailure=`/`OnSuccess=modulix-installer-fallback.service` prints `systemctl status`, the
  last 60 journal lines and `ls /dev/dri` on tty1, then hands over a root `agetty`
  (`OnSuccess` too: the installer has no reboot button, so quitting it normally also drops
  to a bare VT). The VT-existence check is `AssertPathExists`, **not**
  `ConditionPathExists` — an unmet condition makes systemd *skip* the unit and call that a
  success, so `OnFailure=` would never fire. `runInstaller` touches
  `/run/modulix-installer.started` and writes the installer's exit status to
  `/run/modulix-installer.status`; `startSession` reads both back, since sway `exec`s the
  installer and those files are the only way its fate can reach the unit. The `.started`
  flag is what gates the software retry: once the compositor has come up, a second attempt
  would re-run a destructive pipeline rather than fix a renderer.
