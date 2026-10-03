use crate::engine::PipelineState;
use crate::engine::live_input::gather_plan_input;
use crate::engine::plan::{self, PartitionRole, PlanOp};
use crate::engine::{ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;

fn assign_role(
    state: &mut PipelineState,
    role: PartitionRole,
    path: String,
    format: Option<crate::backend::disk::FormatFs>,
) {
    match role {
        PartitionRole::Esp => {
            state.efi_partition = Some(path);
            state.esp_is_new = format.is_some();
        }
        PartitionRole::Root => {
            state.root_partition = Some(path);
            state.root_format = format;
        }
        PartitionRole::Swap => {
            state.swap_partition = Some(path);
            state.swap_format = format;
        }
        PartitionRole::Home => {
            state.home_partition = Some(path);
            state.home_format = format;
        }
    }
}

/// Executes `engine::plan::plan`'s output op by op — this task computes
/// nothing itself, so the plan the UI showed as a preview (`steps::partitioning`)
/// and the plan actually written to disk can never diverge.
pub struct PartitionTask;

#[async_trait]
impl Task for PartitionTask {
    fn label(&self) -> String {
        "Partitioning disk".to_string()
    }

    fn weight(&self) -> u32 {
        10
    }

    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        let disk_path = ctx
            .config
            .partitioning
            .target_disk
            .clone()
            .ok_or_else(|| mx::Error::Backend("no target disk selected".into()))?;

        // Best-effort: the disk must not have any of its own partitions
        // mounted before we rewrite its partition table.
        for p in ctx.backends.disk.list_partitions(&disk_path).await? {
            let _ = ctx.backends.disk.unmount(&p.path).await;
        }

        let input = gather_plan_input(
            &ctx.backends.disk,
            &disk_path,
            ctx.config.partitioning.clone(),
            ctx.uefi,
        )
        .await?;
        let result = plan::plan(&input).map_err(|e| mx::Error::Backend(e.msgid()))?;

        let mut state = ctx.state.lock().await;
        for op in &result.ops {
            match op {
                PlanOp::CreateTable { disk, table } => {
                    ctx.backends.disk.create_table(disk, *table).await?;
                    let _ = tx
                        .send(ProgressEvent::Log(format!(
                            "created a {table:?} partition table on {disk}"
                        )))
                        .await;
                }
                PlanOp::ShrinkNtfs {
                    path,
                    new_size_bytes,
                } => {
                    ctx.backends
                        .disk
                        .resize_partition(path, *new_size_bytes)
                        .await?;
                    let _ = tx
                        .send(ProgressEvent::Log(format!(
                            "shrank Windows partition {path} to {new_size_bytes} bytes"
                        )))
                        .await;
                }
                PlanOp::Create {
                    role,
                    start_bytes,
                    size_bytes,
                    kind,
                    fs,
                } => {
                    let path = ctx
                        .backends
                        .disk
                        .create_partition(&disk_path, *start_bytes, *size_bytes, *kind)
                        .await?;
                    let _ = tx
                        .send(ProgressEvent::Log(format!("created {path} ({role:?})")))
                        .await;
                    assign_role(&mut state, *role, path, Some(*fs));
                }
                PlanOp::UseExisting { path, role, format } => {
                    let _ = tx
                        .send(ProgressEvent::Log(format!("reusing {path} as {role:?}")))
                        .await;
                    assign_role(&mut state, *role, path.clone(), *format);
                }
            }
        }
        Ok(())
    }
}
