# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

**All documentation in this repo — this file, `README.md`, doc comments, commit messages — is
written in English**, regardless of the language a conversation about the codebase happens in.

## What this is

`modulixos-installer` is the installer for Modulix OS (NixOS-based distro): a single **fullscreen Rust + GTK4/libadwaita app** running as root inside a minimal `sway` kiosk session (Windows-installer style), replacing the older Calamares-based flow.

**Current state: the whole wizard plus a real, destructive install run end to end** — partition → encrypt → format → mount → TPM2 enroll → write the NixOS configuration → `nixos-install` → set passwords → release the target. Steps 7-8 (user, desktop environment) still have a minimal UI; step 9 is a full multi-select application grid. All their answers really reach the installed system. Caveat on "end to end": that was only ever true of an _unencrypted_ install, and for two reasons. The removed `luks.nix` defined `boot.initrd.luks.devices."modulixroot".device` a second time and made the encrypted path fail at evaluation; and `EncryptTask`/`EnrollTpmTask` handed `cryptsetup` the udisks2 **object path** out of `PipelineState` instead of a `/dev` node, so `luksFormat` died with exit status 4 long before evaluation was reached. Both are fixed, and `CryptsetupBackend::require_device_node` now refuses an object path with an error naming it. Nobody has booted an encrypted install yet.

**The installed system's configuration belongs to mxpkgs, not to this repo.** Anything that
describes how Modulix OS behaves once installed — bootloader, limine, kernel, desktop,
defaults — goes in `mxpkgs/modulixos/`. The installer only generates what is specific to
_this_ machine and cannot be known in advance (hostname, users, the encrypted root, the
application pack) — and it generates all of it _through_ `modulix-core-utils`, into that
crate's own files (`fstab.nix`, `package.nix`, `module.nix`), never into files of its own
invention. So a limine or `boot.loader` change is an mxpkgs change, even when the reason for it lives here.
Caveat: the generated `flake.nix` resolves `github:Modulix-OS/mxpkgs` with no rev, so such a
change reaches installs only once pushed.

Two concrete consequences of that rule, both added for the step-9 application grid: the
`programs.studio.kdenlive`, `programs.creativity.{krita,gimp,inkscape}`,
`programs.dev.{vscode,git,distrobox}` and `programs.dev.ue5` (Unreal Engine 5)
modules the grid enables live in **mxpkgs** (`modules/programs/…`, `modules/services/…`, registered
in `modules/index.json` and `index.fr.json`), as does the controller support the
`programs.games` group now applies to every launcher. The GNOME app grid is arranged by the
**installer**, not by mxpkgs: `mxpkgs/modulixos/desktop/gnome/custom.nix` used to declare
`org/gnome/desktop/app-folders` folders and an `org/gnome/shell` `favorite-apps` list in
`programs.dconf.profiles.user.databases`, and that is exactly the wrong layer for them —
those are system *defaults*, and GNOME Shell deletes a folder or a pinned app by **resetting**
the user-db key, which then falls straight back through to the default and the folder
reappears. `engine::tasks::GnomeGridTask` writes the same folders and favourites into the
created user's **own** database instead (`~/.config/dconf/user`, once, at install time), so
every one of those values is the user's from the first login and deleting one sticks.
Because the installer writes `mx.<module>.enable` without validating it against the index,
an mxpkgs module that is missing or unpushed fails `nixos-install` _after_ the disk has been
partitioned and formatted. New cards therefore need their mxpkgs half pushed first.

**mxpkgs has no ISO/installer module** — `mxpkgs/installer/default.nix` does not exist and never did. The live ISO is defined here, in `nix/iso.nix`, on top of nixpkgs' `installation-cd-minimal.nix`, and imports `modulixos/branding.nix`, `modulixos/boot.nix` and `modulixos/options` from mxpkgs (a flake input) for the Modulix identity and boot configuration. Moving the ISO into mxpkgs is a later step, not a prerequisite.

`branding.nix` carries **no** boot configuration: everything Modulix about the boot lives in `mxpkgs/modulixos/boot.nix`, which `nix/iso.nix` **imports directly** rather than copying — along with `mxpkgs/modulixos/options`, which declares the `mx.mode.server.enable` that `boot.nix` reads (importing `boot.nix` alone fails with `error: attribute 'mode' missing` as soon as anything forces `boot.plymouth.enable`). The import is guarded by two lines, and those are the only reason the copy ever existed:

- `mx.bootloader.enable = false` — the option defaults to `true`, which would enable limine next to the ISO image builder. Nothing _conflicts_ (`iso-image.nix` only does `boot.loader.grub.enable = mkImageMediaOverride false` and never claims `system.build.installBootLoader`), so limine would evaluate and build; it would just be dead weight in a closure whose boot actually goes through the isolinux/grub-efi written into the image.
- `boot.tmp.useTmpfs = lib.mkForce false` — `boot.nix` sets it `true`, and the live ISO's root is already a tmpfs overlay.

Everything else comes from `boot.nix` unchanged and must **not** be restated here: the `boot.plymouth` block (theme `modulix` = a `bgrt` clone with the Modulix logo as watermark, so on real hardware the firmware vendor logo shows through by design), the quiet-boot `boot.kernelParams` — including `iommu=pt`, which mxpkgs carries as a fix for some AMD CPUs — `consoleLogLevel`, `initrd.verbose`, and `initrd.systemd.enable` (already `true` on the ISO before the import, so this is not a change). `isoImage.splashImage`/`efiSplashImage` read `config.mx.bootloader.splash.image` for the same reason. Note `vt.global_cursor_default=0` is in the kernel-params list: a failed kiosk start leaves a completely black VT, which is why the tty1 recovery shell below is not optional.

The only behavioural deltas the import introduces are inert: `systemd.network.wait-online.enable = false` (the ISO runs NetworkManager, `systemd.network.enable` is `false`) and a masked `systemd-udev-settle` unit in both stages (the unit no longer exists upstream in this nixpkgs; `waitForGpu` in `nix/kiosk-module.nix` calls `udevadm settle` itself regardless).

One duplication is left on purpose: `system.nixos.label` recomputes `branding.nix`'s `asciiCodeName` `replaceStrings` table, because mxpkgs exports no slug helper — removing it needs an mxpkgs change.

## Commands

```bash
cargo fmt && cargo clippy -- -D warnings   # mandatory before any commit (global rule)
cargo test                                  # pure logic only — swap sizing, partition planner,
                                             # zone.tab/evdev.xml parsing, step-registry is_relevant(),
                                             # plus tests/po_coverage.rs (gettext catalogs)
cargo run                                   # no CLI flags: connects the REAL backends
cargo run -- --fake-disks=multi-disk        # debug build only: synthetic disks, see below
cargo run -- --fake-error=list              # prints the --fake-error= catalog and exits
```

**`cargo run` with no flags enumerates your real disks — never press Install.** udisks2 and
NetworkManager answer on the system bus, so steps 1-9 are safe to click through, but the
confirmation dialog is the only thing between the wizard and your partition table.

