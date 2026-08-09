use crate::config::PartitionMode;
use crate::engine::sizing::{compute_swap_bytes, detect_ram_bytes};
use crate::engine::{ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;

const EFI_SIZE_BYTES: u64 = 512 * 1024 * 1024;

pub struct PartitionTask;

#[async_trait]
impl Task for PartitionTask {
    fn label(&self) -> String {
        "Partitioning disk".to_string()
    }

    fn weight(&self) -> u32 {
        10
    }

    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        if ctx.config.partitioning.mode == PartitionMode::Manual {
            let _ = tx
                .send(ProgressEvent::Log(
                    "manual partitioning selected, skipping".into(),
                ))
                .await;
            return Ok(());
        }

        let disk = ctx
            .config
            .partitioning
            .target_disk
            .as_deref()
            .ok_or_else(|| mx::Error::Backend("no target disk selected".into()))?;

        let disk_info = ctx
            .backends
            .disk
            .list_disks()
            .await?
            .into_iter()
            .find(|d| d.path == disk)
            .ok_or_else(|| mx::Error::Backend(format!("unknown disk: {disk}")))?;

        let ram_bytes = detect_ram_bytes().await.unwrap_or(0);
        let swap_bytes = compute_swap_bytes(ctx.config.partitioning.swap_mode, ram_bytes);

        let efi_path = ctx
            .backends
            .disk
            .create_partition(disk, 0, EFI_SIZE_BYTES)
            .await?;
        let _ = tx
            .send(ProgressEvent::Log(format!(
                "created EFI partition {efi_path}"
            )))
            .await;

        let root_size = disk_info
            .size_bytes
            .saturating_sub(EFI_SIZE_BYTES)
            .saturating_sub(swap_bytes);
        let root_path = ctx
            .backends
            .disk
            .create_partition(disk, EFI_SIZE_BYTES, root_size)
            .await?;
        let _ = tx
            .send(ProgressEvent::Log(format!(
                "created root partition {root_path}"
            )))
            .await;

        let mut swap_path = None;
        if swap_bytes > 0 {
            let start = EFI_SIZE_BYTES + root_size;
            let path = ctx
                .backends
                .disk
                .create_partition(disk, start, swap_bytes)
                .await?;
            let _ = tx
                .send(ProgressEvent::Log(format!("created swap partition {path}")))
                .await;
            swap_path = Some(path);
        }

        let mut state = ctx.state.lock().await;
        state.efi_partition = Some(efi_path);
        state.root_partition = Some(root_path);
        state.swap_partition = swap_path;
        Ok(())
    }
}
