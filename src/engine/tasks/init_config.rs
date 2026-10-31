use crate::config::SwapMode;
use crate::engine::{
    CONFIG_REPO, INSTALL_ROOT, LUKS_MAPPER_NAME, LUKS_SWAP_MAPPER_NAME, ProgressEvent,
    ProgressSink, Task, TaskCtx,
};
use crate::mx;
use async_trait::async_trait;
use modulix_core_utils::init::{Desktop, InitParams, LuksInit, init};
use modulix_core_utils::secure_boot::{self, FirmwareState};

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
/// The application selection rides along in that same transaction: `packages` goes
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
        let firmware = secure_boot::firmware_state();
        let secure_boot_requested = matches!(firmware, Ok(FirmwareState::SetupMode));
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
                .apps
                .packages(ctx.config.desktop_environment)
                .into_iter()
                .map(String::from)
                .collect(),
            modules: ctx
                .config
                .apps
                .modules()
                .into_iter()
                .map(String::from)
                .collect(),
            luks,
            resume_device,
            secure_boot: true,
            config_dir: Some(CONFIG_REPO.to_string()),
            debug: false,
        };

        // Recorded before anything is built, because enrolling a Platform Key
        // is what takes the firmware out of Setup Mode: once `nixos-install`
        // has run, the live state can no longer say whether an enrollment was
        // attempted. `SecureBootTask` reads this back to decide whether the
        // outcome is worth verifying.
        ctx.state.lock().await.secure_boot_requested = secure_boot_requested;
        let _ = tx
            .send(ProgressEvent::Log(format!(
                "secure boot: {}",
                firmware_summary(&firmware)
            )))
            .await;

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

/// Untranslated one-line description of the firmware's Secure Boot state, for
/// the install log.
///
/// Logged whatever the state is, including the uninteresting ones: a user who
/// expected Secure Boot and did not get it needs the log to say why, and the
/// only alternative — saying nothing on the paths that change nothing — is
/// exactly what makes a silent no-op impossible to diagnose after a reboot.
///
/// * `state` - what `secure_boot::firmware_state` returned.
///
/// # Returns
/// A sentence naming the state and whether the option will be declared.
fn firmware_summary(state: &modulix_core_utils::mx::Result<FirmwareState>) -> String {
    match state {
        Ok(FirmwareState::SetupMode) => {
            "the firmware is in setup mode, enabling it and enrolling keys".to_string()
        }
        Ok(FirmwareState::UserMode) => {
            "not enabled (the firmware already has a platform key enrolled)".to_string()
        }
        Ok(FirmwareState::NotUefi) => {
            "not enabled (this machine did not boot in UEFI mode)".to_string()
        }
        Err(e) => format!("not enabled (the firmware state could not be read: {e})"),
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
