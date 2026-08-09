# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`modulixos-installer` is the future installer for Modulix OS (NixOS-based distro): a single **fullscreen Rust + GTK4/libadwaita app** running as root inside a minimal invisible `cage` kiosk session (Windows-installer style), replacing the current Calamares-based flow (`mxpkgs/installer/default.nix`, patched QML network page + `modulixnixos` job running `mx-init` + `nixos-install`).

**Current state: empty scaffold.** `Cargo.toml` has no dependencies, `src/main.rs` is `Hello, world!`. Everything below documents the **target architecture** to build toward for iteration 1, not code that exists yet — treat unimplemented items as such rather than assuming they're already there. The ISO module stays on Calamares until steps 1-9 are real.

## Commands (once scaffolding lands)

```bash
cargo fmt && cargo clippy -- -D warnings   # mandatory before any commit (global rule)
cargo test                                  # pure logic only — swap sizing, partition planner,
                                             # zone.tab/evdev.xml parsing, step-registry is_relevant()
cargo run -- --fake --windowed              # full UI on a dev machine, no root, fake backends
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
| `A11yBackend` | `OrcaBackend` | narrator (speech-dispatcher/orca), high contrast, magnifier, large text |
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

Pipeline: `PartitionTask` → `FormatTask` → `MountTask` → `EnrollTpmTask` → `InitConfigTask` (calls `modulix_core_utils::init::init_all`) → `PostInstallTask`.

### Directory layout

```
src/
  main.rs               bootstrap adw::Application, flags --fake / --windowed
  app.rs                AdwApplicationWindow fullscreen, NavigationView, step rail
  config.rs             InstallConfig — all answers, Rc<RefCell<_>>
  i18n.rs               gettext + hot reload
  steps/                mod.rs (trait + registry) + one file per step
  backend/              disk/ net/ locale/ a11y/ crypt/  (trait + impl + fake)
  engine/               trait Task, Pipeline, ProgressSink, tasks/
  widgets/              timezone_map.rs, disk_bar.rs, password_entry.rs, ap_row.rs
data/
  icons/*.svg           one symbolic icon per step
  screenshots/          gnome.webp, plasma.webp, lxqt.webp
  resources.gresource.xml
build.rs                glib_build_tools::compile_resources
```

### The 9 steps, fixed order

1. **Narrator** — first page, before anything else; toggle starts orca immediately.
2. **Language** — triggers `retranslate()` on every already-built page.
3. **Timezone** — `gtk::DrawingArea` world map, equirectangular projection of `zone.tab` coordinates, click → nearest zone (same approach as Calamares) + region/city fallback list.
4. **Keyboard** — layout + variant from `evdev.xml`, live typing test area, applies the layout immediately.
5. **Accessibility** — high contrast, large text, magnifier, sticky keys.
6. **Network** — ethernet/Wi-Fi via NetworkManager; if `Connectivity == Portal`, opens an `AdwWindow` with embedded WebKitGTK, auto-closed once `Connectivity == Full`.
7. **Partitioning** — 4 modes: alongside Windows / whole disk / selected free space / manual. Outside manual: swap (none / standard / hibernation) + LUKS2 encryption + TPM2 (with or without PIN).
8. **User** — primary user; root gets the same password.
9. **Desktop environment** — gnome / plasma / lxqt, with description and screenshot.

Then a summary → progress screen (live log) → done.

**Iteration 1 scope**: steps 1-5 fully functional; steps 6-9 are typed stubs (page present, `InstallConfig` already carries the fields, `commit()` implemented, minimal UI). `init_all` itself is out of scope for iteration 1 — step 5 stops before any real install; `Task`/`InitConfigTask` are scaffolded with a fake impl so wiring it up in iteration 2 is a drop-in.

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
- **TPM2 + Limine**: Modulix uses `boot.loader.limine`. TPM2 unlock requires `boot.initrd.systemd.enable = true` and `crypttabExtraOpts = [ "tpm2-device=auto" ]` — validate on a VM before exposing this in the UI.
- **Hibernation**: swap size ≥ RAM, `boot.resumeDevice` set, and if encryption is enabled the swap must live inside the LUKS container.
- **Root-in-kiosk**: acceptable here (throwaway live ISO, single app), but every `Command::new` must pass arguments as an array — never a shell — and device paths must be validated against the udisks2 enumeration, never built by concatenating user input.
