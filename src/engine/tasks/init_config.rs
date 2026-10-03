use crate::engine::{CONFIG_REPO, INSTALL_ROOT, ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;
use modulix_core_utils::init::{Desktop, InitParams, init};

/// Writes the NixOS configuration repository of the target system through
/// `modulix_core_utils::init::init`.
///
/// `init` seeds `/mnt/etc/modulix-os` with `flake.nix` (a
/// `mxpkgs.lib.modulixosSystem` configuration), `configuration.nix`,
/// `hardware-configuration.nix`, `fstab.nix`, `locale.nix` and `users.nix`,
/// runs `nix flake update`, commits, and seals those files immutable. It
/// deliberately does **not** rebuild: it holds modulix-core-utils'
/// skip-rebuild lock for its whole run precisely so the installer can drive
/// the build itself — which `NixosInstallTask` does, two stages later.
pub struct InitConfigTask;

#[async_trait]
impl Task for InitConfigTask {
    fn label(&self) -> String {
        "Writing NixOS configuration".to_string()
    }

    fn weight(&self) -> u32 {
        20
    }

    /// # Pre-conditions
    /// `MountTask` has mounted the target root at `/mnt`, and the machine is
    /// online: `init` refreshes the flake inputs (mxpkgs, nixos-hardware).
    ///
    /// # Post-conditions
    /// `/mnt/etc/modulix-os` is a committed git repository describing the
    /// target system. No system has been built yet.
    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        let desktop = Desktop::parse(ctx.config.desktop_environment.as_nix_str())?;
        let params = InitParams {
            root: INSTALL_ROOT.to_string(),
            hostname: ctx.config.user.hostname.clone(),
            username: ctx.config.user.username.clone(),
            full_name: ctx.config.user.full_name.clone(),
            desktop,
            locale: ctx.config.language.clone(),
            timezone: ctx.config.timezone.clone(),
            kb_layout: ctx.config.keyboard_layout.clone(),
            kb_variant: ctx.config.keyboard_variant.clone(),
            console_keymap: ctx.config.console_keymap.clone(),
            config_dir: Some(CONFIG_REPO.to_string()),
            debug: false,
        };

        let _ = tx
            .send(ProgressEvent::Log(format!(
                "writing the NixOS configuration to {CONFIG_REPO}"
            )))
            .await;
        let _ = tx.send(ProgressEvent::Indeterminate(true)).await;

        // `init` is synchronous (std::process + git2) and runs `nix flake
        // update`, so it must not block a tokio worker thread.
        let result = tokio::task::spawn_blocking(move || init(&params))
            .await
            .map_err(|e| mx::Error::Backend(format!("the configuration writer panicked: {e}")))?;

        let _ = tx.send(ProgressEvent::Indeterminate(false)).await;
        result?;

        let _ = tx
            .send(ProgressEvent::Log(
                "NixOS configuration written and committed".into(),
            ))
            .await;
        Ok(())
    }
}