**A debug build also accepts `--fake-disks=<scenario>[,<scenario>...]`,
`--fake-error=<id>|list|random` and `--fake-instant`** (`src/debugsim/`), so the 9 steps and a
full install run can be exercised without touching a real disk. `--fake-disks=` composes one or
more of `backend::disk::scenario::DiskScenario`'s 11 fixtures (aliases `fragmented` →
`Fragmented`, `half-partitioned` → `FreeSpaceTail`) into a synthetic `/dev/fakeN` disk set;
`--fake-error=` injects exactly one failure from a fixed catalog (`cargo run --
--fake-error=list` prints it) at a specific pipeline stage, so every failure path the progress
page, the QR error report and the install-log copy exist to handle can be reproduced on demand;
`--fake-instant` zeroes the fake install's sleeps. This is gated by `#[cfg(debug_assertions)]`
end to end — none of it is compiled into a release binary (the live ISO's), and it is never a
fallback: only `disk`/`crypt` are faked (`backend::Backends::with_fake_disks`), network/locale/
accessibility stay real, and a release binary refuses to start at all if handed any `--fake*` or
otherwise-unrecognized flag (routed through the same fatal-error window a real backend
connection failure uses, since the kiosk has no terminal to print to). See
`src/debugsim/mod.rs`'s doc comment for why this is not a repeat of the `--fake` mode `eb6840b`
removed for silently degrading a real connection failure into a reported success.

Exercising the wizard against **real hardware/VM disks** end to end — booting the result, TPM2
unlock, hibernation resume, the EFI boot entry — is still what the VM is for:

```bash
nix build .#iso     # the live ISO (nix/iso.nix) — the result carries iso/*.iso plus a
                    # `sha256sum -c`-ready iso/*.iso.sha256 (nix/iso-checksum.nix)
nix run  .#vm       # boot that ISO in qemu: UEFI, scratch 40 GiB disk, swtpm, virgl on
MODULIXOS_VM_GL=0 nix run .#vm   # same, virtio-gpu without 3D — the libvirt-like case the
                                 # kiosk's software-renderer fallback exists for
MODULIXOS_VM_RES=3840x2160 nix run .#vm   # force the guest resolution (xres/yres on the
                                 # virtio GPU, qemu window zoom-to-fit) — the only way to
                                 # exercise the HiDPI auto-scale below
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

| Trait            | Real impl                      | Role                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| ---------------- | ------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `DiskBackend`    | `Udisks2Backend` (zbus)        | enumerate disks/partitions/free space, create/delete/resize/format/mount                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `NetworkBackend` | `NetworkManagerBackend` (zbus) | ethernet, Wi-Fi scan/connect, `Connectivity` → captive-portal detection                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `LocaleBackend`  | `SystemLocaleBackend`          | locales (`locale.gen`), timezones (`zone.tab` lat/lon for the map), keyboard layouts (`evdev.xml` from xkeyboard-config)                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `A11yBackend`    | `OrcaBackend`                  | narrator (speech-dispatcher/orca) — the only method that does anything in the kiosk session; the other four are best-effort GNOME a11y `gsettings` calls (silently `Ok(())` if `gsettings` isn't on `PATH`, real errors otherwise), useful only on a dev machine under a full GNOME session. What actually applies live to the installer itself (high contrast, large text) is `src/a11y.rs`'s `A11ySettings`, not this trait                                                                                                                |
| `CryptBackend`   | `CryptsetupBackend`            | `cryptsetup luksFormat`, `systemd-cryptenroll --tpm2-device` (± `--tpm2-with-pin`). Takes `/dev` nodes only — `require_device_node` rejects a `PipelineState` object path instead of letting `cryptsetup` fail opaquely. `systemd-cryptenroll` gets its two secrets through files in a `0700` tmpfs directory under `/run` (`--unlock-key-file` for the existing passphrase, the `cryptenroll.new-tpm2-pin` credential for a new PIN), never argv and never stdin: it asks through `ask_password`, which does not read the pipe it is handed |

`Backends::new()` is fallible and a failure is **fatal** — `main` shows a fullscreen, selectable error page and exits non-zero rather than degrading into a simulation. A debug build's only second implementation of any of these is `src/debugsim/`'s `FakeDiskBackend`/`FakeCryptBackend`, behind the explicit `--fake-disks=`/`--fake-error=` flags (see "Commands" above) — `NetworkBackend`/`LocaleBackend`/`A11yBackend` have none, real or fake, beyond the table above.

### `trait Task` (`src/engine/mod.rs`) — install pipeline

```rust
#[async_trait]
pub trait Task {
    fn label(&self) -> String;
    fn weight(&self) -> u32;    // for the progress bar
    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()>;
}
```

Pipeline: `PartitionTask` → `EncryptTask` → `FormatTask` → `MountTask` → `EnrollTpmTask` → `InitConfigTask` → `NixosInstallTask` → `SetPasswordsTask` → `FlatpakRemoteTask` → `GnomeGridTask` → `PostInstallTask`.

- `EncryptTask` must run right after `PartitionTask` and before `FormatTask`/`MountTask` — `luksFormat`/`luksOpen` need the bare partition, not one already `mkfs`'d and mounted on `/mnt`. It encrypts **two** containers when the install has swap: root as `modulixroot` and swap as `modulixswap` (`engine::LUKS_MAPPER_NAME`, `::LUKS_SWAP_MAPPER_NAME`), both with the same passphrase, then rewrites `root_partition`/`swap_partition` to the mappers so the later stages mkfs and mount the decrypted devices. A plaintext swap beside an encrypted root would hold the root key in a page-out; a second container rather than LVM-inside-one keeps the partition layout, the preview and `engine::plan` untouched. `EnrollTpmTask` enrols both — enrolling root alone would unlock it silently through the TPM and then prompt for the swap, nothing having put a passphrase in the kernel keyring. `PostInstallTask` closes both.
- `InitConfigTask` calls `modulix_core_utils::init::init` (synchronous, so `spawn_blocking`) with `config_dir = /mnt/etc/modulix-os`, which writes and commits the config repo but **deliberately skips the rebuild** — it holds core-utils' skip-rebuild lock precisely so the installer drives the build itself. **It is the only writer of that repo**: `InitParams` carries `packages` → `package.nix`, `modules` → `module.nix` (one `mx.<name>.enable = true` per dotted name from mxpkgs' `modules/index.json`) `luks` → one `boot.initrd.luks.devices` entry per container in `fstab.nix` and `resume_device` → `boot.resumeDevice`, so everything lands inside core-utils' single seed transaction, written by core-utils' own `install_package`/`install_module`/`filesystem` writers and committed once. Nothing after it touches the configuration, which is why the installer no longer unseals `configuration.nix` with `chattr`, splices `imports` by hand, or runs `git commit` itself.
  There used to be an `ExtraConfigTask` doing exactly that, with a `luks.nix` and an `apps.nix` of its own invention. Both were wrong: `apps.nix` spelled packages `with pkgs; [ firefox ]` where `install_package` reads and writes `pkgs.firefox`, so a post-install `mx` would not have found them; and `luks.nix` re-declared `boot.initrd.luks.devices."modulixroot".device`, which **`nixos-generate-config` already emits** (it reads `/sys/class/block/<dm>/dm/name`, and `filesystem::fstab_module` copies that block into `fstab.nix` verbatim) — two definitions of one `types.str` option make the NixOS module system fail even at equal values, so an encrypted install could never evaluate.
- `NixosInstallTask` runs `nixos-install --root /mnt --flake /mnt/etc/modulix-os#default --no-root-password` with both streams piped, forwarding each line as `ProgressEvent::Log`. This is why the pipeline does not go through core-utils' `rebuild_config`, which inherits stdout and could never feed the log view.
- `SetPasswordsTask` pipes `root:…`/`user:…` into `chpasswd` under `nixos-enter`, so no password — hashed or not — ever reaches the Nix store.
- `FlatpakRemoteTask` runs `flatpak remote-add --if-not-exists flathub …` under `nixos-enter`, so the app store already has its remote on the first session even if that first boot has no network. It duplicates mxpkgs' `mx.services.flatpak` `flatpak-repo` unit on purpose — `--if-not-exists` makes the overlap a no-op. Guarded on the **base-pack card being ticked**, which is the only one enabling `mx.services.flatpak` and therefore the only reason `flatpak` is in the installed system at all. It used to be guarded on the binary existing at `/mnt/nix/var/nix/profiles/system/sw/bin/flatpak` instead, which could never be true — see the host-side-probe entry under "Security / data-loss risk points" — so the stage logged `skipped (flatpak is not installed)` on every single install. Best-effort from end to end (the mxpkgs unit is the fallback), so it never fails an install already on disk.
- `GnomeGridTask` seeds the primary user's dconf database with the app-grid folders and the dash favourites mxpkgs used to declare as system defaults — see the app-grid paragraph above for why that layer was wrong. It generates a dconf **keyfile** into `/mnt/tmp/modulix-dconf-seed/` and turns it into `/home/<user>/.config/dconf/user` with `dconf compile` under `nixos-enter`, then `chown -R <user>:` that `.config`: `dconf compile` is a pure keyfile → gvdb file operation, where `dconf load` would need the session bus the chroot has not got, and a root-owned `.config` would break every app on the first login. The chown is **unconditional** — `write_seed` has already created `.config` by the time a failed compile is reported, and a root-owned one is the worst outcome this stage can leave — and it names no group, so coreutils takes the user's login group instead of assuming `users`. Folders are **category-driven** (`categories=` plus `translate=true` on gnome-menus' `.directory` names, so GNOME localizes them itself and an app installed months later is filed by itself) and only those at least one ticked card justifies are written. Everything about what the target contains is read off **the wizard's answers**, never off the target's files: choosing GNOME is what puts `dconf` and gnome-menus' `desktop-directories` there (so `translate=true` always resolves, and there is no English fallback any more), and `FAVORITE_CANDIDATES` pairs each dash favourite with the card that installs it — the base pack for Firefox/Thunderbird/Nautilus, nothing for GNOME Software, which mxpkgs' `mx.gnome.software` brings in with the desktop itself. Four guards, each logged and each returning `Ok(())`: not GNOME, no username, `/mnt/home/<user>` missing (seeding into a missing home would leave it root-owned), or the user already having a dconf database (a kept `/home` partition — never rewrite tuned settings). Best-effort end to end: an unarranged grid is cosmetic, never a reason to fail an install already on disk.
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
                        user/ (mod.rs, username.rs, hostname.rs), app_pack/ (mod.rs),
                        card_layout.rs (card-grid geometry shared by steps 8 and 9)
  finish/               summary.rs, progress.rs, qr_report.rs (error-report payload for
                        the failure page's QR code) — post-step-9 review (recomputes
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
                        mount.rs, enroll_tpm.rs, init_config.rs, nixos_install.rs,
                        set_passwords.rs, flatpak.rs, gnome_grid.rs, post_install.rs)
  widgets/              timezone_map.rs, disk_bar.rs, partition_editor.rs, password_entry.rs,
                        ap_row.rs, wifi_dialog.rs, portal_window.rs, slideshow.rs,
                        qr_code.rs (QR matrix + its `DrawingArea` and dialog),
                        dropdown.rs (size_dropdown_to_widest + enable_string_search —
                        every long `DropDown` gets substring type-ahead; `selected()`
                        stays an index into the unfiltered model, GTK resets the search
                        filter before reporting it)
data/
  icons/*.svg           one symbolic icon per step — GTK4 symbolic constraint: must be
                        fill-only (no `stroke`, no `fill="none"`); GTK recolors `fill`
                        attributes via the injected `<style>` but never `stroke`, so a
                        stroked path stays the icon's original color instead of following
                        the theme (rings/outlines are drawn as an evenodd fill between two
                        concentric shapes instead)
  screenshots/          gnome.webp, plasma.webp, lxqt.webp
  slides/*.svg          install-slideshow pictograms — square and **background-free**
                        (`slideshow.rs` draws them centred in a fixed `SLIDE_ICON_PX` box
                        over the page background; a baked-in rect would show as a card)
  resources.gresource.xml
nix/
  iso.nix               live ISO: installation-cd-minimal + mxpkgs branding/boot/options + kiosk
  iso-checksum.nix      thin wrapper around nixpkgs' `system.build.isoImage`: re-exports
                        its `iso/` directory as symlinks and adds one `*.iso.sha256`
                        beside each image. This is what `packages.iso` points at
  kiosk-module.nix      the sway kiosk session (systemd unit + PAM + tty1 recovery
                        shell), ISO runtime deps, and the session's input config —
                        `input type:touchpad { scroll_factor 0.4 }`, because libinput's
                        default overshoots the long lists by screens per flick (compositor
                        level, so it covers GParted too; no effect under `cargo run`)
  package.nix           naersk derivation; wrapGAppsHook4 wiring
  vm.nix                `nix run .#vm` qemu runner (UEFI + swtpm + scratch disk)
build.rs                glib_build_tools::compile_resources
```

### i18n: `LANGUAGE` drives messages, `setlocale` drives formatting

`setlocale(LC_ALL, code)` silently no-ops (`NULL`, locale left untouched) for any locale glibc
hasn't _generated_ on the running machine — which is most of the ~470 entries the language-step
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
- **The startup language is always English**, never the host locale: `i18n::init()` seeds
  `LANGUAGE` from `i18n::DEFAULT_LANGUAGE` (`en_US.UTF-8`) and only uses `setlocale` — the
  `DEFAULT_LANGUAGE` spellings, then `FALLBACK_LOCALES`, then the host environment — to get
  `LC_MESSAGES` off `C` for formatting. This is also what pre-selects the language step's
  dropdown (`steps/language.rs` matches `current_language_code()` with `normalized_eq`), hence
  `DEFAULT_LANGUAGE` being a _full_ locale code: a bare `"en"` matches none of
  `list_locales()`'s territory-carrying entries and the dropdown would fall back to index 0.
- `po/` ships `fr.po`, `es.po` and `de.po` — French, Spanish, German and English (gettext's
  `msgid` fallback) are the four languages that visibly differ today, even though the dropdown
  lists every glibc locale. Catalogs are named by language only (`fr`, `es`, `de`), which covers
  every territory variant because `i18n::language_env_value` cascades `de_AT.UTF-8` → `de_AT:de`.
  Nothing in the build system enumerates them: `build.rs::compile_translations` and
  `nix/package.nix`'s `postInstall` both glob `po/*.po`, so a new language is one file.
  `nix/kiosk-module.nix`'s `i18n.supportedLocales` is the one place that does list them, and only
  so that _formatting_ follows the choice (messages need no generated locale).
  There is no extraction script: a new msgid goes into `po/modulixos-installer.pot` _and_ every
  `po/<lang>.po` by hand (`msgcat --use-first --sort-output --no-location` keeps them ordered),
  and `xgettext` would need `--keyword=tr` since `tr` is a plain function, not a macro.
  `tests/po_coverage.rs` is the guard rail for that hand-maintenance: `cargo test` fails if a
  `tr("…")` literal in `src/` has no msgid in the `.pot`, if a `.pot` msgid is missing from
  `fr`/`es`/`de`, or if any `msgstr` is left empty. It only sees _literal_ arguments, so the
  msgids reaching `tr` through a constant (`Task::label`, `widgets::slideshow`,
  `config.rs`' desktop descriptions) are still on their own — the pot is a superset of the
  literals, never an equal. Ten msgids, including the user step's whole `Computer name`
  field, had silently fallen back to English before it existed.
- `src/welcome.rs`'s `GREETINGS` is the one place that is **not** gettext-driven: the pre-wizard
  page cycles through every shipped language at once, so adding a catalog means adding a
  `Greeting` entry there too. Each entry also carries the `locale` it is written in, because
  **the start button adopts the language of the greeting then on screen**: it hands that code to
  `steps::SelectLanguageHook`, which `steps::build_registry` fills from
  `LanguageStep::select_hook` — the programmatic twin of the dropdown's `notify::selected`
  handler, so the message language, the retranslate pass, the keyboard default, the language
  step's own dropdown and therefore `InstallConfig::language` all move together (`app.rs` calls
  it _before_ swapping the root stack, so the wizard's first frame is already translated).
  English is first on purpose — it is `i18n::DEFAULT_LANGUAGE`, so clicking the first greeting is
  a no-op the hook returns out of early, and a `#[cfg(test)]` test in `welcome.rs` pins that
  invariant. The rotation is unconditional and must stay so: pausing it while the button is
  hovered _or focused_ froze it for the whole session, because the button is the page's only
  focusable widget and GTK focuses it on the first frame.
- **A second text domain is bound: `xkeyboard-config`** (`i18n::tr_xkb`). `evdev.xml` carries
  English `<description>` text only — no `xml:lang` variants — and xkeyboard-config ships the
  translations as catalogs whose msgids _are_ those English strings, which is how GNOME localizes
  the same list. The catalog directory comes from `MODULIX_XKB_LOCALE_DIR` (set by `nix/package.nix`
  and the devShell), falling back to the `share/locale` sibling of `MODULIX_DEV_EVDEV_XML` and then
  to the system prefixes; not finding it just leaves layout names in English. The
  `_nl_msg_cat_cntr` bump in `set_language` is global, so this domain follows the language switch
  like ours.
- `Task::label()` returns an **untranslated msgid on purpose**: tasks run on tokio worker threads
  and the `LANGUAGE` slot is read from the GTK main thread only. `finish::progress` calls `tr()` on
  arrival, so the install log stays single-language while the status line follows the UI.

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
4. **Keyboard** — layout + variant from `evdev.xml`, live typing test area, applies the layout
   immediately. Names are localized through xkeyboard-config's own catalogs (`i18n::tr_xkb`, see
   the i18n section) and the list is sorted by the _displayed_ name, so a language change re-sorts
   and rebuilds both dropdowns — selections are restored by code, never by index. The default
   layout follows the language step through `steps::LanguageHook` →
   `KeyboardStep::locale_hook` → `backend::locale::keyboard_default::layout_for_locale` (curated
   exceptions, then territory, then language code, then `us`), and stops following it as soon as
   the user picks a layout by hand. A layout does not necessarily start on its bare form:
   `keyboard_default::DEFAULT_VARIANTS` maps a layout code to the variant it should default
   to, which is `fr` -> `oss` ("French (alt.)", "Français (variante)") and nothing else today
   — the bare `fr` map is the legacy one. The default is applied wherever the selected layout
   changes, so it covers both the language hook and a hand-picked layout, and it silently
   degrades to "Default" if this `evdev.xml` does not offer the listed variant.
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
   resize or delete an _existing_ partition. Formatting is
   forced for `/`, `/boot`, and swap (`engine::plan::format_is_forced`) — only `/home` can
   be kept as-is. The editor collects `Vec<plan::ManualItem>` (`Existing` assignment or
   `New` creation) and preserves the user's assignments/pending creations across a
   `set_data()` reload (e.g. after returning from GParted) — all validation lives in
   `plan::plan`, not the widget. Outside manual: swap (none / standard
   / hibernation, sized via `engine::sizing`) + LUKS2 encryption, which covers the swap as
   well as the root (one container each) + TPM2 (with or without
   PIN). `backend::disk::scenario::DiskScenario` is a `#[cfg(test)]`
   fixture set driving `engine::plan`'s scenario matrix, so every mode/blocker combination is
   covered by a test rather than by hand.
7. **User** — primary user, computer name (`networking.hostName`, default `modulixos`,
   validated as an RFC 1123 label by `steps::user::hostname`); root gets the same
   password. Neither password is written into the configuration — see `SetPasswordsTask`.
8. **Desktop environment** — gnome / plasma / lxqt, with description and screenshot. No
   outer "Next" button on this page (`Step::shows_next` returns `false`): picking a card
   opens a zoom dialog whose own "Choose this desktop environment" button both commits the
   selection and advances straight to the **applications** step, via the `AdvanceHook`
   the app wires up (`steps::new_advance_hook`) — see `src/app.rs`.
9. **Activities** — a **multi-select grid**, three columns wide at any usable window size
   (`src/steps/app_pack/mod.rs`): nine tick boxes — a 3x3 grid — only "Base pack" ticked by
   default.
   Unticking everything is legal and means a bare system — that is what replaced the old
   "No applications" card, and the page says so in a hint line that only shows while nothing
   is ticked. Each card is a `gtk::ToggleButton` (so GTK publishes
   `AccessibleRole::ToggleButton` + `State::Pressed` for Orca by itself) with a
   `can_target(false)`/`focusable(false)` `gtk::CheckButton` overlaid as the corner tick;
   selection state lives in the toggles only, and `commit` reads it back into
   `config::AppSelection`. Unlike step 8, this page **does** use the outer "Next" button
   (`Step::shows_next` keeps its `true` default) — a card click no longer means "done", so
   `AppPackStep::new()` takes no `AdvanceHook`.
   The nine cards, in grid order (left to right, top to bottom — `AppCard`'s derived `Ord`
   **is** that order), and what each one writes:
   `Base` → `package.nix` (`AppCard::packages(de)`: `thunderbird` plus the per-DE list in
   `config.rs`, GNOME's including `gnome-disk-utility` and `gnome-tour`) **and** `module.nix`
   (`programs.utilities.web-browser.firefox`, `services.flatpak`, `services.printer`);
   `OfficeSuite` → `programs.desktop.onlyoffice` (the child the `programs.desktop` group
   declares in the index; the group's own `enable` auto-derives from it, so it is not
   enabled separately). The card deliberately names no suite in the UI — its subtitle is
   "Write documents, build spreadsheets and presentations" and its icon is Papers;
   `Modeling3D` → `programs.creativity.blender`.
   Second row: `VideoEditing` → `programs.studio.kdenlive` plus **one of two DaVinci Resolve
   editions**, picked by a radio row inside the card itself ("Free" / "Studio"):
   `programs.studio.davinci-resolve` or `programs.studio.davinci-resolve-studio`, never both.
   The two mxpkgs modules are mutually exclusive by construction — the free one's `config` is
   `mkIf (cfg.enable && !studioEnabled)` — and the Studio one additionally carries the
   Blackmagic licence-dongle udev rule, so writing only the picked one is both correct and the
   only thing `InitParams::modules` can express (it writes `mx.<name>.enable = true` and no
   option values, which is why an edition is a second module rather than a flag);
   `Graphics` → `programs.creativity.{gimp,krita,inkscape}` under one "Drawing and image
   editing" card; `Streaming` → `programs.studio.obs-studio`, titled "Recording and
   streaming" because OBS is as much a screen recorder as a streaming tool.
   Third row: `Gaming` → `programs.games.{steam,lutris,heroic,umu}` in one card — the
   launchers are not worth splitting, and mxpkgs' `programs.games` group (whose `enable`
   auto-derives from any child) is where the controller support common to all of them now
   lives: `hardware.xone`, `hardware.steam-hardware`, the `xpadneo` out-of-tree module, the
   DualSense touchpad `LIBINPUT_IGNORE_DEVICE` rules and `joystickwake`, ported from glf-os'
   gaming socle;
   `GameDevelopment` → `programs.dev.ue5`, which replaced the virtual-reality card
   (`programs.games.vr` still exists in mxpkgs, it is simply no longer reachable from the
   installer). mxpkgs installs the editor from an Epic archive into a Distrobox container of
   its own (`mx.programs.dev.distrobox` comes on by `mkMxDefault`), and like the Ubuntu
   container nothing is downloaded during `nixos-install`: the launcher's first run on the
   installed system creates the container and asks for the `.zip`;
   `Development` → `programs.dev.{vscode,git,distrobox}`, which replaced the racing-wheel
   card (`services.sim-hardware` still exists in mxpkgs and is still in `modules/index.json`
   — it is simply no longer reachable from the installer, only from the post-install store
   and the `mx` CLI). `programs.dev.vscode` installs **`pkgs.vscode-fhs`**, not `pkgs.vscode`:
   most marketplace extensions ship prebuilt native binaries (clangd, debug adapters,
   language servers) that cannot run against the store layout. It is unfree, which costs
   nothing here — mxpkgs' `modulixos/default.nix` already sets
   `nixpkgs.config.allowUnfree = lib.mkMxDefault true` — and it turns `programs.dev.git` on
   by `mkMxDefault`, since VS Code's SCM pane is dead without git. `programs.dev.git` goes
   through the NixOS `programs.git` module rather than the bare package, because
   `lfs.enable` is what writes the `filter.lfs` stanza into `/etc/gitconfig` — installing
   `git-lfs` alone leaves an LFS clone checked out as pointer files, which reads as a
   corrupt working tree. `programs.dev.distrobox`
   is the one card half that is **not** fully declarative: it enables rootless
   `virtualisation.podman` (which pulls `virtualisation.containers` in by itself, and needs
   no `/etc/subuid` work — `nixos/modules/config/users-groups.nix` already gives every
   `isNormalUser` an automatic 65536-id range) and ships `pkgs/mx-ubuntu-box.nix`, a
   launcher plus its `Categories=Development;` desktop entry, which creates the Ubuntu
   container on its **first run on the installed system**, guarded by
   `podman container exists`. The image is
   never pulled during `nixos-install`: the install must not depend on the network, and the
   user's rootless container store does not exist yet at that point. Three details of that
   script are load-bearing: `DBX_CONTAINER_MANAGER=podman`, so a machine that also enabled
   `mx.services.docker` cannot end up with a Docker container; `distrobox create --no-entry`,
   because distrobox generates a desktop entry of its own and the app grid would show two;
   and a `read -r` pause on every failure path, since a `Terminal=true` window closes the
   instant the script exits and an error message would only flash past. The image is fully
   qualified (`docker.io/library/ubuntu:latest`) because NixOS writes two unqualified search
   registries and podman stops on an interactive menu for an ambiguous short name. Podman
   next to `mx.services.docker` is fine — the only assertions in nixpkgs' podman module are
   on `dockerCompat`/`dockerSocket`, neither of which this module sets. The group's terminal
   default follows `config.mx.desktop` (`gnome-console`/`konsole`/`qterminal`, `null` on
   `cli`), which is also what guarantees the `Terminal=true` launcher has something to open
   in; reading `mx.desktop` from `modules/` follows `programs.games` and the Firefox module.
   The multi-module cards write **no group module**: the card enables each child module
   directly, and a group whose `enable` auto-derives (`programs.desktop`, `programs.games`,
   and now `programs.dev`, which carries the shared terminal and `htop`) switches itself on. `AppSelection` is a `BTreeMap<AppCard, usize>`
   whose derived key `Ord` is the grid order, so the union handed to `InitParams` is
   deduplicated and byte-stable — it is committed into a git repo on the target. The value is
   the **edition** the card is set to, and the mechanism is generic rather than
   Resolve-specific: `AppCard::variants()` returns a `&'static [CardVariant]` (`key` +
   the modules that edition adds on top of `AppCard::modules()`), empty for a card with no
   choice; any card that declares two or more gets the radio row, the summary suffix
   (`Video editing (Studio)`) and the module mapping for free, with
   `steps::app_pack::variant_label` holding the `tr` literals. The radios sit inside the
   card's `gtk::ToggleButton`, and nesting alone does **not** give them the click: the
   ToggleButton's gesture wins the sequence, so a click on a radio only flipped the card's
   tick. Each radio therefore carries a `gtk::GestureClick` in the **capture** phase that
   claims the sequence (cancelling the card's gesture) and calls `set_active` itself, since
   claiming also denies the radio's own internal gesture; keyboard activation never goes
   through a click gesture and is unaffected. The reverse direction is deliberate (a radio
   ticks its card, so picking an edition is one click), and that is why the default radio is made
   active _before_ its handler is connected — the opposite order to the card toggles, whose
   defaults are applied after, so that the indicator and the hint line settle through the
   same path a click takes.
   Two of the base pack's staples are module-provided rather than package-provided, because
   `environment.systemPackages` cannot express them: printing needs
   `services.printing.enable` plus a driver set (which is why `cups`/`gutenprint`/
   `system-config-printer` are not in `COMMON_BASE_PACKAGES` — they were installed but the
   service was never switched on), and Firefox needs its language packs, hunspell
   dictionaries, VAAPI preferences and GNOME search provider (which is why `firefox` left
   `COMMON_BASE_PACKAGES` too). The Flathub remote is registered twice over: by mxpkgs'
   `flatpak-repo` unit on every boot, and once at install time by
   `engine::tasks::FlatpakRemoteTask`, so the store is usable on the first session.
   Geometry is shared with step 8 through `steps::card_layout` (`CARD_SPACING`,
   `PAGE_MARGIN`, `grid_width`, `column_min_width`, `add_column_breakpoint`) but the card
   sizes are not: step 9 uses 220/420 px because nine cards go three rows deep and three
   step-8-sized cards never fit the kiosk's guaranteed logical width. Step 9's cards carry
   **no aspect ratio** either — their height is content-driven (a fixed `ICON_BOX_PX` icon
   block for a lone icon, `ICON_TILE_PX` tiles for a collage — bigger than half an icon on
   purpose, since halved icons read as clutter — built from `gtk::Image`s whose `pixel-size`
   is the same in both orientations, plus the labels). A collage of three tiles puts the odd
   one across both columns so it stays centred. A `gtk::AspectFrame` around the card letterboxes it inside a line sized by
   the tallest card, which reads as a vertical gap wider than the horizontal one; with the
   card filling its cell both gaps are exactly `CARD_SPACING`. Inside a card the text block
   is pinned to the bottom (`valign(End)` on a non-expanding box) and the icon takes every
   remaining pixel with `valign(Center)`, so it sits halfway between the top of the card and
   the top of the text — the base pack's card has one text line more than the others and
   `homogeneous` makes every card as tall as that one.

Then a summary (its "Install" button uses `suggested-action`, not `destructive-action` — the
confirmation dialog it opens stays destructive) → install-progress screen (an auto-advancing
`widgets::slideshow::Slideshow` fills the main area, collapsed-by-default log in a
`gtk::Expander` that auto-scrolls and opens itself on failure, progress bar pinned to the
bottom, pulsing while `nixos-install` runs) → done. When the pipeline ends, the status line and
progress bar are hidden and an outcome row takes their place in the same spot: outcome text on
the left, "Restart now" on the right. Failure keeps the status line, puts the (selectable,
scrollable) error and the log paths in that row, and replaces the restart button with two:
an "Error report QR code" button and a `destructive-action` "Quit the installer" — the
kiosk has no browser, no guaranteed network and no way to get text out, so the report
leaves by phone camera, and leaving a failed install is the same reboot the success path
offers (`wire_reboot_button` is shared, same remove-the-medium confirmation, same
`sync(2)` + `reboot(2)` — not `systemctl reboot`, which the dialog's own instruction makes
impossible: the live store lives on the USB drive, so once it is unplugged every `exec`
dies with `EIO` and the button only ever printed `Could not restart: … (os error 5)`).
The payload (`finish::qr_report::build_report`, untranslated like the install log) is the
error, the persisted log paths and the **tail** of `/var/log/modulixos-install.log` read back
asynchronously, cut to `widgets::qr_code::QR_MAX_BYTES` (2953 bytes = version 40 / EC level
L, the hard ceiling of a single QR code) with an explicit `(truncated, N bytes omitted)`
marker. `widgets::qr_code` draws the matrix itself on a `DrawingArea` — forced white
background whatever the theme, 4-module quiet zone, whole-pixel module size (a fractional
module renders blurry and a blurry code does not scan).

**Exactly one `HeaderBar` exists in the wizard** — `app.rs`'s, wrapping the whole
`NavigationView`, with both sets of title buttons off (nothing in a sway kiosk can act on
minimise/maximise/close). The summary and progress pages deliberately carry no `ToolbarView` of
their own; two header bars used to stack, each drawing its own window controls. Consequences:
the outer header's title has to be re-set from the visible page (`content_page.set_title`, in the
`visible-page` handler _and_ in the retranslate hook, which also re-titles the step
`NavigationPage`s), and the back chevron those pages used to get for free is now the outer
"Previous" button, shown on non-step pages only when `visible.can_pop()` — which is why the
progress page never clears its `can_pop(false)`.

**Scope**: the install is real end to end. Steps 7-8 still have a minimal UI, but their
answers reach the installed system. Known gaps: `limine`'s `extraEntries` for a Windows
dual boot is still unimplemented (a `modulix-core-utils` config option), and `mx.languages`
is never written from the chosen locale, so the Firefox module's language packs, hunspell
dictionaries and search-provider language stay on its `[ "fr" ]` default whatever language
the wizard ran in (that needs a new `InitParams` field in core-utils).

## Contract with `modulix-core-utils`

Direct `path` dependency (decided):

```toml
modulix-core-utils = { path = "../modulix-core-utils", features = ["init", "filesystem", "detect-hardware", "user", "locale"] }
```

- **`CONFIG_DIRECTORY`** (`modulix-core-utils/src/lib.rs`) is `/etc/modulix-os/` in release, `<repo>/test/` in debug — a real NixOS fixture repo used by examples/tests. See `modulix-core-utils/CLAUDE.md` for details.
- **`BuildCommand::as_str()`** collapses to `"build-vm"` for every variant in debug builds — a debug run never touches the host, it builds a VM image instead. Only release builds actually run `nixos-install`.
- `init(&InitParams)` does the whole seed in **one transaction** (main config + locale + keyboard + user + `package.nix` + `module.nix` + the LUKS entries + `boot.resumeDevice`) and is what `InitConfigTask` calls. It is synchronous (`std::process`, `git2`), so it must go through `spawn_blocking`. There is no `init_all` and none is needed.
- **Everything the installer writes into the config repo goes through `InitParams`.** The three fields that carry it: `packages: Vec<String>` → `package.nix` (`install_package::install_no_transaction`, so the `pkgs.<attr>` spelling matches what a later `install_package::uninstall` looks for), `modules: Vec<String>` → `module.nix` (`install_module::install_no_transaction`, one `mx.<name>.enable = true` each; names are _not_ validated against `modules/index.json`, a typo only fails at build time), `luks: Vec<LuksInit>` → `write_fstab_extras` on `fstab.nix`, and `resume_device: Option<String>` → `filesystem::set_resume_device_no_transaction`. An empty list writes no file and adds no import. `LuksInit::name` must be the **live mapper name** (`engine::LUKS_MAPPER_NAME` / `::LUKS_SWAP_MAPPER_NAME`), because that is what `nixos-generate-config` keyed its own entry on. `LuksInit::container` is the asymmetry between the two: `None` for root, whose `.device` the generator already wrote, and `Some("/dev/disk/by-uuid/…")` for the swap container, which the generator never mentions at all — it only emits LUKS entries while walking the mount points it found, and a swap device is not one, so without this the installed system has nothing to unlock the swap with. `InitConfigTask::luks_and_resume` builds both from `PipelineState`, reading the UUIDs through `DiskBackend::partition_uuid` (udisks2' `Block.IdUUID`, no shell-out).
- **`install_package`/`install_module` are each split in two features.** Their public `install`/`uninstall` force `BuildCommand::Switch` and so are unusable from the installer, and their full features drag in `reqwest`/`memmap2`/`phf`. The light `install-package-file`/`install-module-file` (`["core-nix-file"]`) expose only the `*_no_transaction` halves and the `pub FILE_*_PATH` constants; `init` depends on those two, so the installer links no HTTP client. Verify with `cargo tree -p modulixos-installer | grep reqwest` — it must stay empty.
- `init()` **never rebuilds**: outside debug it holds core-utils' skip-rebuild lock for its whole run, and its own doc says the installer drives the build. `rebuild_config` is private and unreachable anyway — hence `NixosInstallTask`. This is also why the installer cannot call `install_package::install` directly: that one commits _and_ switches, and `hold_skip_rebuild_lock` is private.
- `InitParams::config_dir` is passed explicitly (`/mnt/etc/modulix-os`) so `resolve_config_path` behaves the same in debug and release; left to `None`, a debug build would write into core-utils' own `test/` tree.
- `--root /mnt` is hardcoded in `rebuild_config` (`modulix-core-utils/src/core/transaction/transaction.rs:226`) — the installer must mount the install target at `/mnt`, not a configurable path.
- `rebuild_config` inherits stdout and only captures stderr for the error message — no live streaming. This is precisely why `NixosInstallTask` spawns `nixos-install` itself with both streams piped.

### Pitfall: `configuration.nix` imports vs. file overwrite

`Transaction::begin()` auto-inserts any newly-created file into `configuration.nix`'s `imports` list — but `init()` then overwrites `configuration.nix` wholesale, discarding that insertion, which is why its `configuration_nix()` lists `./hardware-configuration.nix`, `./fstab.nix`, `./locale.nix`, `./users.nix` explicitly, plus `./package.nix`/`./module.nix` whenever the matching `InitParams` list is non-empty. The trap is therefore fully handled inside core-utils now; the installer no longer patches that list after the fact. (Existing side effect this masks today: `flake.nix` is in `add_file`, so `begin()` inserts a nonsensical `./flake.nix` into `imports` — currently hidden only because the overwrite wipes it.)

## Security / data-loss risk points to handle explicitly

- **Passwords must never land in the Nix store in plaintext.** `user::add_no_transaction` writes `initialPassword`, and `init()` passes an empty one. The installer therefore never puts a password in the configuration at all: `SetPasswordsTask` pipes `chpasswd` into `nixos-enter` after the install, writing `/etc/shadow` directly. This depends on `users.mutableUsers` staying `true`.
- **NTFS resize ("alongside Windows" mode)**: refuse if BitLocker is detected (can't be shrunk from Linux), if Windows fast-startup/hibernation left the volume dirty (`hiberfil.sys`, dirty flag), or if `ntfsresize --no-action` fails. Mandatory preflight before any partition-table write.
- **TPM2 + Limine**: Modulix uses `boot.loader.limine`. TPM2 unlock needs `boot.initrd.systemd.enable` plus `boot.initrd.systemd.tpm2.enable`, and both come from **mxpkgs** (`mxpkgs/modulixos/boot.nix:226` and `:234`) — the installer writes neither. What it does write is the `crypttabExtraOpts = [ "tpm2-device=auto" ]` half, through `InitParams::luks`, onto the `boot.initrd.luks.devices."modulixroot"` entry `nixos-generate-config` already put in `fstab.nix`. For that entry it must add _only_ that attribute: re-declaring `.device` is what the removed `luks.nix` did, and two definitions of one `types.str` option make the module system fail. The `modulixswap` entry is the exact opposite and the only place the installer does declare a `.device`, because the generator reports no LUKS entry for a swap-only container. `EnrollTpmTask` runs `systemd-cryptenroll --tpm2-device=auto` for real, and runs _before_ `InitConfigTask` so `tpm2_enabled` is settled by the time the configuration is written. **Still unverified**: treat TPM2 unlock as unconfirmed until an encrypted install has actually been booted on real hardware or a VM with Limine — the double definition above means no encrypted install can ever have got that far.
- **Nobody writes EFI boot variables** — not the installer, not nixpkgs. `mxpkgs/modulixos/boot.nix`
  keeps `boot.loader.efi.canTouchEfiVariables = lib.mkMxDefault false`, which makes
  `boot.loader.limine.efiInstallAsRemovable` (a `!canTouchEfiVariables` default) true, so
  limine installs the firmware-probed fallback `ESP/efi/boot/BOOTX64.EFI` and
  `limine-install.py` skips its NVRAM block entirely. The install therefore boots through the
  firmware's default path, and nothing can drift: the `canTouchEfiVariables is set to false …`
  warning `limine-install.py` prints during the install is expected. There used to be two
  mechanisms fighting over that entry and neither is left: an `EfiEntryTask` creating a
  `ModulixOS` entry with `efibootmgr` and rewriting `BootOrder`, and — in mxpkgs — a
  `replaceStrings` rewrite of nixpkgs' `limine-install.py` source relabelling `Limine` to
  `ModulixOS`, which was dead code anyway (the whole block it patched is guarded by
  `canTouchEfiVariables`) and broke on every upstream reformat. The trade-off is explicit: a
  firmware that refuses to boot `EFI/BOOT/BOOTX64.EFI` off an internal disk has no NVRAM entry
  to fall back on, which is why `efibootmgr` stays in the ISO's `systemPackages` — it is the
  way to write one by hand from the tty1 recovery shell. `limine.conf` is unaffected either
  way, it stays at `ESP/limine/limine.conf`.
- **Hibernation**: swap size ≥ RAM, `boot.resumeDevice` set, and if encryption is enabled the swap must live inside a LUKS container. All three are implemented: `engine::sizing::compute_swap_bytes` oversizes the hibernation image, `plan::swap_bytes_for` adds `sizing::LUKS2_HEADER_BYTES` on top when the install is encrypted (the partition is 16 MiB larger than the swap it ends up offering, which is what the manual-mode `SwapTooSmallForHibernation` guards compare through `sizing::usable_swap_bytes`), `EncryptTask` puts the swap in `modulixswap`, and `InitParams::resume_device` writes `boot.resumeDevice` — `/dev/mapper/modulixswap` when encrypted, the swap's `by-uuid` path otherwise. It is **not** optional: with `boot.initrd.systemd.enable` (which mxpkgs sets) NixOS only passes `resume=` when that option is set, and the fallback over `swapDevices` exists in the script initrd only. Untested end to end — `systemctl hibernate` on an installed system is the only thing that proves it.
- **Never probe the installed system with a host-side path**: `nixos-install` builds into the
  target's own store (`nix build --store /mnt`, then
  `nix-env --store /mnt -p /mnt/nix/var/nix/profiles/system --set`), so
  `/mnt/nix/var/nix/profiles/system` is a symlink to an **absolute** `/nix/store/…` path the
  live ISO does not have — and `<toplevel>/sw` is another one. Every
  `fs::metadata("/mnt/nix/var/nix/profiles/system/sw/bin/…")` therefore fails, and a guard
  built on one reads the answer as "not installed" whatever the truth. That is exactly what
  silently disabled `FlatpakRemoteTask` and `GnomeGridTask` on every install until their
  guards were moved onto the wizard's answers (upstream has the same constraint and tests
  inside the chroot instead — `nixos-install.sh`'s
  `nixos-enter --root "$mountPoint" -c 'test -e …/bin/passwd'`). Rules: derive a guard from
  `InstallConfig`, which is what decided the target's contents in the first place; run the
  command through `nixos-enter` with a chroot-relative absolute path and let `best_effort`
  report a real failure; and only test paths that genuinely live on the target
  (`/mnt/home/<user>/…`) with a host-side `fs::metadata`.
- **Root-in-kiosk**: acceptable here (throwaway live ISO, single app), but every `Command::new` must pass arguments as an array — never a shell — and device paths must be validated against the udisks2 enumeration, never built by concatenating user input.
- **WebKitGTK captive-portal window runs as root**: step 6's portal window (`widgets/portal_window.rs`) embeds a third-party-controlled web page (any café's captive portal) in a process running as root inside the kiosk. Mitigations in place: an ephemeral `webkit6::NetworkSession` (no persisted cookies/cache), `enable_developer_extras`/`javascript_can_open_windows_automatically` disabled, and WebKitGTK 6.0's own bubblewrap sandbox for the web process (`set_sandbox_enabled` no longer exists in the 6.0 API — the sandbox is always on). Not a regression versus the Calamares-based flow it replaces (the patched QML network page did the same), but verify on the real ISO that user namespaces are available, or WebKit's web process refuses to start. `webkitgtk_6_0` comes in as a `buildInput` of `nix/package.nix`, so the wrapper carries it.
- **The partitioning step's shell-outs need runtime packages too** (all carried by `nix/kiosk-module.nix` and `nix/package.nix`'s `--prefix PATH`): `ntfs3g` (`ntfsresize`, NTFS shrink
  preflight), `gptfdisk` (udisks2's GPT backend), `util-linux` (`mkswap`/`swapon`, `mount`/
  `umount`), `cryptsetup` (LUKS2), `tpm2-tools` (`systemd-cryptenroll`), `efibootmgr`
  (nothing calls it any more — it is the only way to inspect or repair the boot record from
  the tty1 recovery shell, so the ISO carries it while the app wrapper no longer does),
  `udisks2` (the
  `org.freedesktop.UDisks2` D-Bus service itself), `gparted` (manual mode's partition
  editor, needed only to resize or delete a partition the user already created — carving a
  new one out of free space happens in-app, see step 6 above), plus `git` (the config repo),
  `e2fsprogs` (`mkfs.ext4`, which udisks2 shells out to), `sway` (`swaymsg`, for
  the keyboard step and the HiDPI scale script) and `jq` (that same script's only
  JSON parser). Also `nixos-install-tools` (`nixos-generate-config` for
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
  installer simulated a whole install and reported success (`eb6840b`, "drop the fake backends
  and simulation mode"). `Backends::new()` failing means `app::build_fatal_window` takes over: a
  fullscreen error page with the full, selectable reason and a single Quit button, and a
  non-zero exit — still true today, fallback or not.
  `src/debugsim/` (see "Commands" above) reintroduces a fake `DiskBackend`/`CryptBackend` pair
  without reopening that hole, on three separate guarantees: it is `#[cfg(debug_assertions)]`
  end to end, so a release binary (the only thing the live ISO ships) never links a line of it;
  it only ever activates on an explicit `--fake-disks=`/`--fake-error=`/`--fake-instant` flag,
  never as a fallback from a connection failure; and `Backends::new()`/`Backends::with_fake_disks()`
  failing is fatal either way — faking the disks is not a reason to also fake away a genuinely
  unreachable D-Bus service. A release binary additionally refuses outright to start on any
  `--fake*` flag (or any other unrecognized one), through the exact same `build_fatal_window`
  path, since the kiosk has no terminal for a plain exit code to mean anything.
- **A failed install must stay diagnosable after a reboot**: the live ISO root is a RAM
  overlay and journald on it is volatile, so `engine::install_log` mirrors every
  `ProgressEvent` to `/var/log/modulixos-install.log`, to `stderr` (→ journald, tty2 is still
  a getty), and — via `copy_to_target`, guarded on `/mnt` really being a mount point — to
  `/mnt/var/log/modulixos-install.log`, the only copy that survives. On failure the tee does
  the copy because `PostInstallTask` never runs; on success `PostInstallTask` does it just
  before unmounting.
- **HiDPI is handled by the compositor, not the app**: there is no `scale_factor()`,
  `GDK_SCALE` or DPI logic anywhere in `src/` — at scale 1 the wizard is unreadable on a
  1440p or 4K panel. `nix/kiosk-module.nix`'s `applyOutputScale` script runs from inside
  `runInstaller`, _before_ the binary, so the window gets its size on its first frame; it is
  not a second `exec` in `swayConfig`, because two `exec` lines race. Per active output it
  reads `swaymsg -t get_outputs -r` through `jq`, takes the physical width from byte 21 (cm)
  of `/sys/class/drm/card*-<name>/edid` — sway names outputs after DRM connectors, so the
  glob is the whole mapping — and picks `dpi / 96` rounded to the nearest 0.25, with a
  `dpi < 120` deadzone. The EDID is ignored under `systemd-detect-virt` (qemu synthesises a
  ~100 dpi one that would mask the resolution) and when it reads 0; the fallback is pure
  resolution thresholds (≥3200 or ≥1800 → 2.0, ≥2400 or ≥1400 → 1.5). Two clamps then
  bracket the result, and they are what actually decides most real cases: a **logical height
  ceiling of 1300** (this is what moves a 27" 1440p off 1.0, where the raw DPI would have
  left it) and a **logical floor of 1280x800**, below which the desktop-environment step's
  cards (`height_request(600)`) and its 1300x931 zoom dialog start clipping. `MODULIX_SCALE`
  or a `modulix.scale=` kernel parameter overrides the whole computation. Fractional scales
  are deliberate: sway 1.12 and GTK 4.22 both speak `wp-fractional-scale-v1`, so the
  installer stays crisp at 1.25/1.5/1.75 — GParted is GTK3, gets told 2 and is downscaled by
  sway, i.e. slightly soft, which is the accepted trade.
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
  `multi-user.target` is _not_ ordered before the session, so the module's `ExecStartPre`
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
  `plymouth-quit.service`, which merely _asks_ plymouthd to stop), and
  `OnFailure=`/`OnSuccess=modulix-installer-fallback.service` prints `systemctl status`, the
  last 60 journal lines and `ls /dev/dri` on tty1, then hands over a root `agetty`
  (`OnSuccess` too: the installer has no reboot button, so quitting it normally also drops
  to a bare VT). The VT-existence check is `AssertPathExists`, **not**
  `ConditionPathExists` — an unmet condition makes systemd _skip_ the unit and call that a
  success, so `OnFailure=` would never fire. `runInstaller` touches
  `/run/modulix-installer.started` and writes the installer's exit status to
  `/run/modulix-installer.status`; `startSession` reads both back, since sway `exec`s the
  installer and those files are the only way its fate can reach the unit. The `.started`
  flag is what gates the software retry: once the compositor has come up, a second attempt
  would re-run a destructive pipeline rather than fix a renderer.
