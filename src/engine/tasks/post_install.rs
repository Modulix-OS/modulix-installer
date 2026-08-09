use crate::engine::{ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;

/// Stub: cleanup/unmount/reboot-prompt hooks land alongside the real
/// `InitConfigTask` in iteration 2.
pub struct PostInstallTask;

#[async_trait]
impl Task for PostInstallTask {
    fn label(&self) -> String {
        "Finishing up".to_string()
    }

    fn weight(&self) -> u32 {
        1
    }

    async fn run(&self, _ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        let _ = tx
            .send(ProgressEvent::Log(
                "post-install steps not implemented yet".into(),
            ))
            .await;
        Ok(())
    }
}
