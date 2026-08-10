//! One live-disk-state → [`PlanInput`] fetch, shared by `tasks::PartitionTask`
//! (which executes the result) and the summary page (which only previews
//! it) — both must derive the same input the same way, or the preview could
//! promise something the pipeline doesn't actually do.

use crate::backend::disk::DiskBackend;
use crate::backend::disk::layout;
use crate::config::PartitioningConfig;
use crate::engine::plan::PlanInput;
use crate::engine::sizing::detect_ram_bytes;
use crate::mx;
use std::sync::Arc;

pub fn detect_uefi() -> bool {
    std::path::Path::new("/sys/firmware/efi").exists()
}

pub async fn gather_plan_input(
    disk_backend: &Arc<dyn DiskBackend>,
    disk_path: &str,
    cfg: PartitioningConfig,
) -> mx::Result<PlanInput> {
    let disk = disk_backend
        .list_disks()
        .await?
        .into_iter()
        .find(|d| d.path == disk_path)
        .ok_or_else(|| mx::Error::Backend(format!("unknown disk: {disk_path}")))?;
    let mut partitions = disk_backend.list_partitions(disk_path).await?;
    let gaps = layout::free_gaps(disk.size_bytes, &partitions);
    let ntfs = match partitions
        .iter()
        .find(|p| p.fs_type.as_deref() == Some("ntfs"))
    {
        Some(p) => Some((p.path.clone(), disk_backend.probe_ntfs(&p.path).await?)),
        None => None,
    };
    for p in &mut partitions {
        p.used_bytes = if let Some((ntfs_path, probe)) = &ntfs
            && &p.path == ntfs_path
        {
            probe.used_bytes
        } else {
            disk_backend
                .probe_usage(&p.path, p.fs_type.as_deref())
                .await
                .unwrap_or(None)
        };
    }
    let ram_bytes = detect_ram_bytes().await.unwrap_or(0);

    Ok(PlanInput {
        disk,
        partitions,
        gaps,
        ntfs,
        cfg,
        ram_bytes,
        uefi: detect_uefi(),
    })
}
