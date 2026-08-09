use crate::engine::{ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;

pub struct EnrollTpmTask;

#[async_trait]
impl Task for EnrollTpmTask {
    fn label(&self) -> String {
        "Setting up encryption".to_string()
    }

    fn weight(&self) -> u32 {
        3
    }

    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        if !ctx.config.partitioning.encryption_enabled {
            return Ok(());
        }

        let root = ctx.state.lock().await.root_partition.clone();
        let root = root.ok_or_else(|| mx::Error::Backend("no root partition to encrypt".into()))?;

        ctx.backends
            .crypt
            .luks_format(&root, &ctx.config.partitioning.encryption_passphrase)
            .await?;
        let _ = tx
            .send(ProgressEvent::Log(format!("LUKS2-formatted {root}")))
            .await;

        if ctx.config.partitioning.tpm2_enabled {
            ctx.backends
                .crypt
                .enroll_tpm2(&root, ctx.config.partitioning.tpm2_pin.as_deref())
                .await?;
            let _ = tx.send(ProgressEvent::Log("enrolled TPM2".into())).await;
        }
        Ok(())
    }
}
