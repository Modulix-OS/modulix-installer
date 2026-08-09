pub mod sizing;
pub mod tasks;

use crate::backend::Backends;
use crate::config::InstallConfig;
use crate::mx;
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub enum ProgressEvent {
    Started { task: String },
    Log(String),
    Finished { task: String },
}

pub type ProgressSink = async_channel::Sender<ProgressEvent>;

/// Partition paths handed off between pipeline stages (`PartitionTask`
/// creates them, `FormatTask`/`MountTask`/`EnrollTpmTask` consume them).
#[derive(Debug, Clone, Default)]
pub struct PipelineState {
    pub efi_partition: Option<String>,
    pub root_partition: Option<String>,
    pub swap_partition: Option<String>,
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
        for task in &self.tasks {
            let _ = tx.send(ProgressEvent::Started { task: task.label() }).await;
            task.run(ctx, tx).await?;
            let _ = tx
                .send(ProgressEvent::Finished { task: task.label() })
                .await;
        }
        Ok(())
    }
}

/// The full iteration-2 install pipeline. `InitConfigTask`/`PostInstallTask`
/// are no-op stubs until `modulix-core-utils` grows `init::init_all` (see
/// CLAUDE.md) — wiring them up then is a one-line change inside those tasks.
pub fn full_pipeline() -> Pipeline {
    Pipeline::new(vec![
        Box::new(tasks::PartitionTask),
        Box::new(tasks::FormatTask),
        Box::new(tasks::MountTask),
        Box::new(tasks::EnrollTpmTask),
        Box::new(tasks::InitConfigTask),
        Box::new(tasks::PostInstallTask),
    ])
}
