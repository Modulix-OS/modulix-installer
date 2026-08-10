use crate::engine::{ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;

/// Filesystem label written to the root partition — 10 chars, fits every
/// `FormatFs` variant's label limit (xfs is tightest, at 12).
const ROOT_PARTITION_LABEL: &str = "Modulix OS";

pub struct FormatTask;

#[async_trait]
impl Task for FormatTask {
    fn label(&self) -> String {
        "Formatting partitions".to_string()
    }

    fn weight(&self) -> u32 {
        5
    }

    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        let state = ctx.state.lock().await.clone();

        // `PartitionTask` already ran every `PlanOp::UseExisting { format:
        // None, .. }` op without touching the filesystem — `None` here means
        // "never format this partition" (a reused Windows ESP, or a
        // Manual-mode partition the user chose to keep), not "not decided
        // yet". Only a `Some(fs)` — a freshly created partition, or an
        // explicit reformat request — gets `format_partition` called on it.
        if let Some(efi) = state.efi_partition.as_ref().filter(|_| state.esp_is_new) {
            ctx.backends
                .disk
                .format_partition(efi, crate::backend::disk::FormatFs::Fat32, None)
                .await?;
            let _ = tx
                .send(ProgressEvent::Log(format!("formatted {efi} as FAT32")))
                .await;
        }
        if let (Some(root), Some(fs)) = (&state.root_partition, state.root_format) {
            ctx.backends
                .disk
                .format_partition(root, fs, Some(ROOT_PARTITION_LABEL))
                .await?;
            let _ = tx
                .send(ProgressEvent::Log(format!(
                    "formatted {root} as {}",
                    fs.as_mkfs_type()
                )))
                .await;
        }
        if let (Some(swap), Some(fs)) = (&state.swap_partition, state.swap_format) {
            ctx.backends.disk.format_partition(swap, fs, None).await?;
            let _ = tx
                .send(ProgressEvent::Log(format!("formatted {swap} as swap")))
                .await;
        }
        if let (Some(home), Some(fs)) = (&state.home_partition, state.home_format) {
            ctx.backends.disk.format_partition(home, fs, None).await?;
            let _ = tx
                .send(ProgressEvent::Log(format!(
                    "formatted {home} as {}",
                    fs.as_mkfs_type()
                )))
                .await;
        }
        Ok(())
    }
}
