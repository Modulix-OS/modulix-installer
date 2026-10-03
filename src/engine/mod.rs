pub mod install_log;
pub mod live_input;
pub mod plan;
pub mod sizing;
pub mod tasks;

use crate::backend::Backends;
use crate::backend::disk::FormatFs;
use crate::config::InstallConfig;
use crate::mx;
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub enum ProgressEvent {
    Started {
        task: String,
    },
    Log(String),
    Progress {
        fraction: f64,
    },
    /// Switches the progress bar between pulsing and fraction mode. Sent by
    /// the long tasks that cannot report a fraction of their own
    /// (`NixosInstallTask`), so the bar does not sit frozen for the whole
    /// `nixos-install`.
    Indeterminate(bool),
    Finished {
        task: String,
    },
}

pub type ProgressSink = async_channel::Sender<ProgressEvent>;

/// Where the target system is mounted. Not configurable: the whole
/// modulix-core-utils install path (and `nixos-install --root`) is pinned to
/// this prefix.
pub const INSTALL_ROOT: &str = "/mnt";

/// NixOS configuration repository written by `InitConfigTask`, inside the
/// target root. Passed explicitly as `InitParams::config_dir` so debug and
/// release builds resolve it identically (`CONFIG_DIRECTORY` otherwise points
/// at modulix-core-utils' own `test/` tree in debug).
pub const CONFIG_REPO: &str = "/mnt/etc/modulix-os";

/// `dm-crypt` mapper name the root LUKS container is opened as, and the
/// `boot.initrd.luks.devices` entry the installed system unlocks it through.
pub const LUKS_MAPPER_NAME: &str = "modulixroot";

/// Flake attribute `init` writes into `nixosConfigurations`
/// (modulix-core-utils' private `CONFIG_NAME`).
pub const CONFIG_FLAKE_ATTR: &str = "default";

/// Partition paths and per-partition format decisions handed off between
/// pipeline stages. `PartitionTask` fills these in from `engine::plan`'s
/// output; `EncryptTask` may rewrite `root_partition` to a `/dev/mapper/…`
/// path; `FormatTask`/`MountTask`/`EnrollTpmTask` consume them.
///
/// `{role}_format` is `None` for a partition `plan::plan` reused as-is
/// (`PlanOp::UseExisting { format: None, .. }`, e.g. an existing Windows ESP
/// or a Manual-mode partition the user chose not to reformat) — `FormatTask`
/// must never call `format_partition` on those (never reformat the Windows
/// ESP, never destroy data the plan didn't intend to erase).
/// The ESP additionally gets its own `esp_is_new` rather than an
/// `efi_format` field, since its target filesystem is always FAT32 and
/// never configurable.
#[derive(Debug, Clone, Default)]
pub struct PipelineState {
    pub efi_partition: Option<String>,
    pub esp_is_new: bool,
    pub root_partition: Option<String>,
    pub root_format: Option<FormatFs>,
    pub swap_partition: Option<String>,
    pub swap_format: Option<FormatFs>,
    pub home_partition: Option<String>,
    pub home_format: Option<FormatFs>,
    /// The raw (pre-`luksOpen`) LUKS container device — `EnrollTpmTask`
    /// enrolls the TPM2 against this, not the `/dev/mapper/…` path.
    pub luks_device: Option<String>,
}

/// Everything a [`Task`] needs: the backends it acts through, the answers
/// collected by the wizard, the firmware mode the plan is computed for, and
/// the mutable hand-off state between stages.
pub struct TaskCtx {
    pub backends: Backends,
    pub config: InstallConfig,
    /// Whether the live system booted in UEFI mode — `PartitionTask` feeds
    /// this to `live_input::gather_plan_input`, which decides from it alone
    /// whether the plan carves a new ESP. Held here instead of probed per
    /// task so tests can pin it (`/sys/firmware/efi` does not exist inside a
    /// Nix build sandbox, which would otherwise silently turn every pipeline
    /// test into a BIOS-mode one).
    pub uefi: bool,
    pub state: tokio::sync::Mutex<PipelineState>,
}

impl TaskCtx {
    /// Builds a context probing the running system for its firmware mode.
    ///
    /// * `backends` — subsystem backends every task acts through.
    /// * `config` — the wizard answers driving the install.
    pub fn new(backends: Backends, config: InstallConfig) -> Self {
        Self::with_uefi(backends, config, live_input::detect_uefi())
    }

    /// Builds a context with the firmware mode pinned explicitly.
    ///
    /// * `backends` — subsystem backends every task acts through.
    /// * `config` — the wizard answers driving the install.
    /// * `uefi` — firmware mode the plan is computed for, see [`TaskCtx::uefi`].
    pub fn with_uefi(backends: Backends, config: InstallConfig, uefi: bool) -> Self {
        Self {
            backends,
            config,
            uefi,
            state: tokio::sync::Mutex::new(PipelineState::default()),
        }
    }
}

/// One stage of the install pipeline (see `tasks/`), weighted for the
/// progress bar.
#[async_trait]
pub trait Task: Send + Sync {
    fn label(&self) -> String;
    fn weight(&self) -> u32;
    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()>;
}

pub struct Pipeline {
    tasks: Vec<Box<dyn Task>>,
}

impl Pipeline {
    pub fn new(tasks: Vec<Box<dyn Task>>) -> Self {
        Self { tasks }
    }

    pub fn total_weight(&self) -> u32 {
        self.tasks.iter().map(|t| t.weight()).sum()
    }

    pub async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        let total = self.total_weight().max(1) as f64;
        let mut done = 0u32;
        for task in &self.tasks {
            let _ = tx.send(ProgressEvent::Started { task: task.label() }).await;
            task.run(ctx, tx).await?;
            done += task.weight();
            let _ = tx
                .send(ProgressEvent::Progress {
                    fraction: (done as f64 / total).min(1.0),
                })
                .await;
            let _ = tx
                .send(ProgressEvent::Finished { task: task.label() })
                .await;
        }
        Ok(())
    }
}

/// The full install pipeline.
///
/// Order constraints:
/// * `EncryptTask` runs right after `PartitionTask` and before
///   `FormatTask`/`MountTask` — `luksFormat`/`luksOpen` need the bare
///   partition, not one already `mkfs`'d and mounted on `/mnt`.
/// * `InitConfigTask` needs the target mounted, `ExtraConfigTask` needs the
///   configuration repository it creates, and `NixosInstallTask` needs both
///   committed.
/// * `SetPasswordsTask` needs an installed system to `nixos-enter` into, so
///   it cannot move earlier.
/// * `EfiEntryTask` needs limine's loader already on the ESP, so it follows
///   `NixosInstallTask`, and it needs `/mnt` still mounted, so it precedes
///   `PostInstallTask`.
pub fn full_pipeline() -> Pipeline {
    Pipeline::new(vec![
        Box::new(tasks::PartitionTask),
        Box::new(tasks::EncryptTask),
        Box::new(tasks::FormatTask),
        Box::new(tasks::MountTask),
        Box::new(tasks::EnrollTpmTask),
        Box::new(tasks::InitConfigTask),
        Box::new(tasks::ExtraConfigTask),
        Box::new(tasks::NixosInstallTask),
        Box::new(tasks::EfiEntryTask),
        Box::new(tasks::SetPasswordsTask),
        Box::new(tasks::PostInstallTask),
    ])
}
