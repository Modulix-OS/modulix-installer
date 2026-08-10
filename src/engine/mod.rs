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
    Started { task: String },
    Log(String),
    Progress { fraction: f64 },
    Finished { task: String },
}

pub type ProgressSink = async_channel::Sender<ProgressEvent>;

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

pub struct TaskCtx {
    pub backends: Backends,
    pub config: InstallConfig,
    pub state: tokio::sync::Mutex<PipelineState>,
}

impl TaskCtx {
    pub fn new(backends: Backends, config: InstallConfig) -> Self {
        Self {
            backends,
            config,
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

/// The full iteration-2 install pipeline. `InitConfigTask`/`PostInstallTask`
/// are no-op stubs until `modulix-core-utils` grows `init::init_all` —
/// wiring them up then is a one-line change inside those tasks.
///
/// `EncryptTask` must run right after `PartitionTask` and before
/// `FormatTask`/`MountTask` — `luksFormat`/`luksOpen` need the bare
/// partition, not one that's already been `mkfs`'d and mounted on `/mnt`.
pub fn full_pipeline() -> Pipeline {
    Pipeline::new(vec![
        Box::new(tasks::PartitionTask),
        Box::new(tasks::EncryptTask),
        Box::new(tasks::FormatTask),
        Box::new(tasks::MountTask),
        Box::new(tasks::EnrollTpmTask),
        Box::new(tasks::InitConfigTask),
        Box::new(tasks::PostInstallTask),
    ])
}

#[cfg(test)]
mod pipeline_tests {
    use super::*;
    use crate::backend::disk::scenario::DiskScenario;
    use crate::config::{InstallConfig, PartitionMode, SwapMode};

    // `MountTask::run`'s `swapon` call shells out to the real `swapon(8)`
    // regardless of which `DiskBackend` is behind it (unlike every other
    // disk operation, it isn't trait-abstracted) — every test here uses
    // `SwapMode::None` so that call is never reached. Swap activation itself
    // needs real-hardware/VM verification, same as TPM2+Limine needing it
    // before exposing that in the UI.

    fn config_for(scenario_disk: &str, mode: PartitionMode) -> InstallConfig {
        let mut cfg = InstallConfig::default();
        cfg.partitioning.mode = mode;
        cfg.partitioning.target_disk = Some(scenario_disk.to_string());
        cfg.partitioning.swap_mode = SwapMode::None;
        cfg
    }

    #[tokio::test]
    async fn linux_scenario_entire_disk_produces_esp_and_root() {
        let backends = crate::backend::Backends::fake_with(DiskScenario::Linux);
        let config = config_for("/dev/fake0", PartitionMode::EntireDisk);
        let ctx = TaskCtx::new(backends.clone(), config);
        let (tx, _rx) = async_channel::unbounded();

        full_pipeline().run(&ctx, &tx).await.unwrap();

        let partitions = backends.disk.list_partitions("/dev/fake0").await.unwrap();
        assert_eq!(partitions.len(), 2);
        assert!(
            partitions
                .iter()
                .any(|p| p.fs_type.as_deref() == Some("vfat"))
        );
        assert!(
            partitions
                .iter()
                .any(|p| p.fs_type.as_deref() == Some("ext4"))
        );
    }

    #[tokio::test]
    async fn empty_scenario_entire_disk_with_standard_swap_formats_all_three() {
        let backends = crate::backend::Backends::fake_with(DiskScenario::Empty);
        let mut config = config_for("/dev/fake0", PartitionMode::EntireDisk);
        config.partitioning.swap_mode = SwapMode::Standard;
        let ctx = TaskCtx::new(backends.clone(), config);
        let (tx, _rx) = async_channel::unbounded();

        // `swapon` would run for real here once `MountTask` reaches it — cut
        // the pipeline short at `PartitionTask` + `FormatTask` instead of
        // running `full_pipeline()`, to test formatting without touching a
        // real syscall.
        let short_pipeline = Pipeline::new(vec![
            Box::new(tasks::PartitionTask),
            Box::new(tasks::FormatTask),
        ]);
        short_pipeline.run(&ctx, &tx).await.unwrap();

        let partitions = backends.disk.list_partitions("/dev/fake0").await.unwrap();
        assert_eq!(partitions.len(), 3);
        assert!(
            partitions
                .iter()
                .any(|p| p.fs_type.as_deref() == Some("vfat"))
        );
        assert!(
            partitions
                .iter()
                .any(|p| p.fs_type.as_deref() == Some("ext4"))
        );
        assert!(
            partitions
                .iter()
                .any(|p| p.fs_type.as_deref() == Some("swap"))
        );
    }

    #[tokio::test]
    async fn encrypted_root_formats_the_luks_mapper_not_the_raw_partition() {
        let backends = crate::backend::Backends::fake_with(DiskScenario::Empty);
        let mut config = config_for("/dev/fake0", PartitionMode::EntireDisk);
        config.partitioning.encryption_enabled = true;
        config.partitioning.encryption_passphrase = "correct horse battery staple".into();
        config.partitioning.tpm2_enabled = true;
        let ctx = TaskCtx::new(backends.clone(), config);
        let (tx, _rx) = async_channel::unbounded();

        full_pipeline().run(&ctx, &tx).await.unwrap();

        let state = ctx.state.lock().await;
        assert_eq!(
            state.root_partition.as_deref(),
            Some("/dev/mapper/modulixroot")
        );
        assert!(state.luks_device.is_some());
        drop(state);

        // The raw partition (pre-`luksOpen`) must never have been formatted
        // directly — only the opened mapper device should be.
        let partitions = backends.disk.list_partitions("/dev/fake0").await.unwrap();
        let raw_root = partitions
            .iter()
            .find(|p| p.fs_type.as_deref() != Some("vfat"))
            .expect("a non-ESP partition exists");
        assert_eq!(raw_root.fs_type, None);
    }

    #[tokio::test]
    async fn windows_scenario_alongside_windows_shrinks_and_adds_linux_partitions() {
        let backends = crate::backend::Backends::fake_with(DiskScenario::Windows);
        let config = config_for("/dev/fake0", PartitionMode::AlongsideWindows);
        let ctx = TaskCtx::new(backends.clone(), config);
        let (tx, _rx) = async_channel::unbounded();

        full_pipeline().run(&ctx, &tx).await.unwrap();

        let partitions = backends.disk.list_partitions("/dev/fake0").await.unwrap();
        // Windows ESP + MSR + shrunk NTFS + WinRE (all pre-existing) + a new
        // Modulix ESP of its own + new root — Modulix never reuses/reformats
        // the Windows ESP (see `engine::plan::NEW_ESP_BYTES`'s doc comment).
        assert_eq!(partitions.len(), 6);
        let ntfs = partitions
            .iter()
            .find(|p| p.fs_type.as_deref() == Some("ntfs"))
            .expect("Windows partition still present");
        assert!(ntfs.size_bytes < 400 * 1024 * 1024 * 1024);
        assert!(
            partitions
                .iter()
                .any(|p| p.fs_type.as_deref() == Some("ext4"))
        );
    }
}
