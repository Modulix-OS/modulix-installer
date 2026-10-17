//! Fake doubles for the four pipeline stages that have no `DiskBackend`/
//! `CryptBackend` of their own (`MountTask`'s `swapon(8)` call,
//! `InitConfigTask`, `NixosInstallTask`, `EfiEntryTask`, `SetPasswordsTask`,
//! `PostInstallTask`). `PartitionTask`/`EncryptTask`/`FormatTask`/
//! `EnrollTpmTask` need no double at all: they only ever call through
//! `ctx.backends.disk`/`.crypt`, which `fake_pipeline` already points at
//! `debugsim::disks`/`debugsim::crypt`.
//!
//! Every label/weight below matches its real counterpart exactly, so the
//! progress bar's dynamics (dominated by `NixosInstallTask`'s weight of 200
//! out of 248) are unchanged in a fake run.

use super::fault::Fault;
use super::{FaultPlan, sim_sleep};
use crate::engine::tasks::{EncryptTask, EnrollTpmTask, FormatTask, PartitionTask, mount};
use crate::engine::{Pipeline, ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

/// Fault checking goes through an `Arc<FaultPlan>` carried on each fake task
/// struct, not through `debugsim::active()`'s global singleton: tests build
/// several independent `(DiskScenario, Fault)` combinations in one process,
/// and a `OnceLock` can only ever be set once. Production's only caller
/// (`finish::progress`) passes `debugsim::active().unwrap().faults.clone()`.
pub struct FakeMountTask {
    faults: Arc<FaultPlan>,
}

#[async_trait]
impl Task for FakeMountTask {
    fn label(&self) -> String {
        "Mounting target filesystems".to_string()
    }

    fn weight(&self) -> u32 {
        2
    }

    /// Mounts root/ESP/home for real through `mount::mount_filesystems`
    /// (purely in-memory on `debugsim::disks::FakeDiskBackend`) and fakes
    /// only `swapon(8)`, which isn't trait-abstracted on the real path.
    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        mount::mount_filesystems(ctx, tx).await?;

        let swap = ctx.state.lock().await.swap_partition.clone();
        if let Some(swap) = swap {
            if self.faults.is(Fault::MountSwapon) {
                return Err(mx::Error::Backend(format!("swapon: {swap}: swapon failed")));
            }
            let _ = tx
                .send(ProgressEvent::Log(format!("activated swap on {swap}")))
                .await;
        }
        Ok(())
    }
}

pub struct FakeInitConfigTask {
    faults: Arc<FaultPlan>,
}

#[async_trait]
impl Task for FakeInitConfigTask {
    fn label(&self) -> String {
        "Writing NixOS configuration".to_string()
    }

    fn weight(&self) -> u32 {
        20
    }

    async fn run(&self, _ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        let _ = tx
            .send(ProgressEvent::Log(
                "writing the NixOS configuration to /mnt/etc/modulix-os".into(),
            ))
            .await;
        sim_sleep(Duration::from_millis(300)).await;

        if self.faults.is(Fault::ConfigInit) {
            return Err(mx::Error::Backend(
                "core-utils init failed: git commit refused".into(),
            ));
        }

        let _ = tx
            .send(ProgressEvent::Log(
                "NixOS configuration written and committed".into(),
            ))
            .await;
        Ok(())
    }
}

/// Realistic Nix evaluation failure text — the exact historical bug CLAUDE.md
/// documents: a `luks.nix` of the installer's own invention re-declaring an
/// option `nixos-generate-config` already wrote.
const EVAL_ERROR: &str = "error: The option `boot.initrd.luks.devices.\"modulixroot\".device' has conflicting definition values:\n\
- In `/mnt/etc/modulix-os/fstab.nix': \"/dev/disk/by-uuid/11111111-2222-3333-4444-555555555555\"\n\
- In `/mnt/etc/modulix-os/luks.nix': \"/dev/disk/by-uuid/11111111-2222-3333-4444-555555555555\"\n\
Use `lib.mkForce value' or `lib.mkDefault value' to change the priority on any of these definitions.";

pub struct FakeNixosInstallTask {
    faults: Arc<FaultPlan>,
}

#[async_trait]
impl Task for FakeNixosInstallTask {
    fn label(&self) -> String {
        "Installing Modulix OS".to_string()
    }

    fn weight(&self) -> u32 {
        200
    }

