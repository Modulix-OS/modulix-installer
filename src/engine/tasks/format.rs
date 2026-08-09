use crate::backend::disk::FormatFs;
use crate::engine::{ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;

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

        if let Some(efi) = &state.efi_partition {
            ctx.backends
                .disk
                .format_partition(efi, FormatFs::Fat32)
                .await?;
            let _ = tx
                .send(ProgressEvent::Log(format!("formatted {efi} as FAT32")))
                .await;
        }
        if let Some(root) = &state.root_partition {
            ctx.backends
                .disk
                .format_partition(root, FormatFs::Ext4)
                .await?;
            let _ = tx
                .send(ProgressEvent::Log(format!("formatted {root} as ext4")))
                .await;
        }
        if let Some(swap) = &state.swap_partition {
            ctx.backends
                .disk
                .format_partition(swap, FormatFs::LinuxSwap)
                .await?;
            let _ = tx
                .send(ProgressEvent::Log(format!("formatted {swap} as swap")))
                .await;
        }
        Ok(())
    }
}
