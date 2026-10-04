use crate::engine::{
    LUKS_MAPPER_NAME as MAPPER_NAME, LUKS_SWAP_MAPPER_NAME as SWAP_MAPPER_NAME, ProgressEvent,
    ProgressSink, Task, TaskCtx,
};
use crate::mx;
use async_trait::async_trait;

/// `luksFormat` + `luksOpen` on every container of the target, run right
/// after `PartitionTask` and before `FormatTask`/`MountTask` — the corrected
/// pipeline order (see `engine::full_pipeline`). Rewrites
/// `PipelineState::root_partition` and `::swap_partition` to the opened
/// mappers so every later stage (`FormatTask`, `MountTask`) formats/mounts
/// the decrypted devices, never the raw LUKS containers.
///
/// Both containers take the same passphrase, so the installed system asks
/// once: `systemd-cryptsetup` caches it in the kernel keyring and reuses it
/// for the second volume, and with TPM2 enrolled (`EnrollTpmTask` does both)
/// neither is asked for at all.
pub struct EncryptTask;

#[async_trait]
impl Task for EncryptTask {
    fn label(&self) -> String {
        "Encrypting the disk".to_string()
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

        let (raw_root, raw_swap) = {
            let state = ctx.state.lock().await;
            (state.root_partition.clone(), state.swap_partition.clone())
        };
        let raw_root =
            raw_root.ok_or_else(|| mx::Error::Backend("no root partition to encrypt".into()))?;

        let passphrase = &ctx.config.partitioning.encryption_passphrase;
        let root_mapper = encrypt_one(ctx, tx, &raw_root, MAPPER_NAME, passphrase).await?;

        let swap_mapper = match &raw_swap {
            Some(raw_swap) => {
                Some(encrypt_one(ctx, tx, raw_swap, SWAP_MAPPER_NAME, passphrase).await?)
            }
            None => None,
        };

        let mut state = ctx.state.lock().await;
        state.luks_device = Some(raw_root);
        state.root_partition = Some(root_mapper);
        if let Some(swap_mapper) = swap_mapper {
            state.luks_swap_device = raw_swap;
            state.swap_partition = Some(swap_mapper);
        }
        Ok(())
    }
}

/// LUKS2-formats one partition, opens it, and reports the udisks2 object path
/// of the resulting mapper.
///
/// # Parameters
/// * `ctx` - task context, for the disk and crypt backends.
/// * `tx` - progress sink the two log lines go to.
/// * `partition` - udisks2 object path of the bare partition to encrypt; it
///   must not be formatted or mounted yet.
/// * `mapper_name` - `dm-crypt` name to open the container as.
/// * `passphrase` - passphrase to enroll in the container's first key slot.
///
/// # Post-conditions
/// `partition` holds a LUKS2 header, `/dev/mapper/<mapper_name>` exists, and
/// udisks2 has noticed it.
///
/// # Returns
/// The mapper's udisks2 object path, usable like any other partition path.
///
/// # Errors
/// [`mx::Error`] from `device_node`, `luksFormat`, `luksOpen`, or from
/// `resolve_device` if udisks2 never sees the mapper.
async fn encrypt_one(
    ctx: &TaskCtx,
    tx: &ProgressSink,
    partition: &str,
    mapper_name: &str,
    passphrase: &str,
) -> mx::Result<String> {
    let node = ctx.backends.disk.device_node(partition).await?;

    ctx.backends.crypt.luks_format(&node, passphrase).await?;
    let _ = tx
        .send(ProgressEvent::Log(format!("LUKS2-formatted {node}")))
        .await;

    ctx.backends
        .crypt
        .luks_open(&node, mapper_name, passphrase)
        .await?;
    let mapper_node = format!("/dev/mapper/{mapper_name}");
    let mapper_path = ctx.backends.disk.resolve_device(&mapper_node).await?;
    let _ = tx
        .send(ProgressEvent::Log(format!(
            "opened {node} as {mapper_node}"
        )))
        .await;

    Ok(mapper_path)
}
