use crate::engine::{ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;

/// modulix-core-utils' `rebuild_config` hardcodes `--root /mnt` (see
/// CLAUDE.md), so the target must be mounted here, not a configurable path.
const INSTALL_ROOT: &str = "/mnt";

pub struct MountTask;

#[async_trait]
impl Task for MountTask {
    fn label(&self) -> String {
        "Mounting target filesystems".to_string()
    }

    fn weight(&self) -> u32 {
        2
    }

    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        let state = ctx.state.lock().await.clone();

        if let Some(root) = &state.root_partition {
            ctx.backends.disk.mount(root, INSTALL_ROOT).await?;
            let _ = tx
                .send(ProgressEvent::Log(format!(
                    "mounted {root} at {INSTALL_ROOT}"
                )))
                .await;
        }
        if let Some(efi) = &state.efi_partition {
            let boot_dir = format!("{INSTALL_ROOT}/boot");
            ctx.backends.disk.mount(efi, &boot_dir).await?;
            let _ = tx
                .send(ProgressEvent::Log(format!("mounted {efi} at {boot_dir}")))
                .await;
        }
        Ok(())
    }
}
