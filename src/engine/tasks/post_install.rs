use crate::engine::tasks::{best_effort, report};
use crate::engine::{
    INSTALL_ROOT, LUKS_MAPPER_NAME, ProgressEvent, ProgressSink, Task, TaskCtx, install_log,
};
use crate::mx;
use async_trait::async_trait;

/// Releases the target system: swap off, `/mnt` unmounted, LUKS mapper
/// closed.
///
/// Every step is best-effort and never fails the install: the system is
/// already on disk by the time this runs, and refusing to finish because a
/// lazy unmount had to wait would be worse than leaving a mount behind. What
/// it does buy is a clean second run in the same live session — otherwise
/// `PartitionTask` would find `/mnt` still busy and the mapper still open.
pub struct PostInstallTask;

#[async_trait]
impl Task for PostInstallTask {
    fn label(&self) -> String {
        "Finishing up".to_string()
    }

    fn weight(&self) -> u32 {
        1
    }

    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        let state = ctx.state.lock().await.clone();

        // Last chance: the log copy has to be taken while the target is still
        // mounted. On the failure path the pipeline never gets here, and the
        // tee in `finish::progress` does it instead.
        install_log::copy_to_target().await;
        let _ = tx
            .send(ProgressEvent::Log(format!(
                "install log copied to {}",
                install_log::TARGET_LOG
            )))
            .await;

        if let Some(swap) = &state.swap_partition {
            // Resolving the node can itself fail once things start going away;
            // that is not a reason to fail an install already on disk.
            match ctx.backends.disk.device_node(swap).await {
                Ok(device) => {
                    report(tx, "swapoff", best_effort("swapoff", &[&device]).await).await;
                }
                Err(e) => report(tx, "swapoff", Err(e.to_string())).await,
            }
        }

        report(
            tx,
            "umount",
            best_effort("umount", &["-R", INSTALL_ROOT]).await,
        )
        .await;

        if ctx.config.partitioning.encryption_enabled {
            report(
                tx,
                "cryptsetup close",
                best_effort("cryptsetup", &["close", LUKS_MAPPER_NAME]).await,
            )
            .await;
        }

        let _ = tx
            .send(ProgressEvent::Log("target system released".into()))
            .await;
        Ok(())
    }
}
