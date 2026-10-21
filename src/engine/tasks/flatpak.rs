use crate::engine::tasks::{best_effort, report};
use crate::engine::{INSTALL_ROOT, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;

/// The Flathub repository description file `flatpak remote-add` reads.
const FLATHUB_REPO: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";

/// Absolute path of `flatpak` inside the installed system. `nixos-enter -- …`
/// execs the command directly, with no PATH of its own, so the system profile
/// has to be spelled out — same path `SetPasswordsTask` uses for `chpasswd`.
const FLATPAK_BIN: &str = "/nix/var/nix/profiles/system/sw/bin/flatpak";

/// Registers the Flathub remote in the freshly installed system.
///
/// Deliberately a duplicate: mxpkgs' `mx.services.flatpak` module ships a
/// `flatpak-repo` systemd unit that runs the very same
/// `flatpak remote-add --if-not-exists` on every boot. Doing it here as well
/// means the app store already has its remote on the first session, even if
/// that first boot has no network — and `--if-not-exists` makes the overlap a
/// no-op.
///
/// Best-effort from end to end: the system is already on disk by the time
/// this runs, and the mxpkgs unit is the fallback, so neither a missing
/// `flatpak` nor a failing `remote-add` may fail the install.
pub struct FlatpakRemoteTask;

#[async_trait]
impl Task for FlatpakRemoteTask {
    fn label(&self) -> String {
        "Registering the Flathub remote".to_string()
    }

    fn weight(&self) -> u32 {
        1
    }

    /// # Pre-conditions
    /// `NixosInstallTask` has completed and the target is still mounted at
    /// `/mnt`, which is why this stage precedes `PostInstallTask`.
    ///
    /// # Post-conditions
    /// The `flathub` remote exists in the target's system installation, or
    /// the reason it does not is in the install log.
    ///
    /// # Returns
    /// Always `Ok(())` — see the type-level doc.
    async fn run(&self, _ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        // The presence of the binary, not the chosen application pack: this
        // also covers a flatpak that came in as a hand-picked package or
        // through a module enabled elsewhere.
        if tokio::fs::metadata(format!("{INSTALL_ROOT}{FLATPAK_BIN}"))
            .await
            .is_err()
        {
            report(
                tx,
                "flatpak remote-add",
                Err("flatpak is not installed".to_string()),
            )
            .await;
            return Ok(());
        }

        // Arguments as an array, never a shell: this runs as root.
        let outcome = best_effort(
            "nixos-enter",
            &[
                "--root",
                INSTALL_ROOT,
                "--",
                FLATPAK_BIN,
                "remote-add",
                "--if-not-exists",
                "flathub",
                FLATHUB_REPO,
            ],
        )
        .await;
        report(tx, "flatpak remote-add", outcome).await;
        Ok(())
    }
}
