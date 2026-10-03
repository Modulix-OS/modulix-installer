use crate::engine::{LUKS_MAPPER_NAME as MAPPER_NAME, ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;

/// `luksFormat` + `luksOpen` on the root partition, run right after
/// `PartitionTask` and before `FormatTask`/`MountTask` — the corrected
/// pipeline order (see `engine::full_pipeline`). Rewrites
/// `PipelineState::root_partition` to the opened `/dev/mapper/…` path so
/// every later stage (`FormatTask`, `MountTask`) formats/mounts the
/// decrypted device, never the raw LUKS container.
pub struct EncryptTask;

#[async_trait]
impl Task for EncryptTask {
    fn label(&self) -> String {
        "Encrypting root partition".to_string()
    }

    fn weight(&self) -> u32 {
        5
    }

    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        if !ctx.config.partitioning.encryption_enabled {
            return Ok(());
        }
        if ctx
            .config
            .partitioning
            .encryption_passphrase
            .trim()
            .is_empty()
        {
            return Err(mx::Error::Backend(
                "encryption is enabled but no passphrase was set".into(),
            ));
        }

        let raw_root = ctx
            .state
            .lock()
            .await
            .root_partition
            .clone()
            .ok_or_else(|| mx::Error::Backend("no root partition to encrypt".into()))?;

        let passphrase = &ctx.config.partitioning.encryption_passphrase;
        ctx.backends
            .crypt
            .luks_format(&raw_root, passphrase)
            .await?;
        let _ = tx
            .send(ProgressEvent::Log(format!("LUKS2-formatted {raw_root}")))
            .await;

        ctx.backends
            .crypt
            .luks_open(&raw_root, MAPPER_NAME, passphrase)
            .await?;
        let mapper_node = format!("/dev/mapper/{MAPPER_NAME}");
        let mapper_path = ctx.backends.disk.resolve_device(&mapper_node).await?;
        let _ = tx
            .send(ProgressEvent::Log(format!(
                "opened {raw_root} as {mapper_node}"
            )))
            .await;

        let mut state = ctx.state.lock().await;
        state.luks_device = Some(raw_root);
        state.root_partition = Some(mapper_path);
        Ok(())
    }
}
