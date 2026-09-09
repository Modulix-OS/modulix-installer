# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`modulixos-installer` is the future installer for Modulix OS (NixOS-based distro): a single **fullscreen Rust + GTK4/libadwaita app** running as root inside a minimal invisible `cage` kiosk session (Windows-installer style), replacing the current Calamares-based flow (`mxpkgs/installer/default.nix`, patched QML network page + `modulixnixos` job running `mx-init` + `nixos-install`).

**Current state: steps 1-6 are real and fully wired**, including a genuinely destructive install pipeline (partition → encrypt → format → mount → TPM2 enroll) gated behind the summary page's confirmation dialog. Steps 7-9 (user, desktop environment, application pack) are still typed stubs. The ISO module stays on Calamares until steps 1-9 are real.

## Commands (once scaffolding lands)

```bash
cargo fmt && cargo clippy -- -D warnings   # mandatory before any commit (global rule)
cargo test                                  # pure logic only — swap sizing, partition planner,
                                             # zone.tab/evdev.xml parsing, step-registry is_relevant()
cargo run -- --fake --windowed              # full UI on a dev machine, no root, fake backends
cargo run -- --fake --windowed --fake-disk=windows   # seed the fake disk backend from a named
                                             # scenario (see backend::disk::scenario::DiskScenario;
                                             # implies --fake); unknown names list valid ones on stderr
```

`nix build` of the ISO itself still goes through `mxpkgs/installer/default.nix` (currently the Calamares-based GNOME live image); that module switches to this app once the 9 steps are real.

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

### Backends (`src/backend/`) — one trait per subsystem, each with a real impl and a `fake` impl