    async fn run(&self, _ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        let _ = tx.send(ProgressEvent::Indeterminate(true)).await;

        const LOG_LINES: &[&str] = &[
            "building '/nix/store/00000000000000000000000000000000-modulixos-system.drv'...",
            "copying path '/nix/store/11111111111111111111111111111111-nixos-system-modulixos-25.11'...",
            "setting up /etc...",
            "activating the configuration...",
            "installing limine to /boot...",
        ];
        for line in LOG_LINES {
            let _ = tx.send(ProgressEvent::Log((*line).to_string())).await;
            sim_sleep(Duration::from_millis(400)).await;
        }

        let result = match self.faults.fault {
            Some(Fault::InstallEval) => Err(mx::Error::Backend(EVAL_ERROR.to_string())),
            Some(Fault::InstallBuild) => Err(mx::Error::Backend(
                "builder for '/nix/store/22222222222222222222222222222222-modulixos-system.drv' failed with exit code 1".into(),
            )),
            Some(Fault::InstallDownload) => Err(mx::Error::Backend(
                "unable to download 'https://github.com/Modulix-OS/mxpkgs/archive/main.tar.gz': Couldn't resolve host name".into(),
            )),
            Some(Fault::InstallDiskFull) => {
                Err(mx::Error::Backend("No space left on device".into()))
            }
            _ => Ok(()),
        };

        let _ = tx.send(ProgressEvent::Indeterminate(false)).await;
        result
    }
}

pub struct FakeEfiEntryTask {
    faults: Arc<FaultPlan>,
}

#[async_trait]
impl Task for FakeEfiEntryTask {
    fn label(&self) -> String {
        "Registering the boot entry".to_string()
    }

    fn weight(&self) -> u32 {
        1
    }

    /// Always best-effort, like the real task: an `efibootmgr` fault is
    /// logged and swallowed, never returned.
    async fn run(&self, _ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        let line = if self.faults.is(Fault::EfiEfibootmgr) {
            "efibootmgr --create ModulixOS: skipped (efibootmgr: command not found)".to_string()
        } else {
            "efibootmgr --create ModulixOS (fake0 part 1): done".to_string()
        };
        let _ = tx.send(ProgressEvent::Log(line)).await;
        Ok(())
    }
}

pub struct FakeSetPasswordsTask {
    faults: Arc<FaultPlan>,
}

#[async_trait]
impl Task for FakeSetPasswordsTask {
    fn label(&self) -> String {
        "Setting passwords".to_string()
    }

    fn weight(&self) -> u32 {
        2
    }

    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        let username = ctx.config.user.username.trim();
        if username.is_empty() {
            return Err(mx::Error::Backend("no username was collected".into()));
        }
        if self.faults.is(Fault::PasswdChpasswd) {
            return Err(mx::Error::Backend(
                "chpasswd: cannot lock /etc/shadow".into(),
            ));
        }
        let _ = tx
            .send(ProgressEvent::Log(format!(
                "passwords set for root and {username}"
            )))
            .await;
        Ok(())
    }
}

pub struct FakePostInstallTask {
    faults: Arc<FaultPlan>,
}

#[async_trait]
impl Task for FakePostInstallTask {
    fn label(&self) -> String {
        "Finishing up".to_string()
    }

    fn weight(&self) -> u32 {
        1
    }

    /// Always best-effort, like the real task: an `umount` fault is logged
    /// and swallowed, never returned.
    async fn run(&self, _ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        crate::engine::install_log::copy_to_target().await;
        let _ = tx
            .send(ProgressEvent::Log(format!(
                "install log copied to {}",
                crate::engine::install_log::TARGET_LOG
            )))
            .await;

        let line = if self.faults.is(Fault::PostUmountBusy) {
            "umount: skipped (target is busy)".to_string()
        } else {
            "umount: done".to_string()
        };
        let _ = tx.send(ProgressEvent::Log(line)).await;
        let _ = tx
            .send(ProgressEvent::Log("target system released".into()))
            .await;
        Ok(())
    }
}

