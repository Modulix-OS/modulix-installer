use crate::engine::{ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;

/// TPM2 enrollment only — `luksFormat`/`luksOpen` now live in `EncryptTask`,
/// which runs earlier (right after `PartitionTask`, before `FormatTask`
/// mkfs's the decrypted mapper device). Enrolling here, after `MountTask`,
/// is fine: `systemd-cryptenroll` operates on the raw LUKS container
/// (`PipelineState::luks_device`), which doesn't change once opened.
pub struct EnrollTpmTask;

#[async_trait]
impl Task for EnrollTpmTask {
    fn label(&self) -> String {
        "Enrolling TPM2".to_string()
    }

    fn weight(&self) -> u32 {
        2
    }

    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        if !ctx.config.partitioning.encryption_enabled || !ctx.config.partitioning.tpm2_enabled {
            return Ok(());
        }

        let device =
            ctx.state.lock().await.luks_device.clone().ok_or_else(|| {
                mx::Error::Backend("no LUKS device to enroll TPM2 against".into())
            })?;

        ctx.backends
            .crypt
            .enroll_tpm2(&device, ctx.config.partitioning.tpm2_pin.as_deref())
            .await?;
        let _ = tx
            .send(ProgressEvent::Log(format!("enrolled TPM2 on {device}")))
            .await;
        Ok(())
    }
}
