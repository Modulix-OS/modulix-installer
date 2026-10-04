use crate::engine::{
    CONFIG_REPO, INSTALL_ROOT, LUKS_MAPPER_NAME, ProgressEvent, ProgressSink, Task, TaskCtx,
};
use crate::mx;
use async_trait::async_trait;
use modulix_core_utils::init::{Desktop, InitParams, LuksInit, init};

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
///
/// The application pack rides along in that same transaction: `packages` goes
/// to `package.nix` and `modules` to `module.nix`, through core-utils' own
/// `install_package`/`install_module` writers, so a post-install `mx` sees
/// exactly the spelling it would have written itself. `luks` adds nothing but
/// `crypttabExtraOpts` — `nixos-generate-config`, which `init` runs to build
/// `fstab.nix`, already declares the container under its live mapper name, and
/// a second definition of that option would make the NixOS module system
/// fail.
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
            packages: ctx
                .config
                .app_pack
                .packages(ctx.config.desktop_environment)
                .into_iter()
                .map(String::from)
                .collect(),
            modules: ctx
                .config
                .app_pack
                .modules()
                .into_iter()
                .map(String::from)
                .collect(),
            luks: ctx
                .config
                .partitioning
                .encryption_enabled
                .then(|| LuksInit {
                    name: LUKS_MAPPER_NAME.to_string(),
                    tpm2: ctx.config.partitioning.tpm2_enabled,
                }),
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
