use crate::config::SwapMode;
use crate::engine::{
    CONFIG_REPO, INSTALL_ROOT, LUKS_MAPPER_NAME, LUKS_SWAP_MAPPER_NAME, ProgressEvent,
    ProgressSink, Task, TaskCtx,
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
/// exactly the spelling it would have written itself.
///
/// `luks` carries one entry per container, and the two halves differ on
/// purpose. For the root container only `crypttabExtraOpts` is asked for:
/// `nixos-generate-config`, which `init` runs to build `fstab.nix`, already
/// declares it under its live mapper name, and a second definition of that
/// option would make the NixOS module system fail. The swap container is the
/// opposite case — the generator emits LUKS entries only for the mount points
/// it walks, so a container holding swap alone is one it never mentions, and
/// its `.device` has to be declared here or the installed system has no way
/// to unlock it. `resume_device` is the last piece hibernation needs: with a
/// systemd initrd NixOS only passes `resume=` when `boot.resumeDevice` is
/// set.
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
        let (luks, resume_device) = luks_and_resume(ctx).await?;
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
            luks,
            resume_device,
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

/// Collects the LUKS and hibernation facts `init` cannot get from
/// `nixos-generate-config`.
///
/// # Parameters
/// * `ctx` - task context; reads `PipelineState` and the partitioning
///   answers, and uses the disk backend to resolve UUIDs.
///
/// # Pre-conditions
/// Runs after `EncryptTask`, so `PipelineState::luks_swap_device` holds the
/// raw swap container and `::swap_partition` its mapper, and after
/// `EnrollTpmTask`, so `tpm2_enabled` is settled.
///
/// # Returns
/// The containers to declare — root first, with no `.device` since the
/// generator writes that one — and the device hibernation resumes from, or
/// `None` when the target has no hibernation swap.
///
/// # Errors
/// [`mx::Error`] from `DiskBackend::partition_uuid`, which is the only way
/// to name a device the initrd can find again.
async fn luks_and_resume(ctx: &TaskCtx) -> mx::Result<(Vec<LuksInit>, Option<String>)> {
    let (swap_partition, luks_swap_device) = {
        let state = ctx.state.lock().await;
        (state.swap_partition.clone(), state.luks_swap_device.clone())
    };
    let cfg = &ctx.config.partitioning;

    let mut luks = Vec::new();
    if cfg.encryption_enabled {
        luks.push(LuksInit {
            name: LUKS_MAPPER_NAME.to_string(),
            container: None,
            tpm2: cfg.tpm2_enabled,
        });
    }
    if let Some(container) = &luks_swap_device {
        luks.push(LuksInit {
            name: LUKS_SWAP_MAPPER_NAME.to_string(),
            container: Some(ctx.backends.disk.partition_uuid(container).await?),
            tpm2: cfg.tpm2_enabled,
        });
    }

    let resume_device = match (cfg.swap_mode, &swap_partition) {
        (SwapMode::Hibernation, Some(swap)) => Some(if luks_swap_device.is_some() {
            format!("/dev/mapper/{LUKS_SWAP_MAPPER_NAME}")
        } else {
            ctx.backends.disk.partition_uuid(swap).await?
        }),
        _ => None,
    };

    Ok((luks, resume_device))
}
