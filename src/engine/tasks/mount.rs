use crate::engine::{INSTALL_ROOT, ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;

/// Mounts root, ESP and (if present) `/home` through `DiskBackend::mount` —
/// the half `debugsim::tasks::FakeMountTask` reuses as-is, since it only
/// goes through the backend and the fake one is purely in-memory.
pub(crate) async fn mount_filesystems(ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
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
    if let Some(home) = &state.home_partition {
        let home_dir = format!("{INSTALL_ROOT}/home");
        ctx.backends.disk.mount(home, &home_dir).await?;
        let _ = tx
            .send(ProgressEvent::Log(format!("mounted {home} at {home_dir}")))
            .await;
    }
    Ok(())
}

/// Activates swap with the real `swapon(8)`, unlike every other disk
/// operation in this pipeline: it isn't trait-abstracted, so a debug build
/// running `--fake-disks=` must fake this half itself rather than reuse it.
async fn activate_swap(ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
    let state = ctx.state.lock().await.clone();
    if let Some(swap) = &state.swap_partition {
        let device = ctx.backends.disk.device_node(swap).await?;
        let status = tokio::process::Command::new("swapon")
            .arg(&device)
            .status()
            .await?;
        if !status.success() {
            return Err(mx::Error::Backend(format!(
                "swapon {device} failed: {status}"
            )));
        }
        let _ = tx
            .send(ProgressEvent::Log(format!("activated swap on {device}")))
            .await;
    }
    Ok(())
}

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
        mount_filesystems(ctx, tx).await?;
        activate_swap(ctx, tx).await?;
        Ok(())
    }
}