| Trait | Real impl | Role |
|---|---|---|
| `DiskBackend` | `Udisks2Backend` (zbus) | enumerate disks/partitions/free space, create/delete/resize/format/mount |
| `NetworkBackend` | `NetworkManagerBackend` (zbus) | ethernet, Wi-Fi scan/connect, `Connectivity` → captive-portal detection |
| `LocaleBackend` | `SystemLocaleBackend` | locales (`locale.gen`), timezones (`zone.tab` lat/lon for the map), keyboard layouts (`evdev.xml` from xkeyboard-config) |
| `A11yBackend` | `OrcaBackend` | narrator (speech-dispatcher/orca) — the only method that does anything under `cage`; the other four are best-effort GNOME a11y `gsettings` calls (silently `Ok(())` if `gsettings` isn't on `PATH`, real errors otherwise), useful only on a dev machine under a full GNOME session. What actually applies live to the installer itself (high contrast, large text) is `src/a11y.rs`'s `A11ySettings`, not this trait |
| `CryptBackend` | `CryptsetupBackend` | `cryptsetup luksFormat`, `systemd-cryptenroll --tpm2-device` (± `--tpm2-with-pin`) |

`fake` impls let the whole UI run on a dev machine without root (`--fake` CLI flag) and let the `Task` pipeline be tested end-to-end without touching a real disk.

### `trait Task` (`src/engine/mod.rs`) — install pipeline

```rust
#[async_trait]
pub trait Task {
    fn label(&self) -> String;
    fn weight(&self) -> u32;    // for the progress bar
    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()>;
}
```

Pipeline: `PartitionTask` → `EncryptTask` → `FormatTask` → `MountTask` → `EnrollTpmTask` → `InitConfigTask` (calls `modulix_core_utils::init::init_all`, currently a fake no-op) → `PostInstallTask`. `EncryptTask` must run right after `PartitionTask` and before `FormatTask`/`MountTask` — `luksFormat`/`luksOpen` need the bare partition, not one already `mkfs`'d and mounted on `/mnt`.

### Directory layout

```
src/
  main.rs               bootstrap adw::Application, flags --fake / --windowed
  app.rs                AdwApplicationWindow fullscreen, NavigationView, step rail
  a11y.rs                A11ySettings (GObject) — shared narrator/high-contrast/large-text/
                        magnifier/sticky-keys state, live effects on the installer itself
  config.rs             InstallConfig — all answers, Rc<RefCell<_>>
  i18n.rs               gettext + hot reload — see "i18n" below for the LANGUAGE/setlocale split
  steps/                mod.rs (trait + registry) + one file per step
  finish/               summary.rs, progress.rs — post-step-7 review (recomputes
                        `plan::plan` against live disk state, destructive-confirmation
                        dialog) + install-progress page; pushed outside the step rail,
                        `StepId::ALL` stays at 9
  backend/              disk/ (mod.rs, layout.rs free-gap math, ntfs.rs shrink-preflight
                        parser, scenario.rs `--fake-disk=` fixtures, fake.rs, udisks2.rs)
                        net/ locale/ a11y/ crypt/  (trait + impl + fake)
  engine/               trait Task, Pipeline, ProgressSink, live_input.rs (shared
                        live-disk-state → `plan::PlanInput` fetch), plan.rs (pure
                        partitioning planner), tasks/ (partition.rs, encrypt.rs, format.rs,
                        mount.rs, enroll_tpm.rs, init_config.rs, post_install.rs)
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
  means the module that builds it (`mxpkgs/installer/default.nix`, once it switches off
  Calamares) **must set `i18n.supportedLocales` to generate at least one non-`C` locale** — with
  none generated, `LANGUAGE` is ignored for the whole session and the language step becomes a
  no-op again, exactly the bug this module fixed.
- `po/` currently ships `fr.po` only — French and English (gettext's `msgid` fallback) are the
  only two languages that visibly differ today, even though the dropdown lists every glibc locale.

### The 9 steps, fixed order

1. **Accessibility** — first page, before anything else. Carries the narrator toggle
   (`Enable narrator`, starts orca immediately — merged in from the former standalone
   narrator step) plus two groups: "Applied now" (high contrast, large text — take effect
   immediately in the installer itself via `src/a11y.rs`) and "Applied to the installed
   system" (screen magnifier, sticky keys — compositor-level settings `cage` doesn't expose
   to the app, so these are only recorded into `InstallConfig` and best-effort forwarded to
   `gsettings`, not applied live). All five toggles are also reachable from every other page
   via a header-bar accessibility `MenuButton` popover (`<Ctrl><Alt>a`), bound to the same
   shared `A11ySettings` instance so step 1 and the popover never drift out of sync.
2. **Language** — triggers `retranslate()` on every already-built page.
3. **Timezone** — `gtk::DrawingArea` world map, equirectangular projection of `zone.tab` coordinates, click → nearest zone (same approach as Calamares) + region/city fallback list.
4. **Keyboard** — layout + variant from `evdev.xml`, live typing test area, applies the layout immediately.
5. **Network** — ethernet/Wi-Fi via NetworkManager; if `Connectivity == Portal`, opens an `adw::Dialog` with embedded WebKitGTK, auto-closed once `Connectivity == Full`. Fully implemented (real NM backend in `backend/net/network_manager.rs`, fake state machine in `backend/net/fake.rs`): open/secured/hidden Wi-Fi, WPA2/WPA3 (transition-safe), live connectivity watch, "Next" gated on `Connectivity` (`Full` → ready, `Limited`/`Unknown` → blocked with a "Continue anyway" override, `Portal`/`None` → hard block). In `--fake`, the backend starts disconnected and exposes five demo SSIDs: `Modulix-Fake-Open` (no password), `Modulix-Fake-WPA2`/`-WPA3`/`-Hidden` (password `modulix`), and `Modulix-Fake-Portal` (routes through the offline demo captive-portal page). `--fake`'s scan also returns two undeduplicated duplicate entries to exercise `dedup_access_points`.
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
   button on the manual page, `steps/partitioning.rs`'s `run_gparted`, not available under
   `--fake`) is only needed to resize or delete an *existing* partition. Formatting is
   forced for `/`, `/boot`, and swap (`engine::plan::format_is_forced`) — only `/home` can
   be kept as-is. The editor collects `Vec<plan::ManualItem>` (`Existing` assignment or
   `New` creation) and preserves the user's assignments/pending creations across a
   `set_data()` reload (e.g. after returning from GParted) — all validation lives in
   `plan::plan`, not the widget. Outside manual: swap (none / standard
   / hibernation, sized via `engine::sizing`) + LUKS2 encryption + TPM2 (with or without
   PIN). `--fake-disk=<scenario>` (see `backend::disk::scenario::DiskScenario`) seeds the
   fake disk backend so every mode/blocker combination can be clicked through by hand.
7. **User** — primary user; root gets the same password.
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
`gtk::Expander`, progress bar pinned to the bottom) → done.

**Iteration 1 scope**: steps 1-6 are fully functional — including a real, destructive
install pipeline (partition → encrypt → format → mount → TPM2 enroll) reachable from the
summary page's "Install" button, behind a confirmation dialog. Steps 7-9 are still typed
stubs (page present, `InstallConfig` already carries the fields, `commit()` implemented,
minimal UI). `init_all` itself remains out of scope for iteration 1 — `InitConfigTask`/
`PostInstallTask` stay fake no-ops, so the pipeline partitions/encrypts/formats/mounts a
real disk but never writes a NixOS configuration or runs `nixos-install`; wiring `init_all`
in once `modulix-core-utils` grows it (iteration 2) is a drop-in inside those two tasks.

## Contract with `modulix-core-utils`

Direct `path` dependency (decided):

```toml
modulix-core-utils = { path = "../modulix-core-utils", features = ["init", "filesystem", "detect-hardware", "user", "locale"] }
```

- **`CONFIG_DIRECTORY`** (`modulix-core-utils/src/lib.rs`) is `/etc/modulix-os/` in release, `<repo>/test/` in debug — a real NixOS fixture repo used by examples/tests. See `modulix-core-utils/CLAUDE.md` for details.
- **`BuildCommand::as_str()`** collapses to `"build-vm"` for every variant in debug builds — a debug run never touches the host, it builds a VM image instead. Only release builds actually run `nixos-install`.
- `init()` (`modulix-core-utils/src/init.rs:224`) currently does **3 separate transactions** (main config, then `locale::set_locale`, then `user::add`) ⇒ 3 commits + 3 builds. The planned `init_all(&InitParamsFull)` (one transaction, one commit, one `nixos-install`) is a **modulix-core-utils change for iteration 2**, not iteration 1 — don't implement it here, just leave `InitConfigTask` calling it as a fake.
- `--root /mnt` is hardcoded in `rebuild_config` (`modulix-core-utils/src/core/transaction/transaction.rs:226`) — the installer must mount the install target at `/mnt`, not a configurable path.
- `rebuild_config` inherits stdout and only captures stderr for the error message — no live streaming. Until `modulix-core-utils` grows a variant that pipes stdout+stderr through a callback, the install screen can only show an indeterminate progress bar, not a live log.

### Pitfall: `configuration.nix` imports vs. file overwrite

`Transaction::begin()` (`modulix-core-utils/src/core/transaction/transaction.rs:~546-553`) auto-inserts any newly-created file into `configuration.nix`'s `imports` list — but `init()`/`init_all` then overwrites `configuration.nix`'s content wholesale, silently discarding that insertion. Any `configuration_nix()` used by `init_all` must **explicitly** list `./hardware-configuration.nix`, `./fstab.nix`, `./locale.nix`, `./users.nix` in `imports` itself. (Existing side effect this masks today: `flake.nix` is in `add_file`, so `begin()` inserts a nonsensical `./flake.nix` into `imports` — currently hidden only because the overwrite wipes it.)

## Security / data-loss risk points to handle explicitly

- **Passwords must never land in the Nix store in plaintext.** `user::add_no_transaction` currently writes `initialPassword`; the installer path (via `init_all`) needs `hashedPassword` (`mkpasswd -m yescrypt`) for both the user and root instead.
- **NTFS resize ("alongside Windows" mode)**: refuse if BitLocker is detected (can't be shrunk from Linux), if Windows fast-startup/hibernation left the volume dirty (`hiberfil.sys`, dirty flag), or if `ntfsresize --no-action` fails. Mandatory preflight before any partition-table write.
- **TPM2 + Limine**: Modulix uses `boot.loader.limine`. TPM2 unlock requires `boot.initrd.systemd.enable = true` and `crypttabExtraOpts = [ "tpm2-device=auto" ]` — validate on a VM before exposing this in the UI. **Not yet done**: step 7 now exposes a TPM2 toggle and `engine::tasks::EnrollTpmTask` runs `systemd-cryptenroll --tpm2-device=auto` for real against `CryptsetupBackend`, ahead of that VM validation — treat TPM2 unlock as unverified until confirmed on real hardware/a VM with Limine.
- **Hibernation**: swap size ≥ RAM, `boot.resumeDevice` set, and if encryption is enabled the swap must live inside the LUKS container.
- **Root-in-kiosk**: acceptable here (throwaway live ISO, single app), but every `Command::new` must pass arguments as an array — never a shell — and device paths must be validated against the udisks2 enumeration, never built by concatenating user input.
- **WebKitGTK captive-portal window runs as root**: step 6's portal window (`widgets/portal_window.rs`) embeds a third-party-controlled web page (any café's captive portal) in a process running as root inside the kiosk. Mitigations in place: an ephemeral `webkit6::NetworkSession` (no persisted cookies/cache), `enable_developer_extras`/`javascript_can_open_windows_automatically` disabled, and WebKitGTK 6.0's own bubblewrap sandbox for the web process (`set_sandbox_enabled` no longer exists in the 6.0 API — the sandbox is always on). Not a regression versus the Calamares-based flow it replaces (the patched QML network page did the same), but verify on the real ISO that user namespaces are available, or WebKit's web process refuses to start. `webkitgtk_6_0` needs to be added as a *runtime* dependency of the ISO module (`mxpkgs/installer/default.nix`) once that module switches off Calamares — it's currently only a `flake.nix` build input.
- **Step 7's shell-outs need runtime packages too**: `ntfs3g` (`ntfsresize`, NTFS shrink
  preflight), `gptfdisk` (udisks2's GPT backend), `util-linux` (`mkswap`/`swapon`, `mount`/
  `umount`), `cryptsetup` (LUKS2), `tpm2-tools` (`systemd-cryptenroll`), `udisks2` (the
  `org.freedesktop.UDisks2` D-Bus service itself), `gparted` (manual mode's partition
  editor, needed only to resize or delete a partition the user already created — carving a
  new one out of free space happens in-app, see step 7 above) — same situation as
  `webkitgtk_6_0` above: currently only in `flake.nix`'s devShell, must become runtime
  deps of the ISO module once it switches off Calamares.
- **GParted is a second toplevel window under `cage`**: step 7's manual mode launches it
  full-screen over the running disk (`steps/partitioning.rs`'s `run_gparted`), unmounting
  every partition on the target disk first since GParted refuses a mounted one. `cage` is
  built for a single-window kiosk session — verify on the real ISO or a VM that a second
  toplevel actually gets focus and renders correctly, same open point as the WebKitGTK
  captive-portal window above.
