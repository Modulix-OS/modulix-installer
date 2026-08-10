use crate::engine::{ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;

/// Stub: `modulix_core_utils::init::init_all` doesn't exist yet — it's an
/// iteration-2 change to `modulix-core-utils`. This task is scaffolded now
/// so wiring the real call in is a one-line change later.
pub struct InitConfigTask;

#[async_trait]
impl Task for InitConfigTask {
    fn label(&self) -> String {
        "Writing NixOS configuration".to_string()
    }

    fn weight(&self) -> u32 {
        20
    }

    async fn run(&self, _ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        let _ = tx
            .send(ProgressEvent::Log(
                "init_all not implemented yet (modulix-core-utils, iteration 2) — skipping".into(),
            ))
            .await;
        Ok(())
    }
}