/// Same ten-stage shape as `engine::full_pipeline`, with the four stages
/// that only ever go through `ctx.backends.disk`/`.crypt` left real — run
/// against `debugsim::disks`/`debugsim::crypt`, they exercise the real
/// `PartitionTask`/`EncryptTask`/`FormatTask`/`EnrollTpmTask` logic, not a
/// simulation of it.
///
/// * `faults` - the single `--fake-error=` fault to check inside the six
///   fake task doubles above; `debugsim::disks`/`debugsim::crypt` hold their
///   own clone of the same `Arc`, set up alongside it in [`super::DebugSim::new`].
pub fn fake_pipeline(faults: Arc<FaultPlan>) -> Pipeline {
    Pipeline::new(vec![
        Box::new(PartitionTask),
        Box::new(EncryptTask),
        Box::new(FormatTask),
        Box::new(FakeMountTask {
            faults: faults.clone(),
        }),
        Box::new(EnrollTpmTask),
        Box::new(FakeInitConfigTask {
            faults: faults.clone(),
        }),
        Box::new(FakeNixosInstallTask {
            faults: faults.clone(),
        }),
        Box::new(FakeEfiEntryTask {
            faults: faults.clone(),
        }),
        Box::new(FakeSetPasswordsTask {
            faults: faults.clone(),
        }),
        Box::new(FakePostInstallTask { faults }),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backends;
    use crate::backend::a11y::A11yBackend;
    use crate::backend::disk::scenario::DiskScenario;
    use crate::backend::locale::{KeyboardLayout, LocaleBackend, LocaleEntry, TimezoneEntry};
    use crate::backend::net::{
        Connectivity, NetworkBackend, PortalPage, WifiAccessPoint, WifiConnectRequest,
    };
    use crate::config::{InstallConfig, PartitionMode, SwapMode};
    use crate::debugsim::{FakeCryptBackend, FakeDiskBackend, FaultPlan};
    use std::sync::Arc;

    struct UnusedNetwork;
    #[async_trait]
    impl NetworkBackend for UnusedNetwork {
        async fn connectivity(&self) -> mx::Result<Connectivity> {
            unimplemented!("not exercised by pipeline tests")
        }
        async fn watch_connectivity(&self) -> mx::Result<async_channel::Receiver<Connectivity>> {
            unimplemented!("not exercised by pipeline tests")
        }
        async fn recheck_connectivity(&self) -> mx::Result<Connectivity> {
            unimplemented!("not exercised by pipeline tests")
        }
        async fn is_ethernet_connected(&self) -> mx::Result<bool> {
            unimplemented!("not exercised by pipeline tests")
        }
        async fn scan_wifi(&self) -> mx::Result<Vec<WifiAccessPoint>> {
            unimplemented!("not exercised by pipeline tests")
        }
        async fn connect_wifi(&self, _request: WifiConnectRequest) -> mx::Result<()> {
            unimplemented!("not exercised by pipeline tests")
        }
        async fn portal_page(&self) -> mx::Result<PortalPage> {
            unimplemented!("not exercised by pipeline tests")
        }
    }

    struct UnusedLocale;
    #[async_trait]
    impl LocaleBackend for UnusedLocale {
        async fn list_locales(&self) -> mx::Result<Vec<LocaleEntry>> {
            unimplemented!("not exercised by pipeline tests")
        }
        async fn list_timezones(&self) -> mx::Result<Vec<TimezoneEntry>> {
            unimplemented!("not exercised by pipeline tests")
        }
        async fn list_keyboard_layouts(&self) -> mx::Result<Vec<KeyboardLayout>> {
            unimplemented!("not exercised by pipeline tests")
        }
        async fn apply_keyboard_layout(&self, _layout: &str, _variant: &str) -> mx::Result<()> {
            unimplemented!("not exercised by pipeline tests")
        }
    }

    struct UnusedA11y;
    #[async_trait]
    impl A11yBackend for UnusedA11y {
        async fn set_narrator_enabled(&self, _enabled: bool) -> mx::Result<()> {
            unimplemented!("not exercised by pipeline tests")
        }
        async fn narrator_available(&self) -> bool {
            unimplemented!("not exercised by pipeline tests")
        }
        async fn set_high_contrast(&self, _enabled: bool) -> mx::Result<()> {
            unimplemented!("not exercised by pipeline tests")
        }
        async fn set_large_text(&self, _enabled: bool) -> mx::Result<()> {
            unimplemented!("not exercised by pipeline tests")
        }
        async fn set_screen_magnifier(&self, _enabled: bool) -> mx::Result<()> {
            unimplemented!("not exercised by pipeline tests")
        }
        async fn set_sticky_keys(&self, _enabled: bool) -> mx::Result<()> {
            unimplemented!("not exercised by pipeline tests")
        }
    }

    fn backends(scenarios: &[DiskScenario], fault: Option<Fault>) -> (Backends, Arc<FaultPlan>) {
        let faults = Arc::new(FaultPlan { fault });
        let backends = Backends {
            disk: Arc::new(FakeDiskBackend::new(scenarios, faults.clone())),
            network: Arc::new(UnusedNetwork),
            locale: Arc::new(UnusedLocale),
            a11y: Arc::new(UnusedA11y),
            crypt: Arc::new(FakeCryptBackend::new(faults.clone())),
        };
        (backends, faults)
    }

    fn config_for(disk: &str, mode: PartitionMode) -> InstallConfig {
        let mut cfg = InstallConfig::default();
        cfg.user.username = "demo".into();
        cfg.partitioning.mode = mode;
        cfg.partitioning.target_disk = Some(disk.to_string());
        cfg.partitioning.swap_mode = SwapMode::None;
        cfg
    }

    // `--fake-instant` is irrelevant to correctness; every test here pays
    // the real (short) sleeps rather than reaching into the global
    // `DebugSim` singleton, which is shared and install-order-sensitive.

    #[tokio::test]
    async fn linux_entire_disk_succeeds() {
        let (backends, faults) = backends(&[DiskScenario::Linux], None);
        let config = config_for("/dev/fake0", PartitionMode::EntireDisk);
        let ctx = TaskCtx::with_uefi(backends, config, true);
        let (tx, _rx) = async_channel::unbounded();

        fake_pipeline(faults).run(&ctx, &tx).await.unwrap();
    }

    #[tokio::test]
    async fn encrypted_root_rewrites_the_pipeline_state_to_the_mapper() {
        let (backends, faults) = backends(&[DiskScenario::Empty], None);
        let mut config = config_for("/dev/fake0", PartitionMode::EntireDisk);
        config.partitioning.encryption_enabled = true;
        config.partitioning.encryption_passphrase = "correct horse battery staple".into();
        let ctx = TaskCtx::with_uefi(backends, config, true);
        let (tx, _rx) = async_channel::unbounded();

        fake_pipeline(faults).run(&ctx, &tx).await.unwrap();

        let state = ctx.state.lock().await;
        assert_eq!(
            state.root_partition.as_deref(),
            Some("/dev/mapper/modulixroot")
        );
    }

    #[tokio::test]
    async fn luks_format_fault_fails_at_the_encrypt_stage() {
        let (backends, faults) = backends(&[DiskScenario::Empty], Some(Fault::EncryptLuksFormat));
        let mut config = config_for("/dev/fake0", PartitionMode::EntireDisk);
        config.partitioning.encryption_enabled = true;
        config.partitioning.encryption_passphrase = "correct horse battery staple".into();
        let ctx = TaskCtx::with_uefi(backends, config, true);
        let (tx, _rx) = async_channel::unbounded();

        let err = fake_pipeline(faults).run(&ctx, &tx).await.unwrap_err();
        assert!(mx::render(&err).contains("status 4"));
    }

    #[tokio::test]
    async fn install_eval_fault_fails_at_the_nixos_install_stage() {
        let (backends, faults) = backends(&[DiskScenario::Linux], Some(Fault::InstallEval));
        let config = config_for("/dev/fake0", PartitionMode::EntireDisk);
        let ctx = TaskCtx::with_uefi(backends, config, true);
        let (tx, _rx) = async_channel::unbounded();

        fake_pipeline(faults).run(&ctx, &tx).await.unwrap_err();
    }

    #[tokio::test]
    async fn efi_fault_does_not_fail_the_install() {
        let (backends, faults) = backends(&[DiskScenario::Linux], Some(Fault::EfiEfibootmgr));
        let config = config_for("/dev/fake0", PartitionMode::EntireDisk);
        let ctx = TaskCtx::with_uefi(backends, config, true);
        let (tx, _rx) = async_channel::unbounded();

        fake_pipeline(faults).run(&ctx, &tx).await.unwrap();
    }

    #[tokio::test]
    async fn post_umount_fault_does_not_fail_the_install() {
        let (backends, faults) = backends(&[DiskScenario::Linux], Some(Fault::PostUmountBusy));
        let config = config_for("/dev/fake0", PartitionMode::EntireDisk);
        let ctx = TaskCtx::with_uefi(backends, config, true);
        let (tx, _rx) = async_channel::unbounded();

        fake_pipeline(faults).run(&ctx, &tx).await.unwrap();
    }
}
