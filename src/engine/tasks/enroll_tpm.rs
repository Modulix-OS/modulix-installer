use crate::engine::{ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;

/// TPM2 enrollment only — `luksFormat`/`luksOpen` live in `EncryptTask`,
/// which runs earlier (right after `PartitionTask`, before `FormatTask`
/// mkfs's the decrypted mapper devices). Enrolling here, after `MountTask`,
/// is fine: `systemd-cryptenroll` operates on the raw LUKS containers
/// (`PipelineState::luks_device`, `::luks_swap_device`), which don't change
/// once opened.
///
/// Both containers are enrolled. Skipping the swap one would leave the boot
/// unlocking root silently through the TPM and then asking for a passphrase
/// for the swap, since nothing would have put one in the kernel keyring.
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

        let (root, swap) = {
            let state = ctx.state.lock().await;
            (state.luks_device.clone(), state.luks_swap_device.clone())
        };
        let root =
            root.ok_or_else(|| mx::Error::Backend("no LUKS device to enroll TPM2 against".into()))?;

        for container in std::iter::once(root).chain(swap) {
            let device = ctx.backends.disk.device_node(&container).await?;
            ctx.backends
                .crypt
                .enroll_tpm2(
                    &device,
                    &ctx.config.partitioning.encryption_passphrase,
                    ctx.config.partitioning.tpm2_pin.as_deref(),
                )
                .await?;
            let _ = tx
                .send(ProgressEvent::Log(format!("enrolled TPM2 on {device}")))
                .await;
        }
        Ok(())
    }
}
