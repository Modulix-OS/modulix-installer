use super::ntfs::{self, NtfsBlocker, NtfsProbe};
use super::{DiskBackend, DiskInfo, FormatFs, PartitionInfo, PartitionKind, TableKind};
use crate::mx;
use async_trait::async_trait;
use std::collections::HashMap;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

const SERVICE: &str = "org.freedesktop.UDisks2";
const MANAGER_PATH: &str = "/org/freedesktop/UDisks2";

#[zbus::proxy(
    interface = "org.freedesktop.UDisks2.PartitionTable",
    default_service = "org.freedesktop.UDisks2"
)]
trait PartitionTable {
    #[zbus(name = "CreatePartition")]
    fn create_partition(
        &self,
        offset: u64,
        size: u64,
        type_: &str,
        name: &str,
        options: HashMap<&str, zbus::zvariant::Value<'_>>,
    ) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.freedesktop.UDisks2.Partition",
    default_service = "org.freedesktop.UDisks2"
)]
trait Partition {
    #[zbus(name = "Delete")]
    fn delete(&self, options: HashMap<&str, zbus::zvariant::Value<'_>>) -> zbus::Result<()>;

    #[zbus(name = "Resize")]
    fn resize(
        &self,
        size: u64,
        options: HashMap<&str, zbus::zvariant::Value<'_>>,
    ) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.UDisks2.Block",
    default_service = "org.freedesktop.UDisks2"
)]
trait Block {
    /// Also used to (re)create a partition table itself: `type_` of `"dos"`,
    /// `"gpt"`, or `"empty"` wipes and (re)initializes the table instead of
    /// formatting a filesystem — that's the documented UDisks2 behavior.
    #[zbus(name = "Format")]
    fn format(
        &self,
        type_: &str,
        options: HashMap<&str, zbus::zvariant::Value<'_>>,
    ) -> zbus::Result<()>;
}

/// udisks2-backed disk enumeration/manipulation. `DiskInfo::path` /
/// `PartitionInfo::path` are udisks2 **object paths** (not `/dev/...`), so
/// every write operation can go straight back through a typed proxy instead
/// of re-deriving a device node from user input; [`Udisks2Backend::device_node`]
/// is the one sanctioned conversion to `/dev/...`, needed for tools that
/// don't speak D-Bus (`cryptsetup`, `mkswap`, `swapon`, `ntfsresize`).
pub struct Udisks2Backend {
    connection: zbus::Connection,
}

impl Udisks2Backend {
    pub async fn connect() -> mx::Result<Self> {
        let connection = zbus::Connection::system().await?;
        Ok(Self { connection })
    }

    async fn managed_objects(&self) -> mx::Result<zbus::fdo::ManagedObjects> {
        let manager = zbus::fdo::ObjectManagerProxy::builder(&self.connection)
            .destination(SERVICE)?
            .path(MANAGER_PATH)?
            .build()
            .await?;
        Ok(manager.get_managed_objects().await?)
    }
}

fn prop<'a>(
    props: &'a HashMap<zbus::names::OwnedInterfaceName, HashMap<String, OwnedValue>>,
    interface: &str,
    key: &str,
) -> Option<&'a OwnedValue> {
    props
        .iter()
        .find(|(name, _)| name.as_str() == interface)
        .and_then(|(_, values)| values.get(key))
}

fn device_path(bytes: &OwnedValue) -> Option<String> {
    let raw: Vec<u8> = bytes.clone().try_into().ok()?;
    let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
    String::from_utf8(raw[..end].to_vec()).ok()
}

/// Disk object paths backing the live medium — `/` (the ISO's own root) or
/// `/iso` (a common squashfs/live-boot mount point) — never a valid install
/// target — refuse the disk carrying the live medium.
/// Best-effort: a `/proc/mounts` parse failure just yields an empty set
/// rather than an error, since this is a defense-in-depth filter, not the
/// primary safety mechanism (the destructive-confirmation dialog is).
async fn live_medium_disks(
    objects: &zbus::fdo::ManagedObjects,
) -> std::collections::HashSet<String> {
    let mounts = tokio::fs::read_to_string("/proc/mounts")
        .await
        .unwrap_or_default();
    let live_devices: Vec<String> = mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let device = fields.next()?;
            let mount_point = fields.next()?;
            (mount_point == "/" || mount_point == "/iso").then(|| device.to_string())
        })
        .collect();
    if live_devices.is_empty() {
        return std::collections::HashSet::new();
    }

    let mut excluded = std::collections::HashSet::new();
    for (path, ifaces) in objects {
        let Some(device_value) = prop(ifaces, "org.freedesktop.UDisks2.Block", "Device") else {
            continue;
        };
        let Some(udisks_device) = device_path(device_value) else {
            continue;
        };
        if !live_devices.iter().any(|d| d == &udisks_device) {
            continue;
        }
        let disk_path = prop(ifaces, "org.freedesktop.UDisks2.Partition", "Table")
            .and_then(|v| v.clone().try_into().ok())
            .map(|t: OwnedObjectPath| t.as_str().to_string())
            .unwrap_or_else(|| path.as_str().to_string());
        eprintln!(
            "excluding live-medium disk {disk_path} ({udisks_device} backs / or /iso) from the target-disk list"
        );
        excluded.insert(disk_path);
    }
    excluded
}

#[async_trait]
impl DiskBackend for Udisks2Backend {
    async fn list_disks(&self) -> mx::Result<Vec<DiskInfo>> {
        let objects = self.managed_objects().await?;
        let excluded = live_medium_disks(&objects).await;

        let mut disks = Vec::new();
        for (path, ifaces) in &objects {
            if excluded.contains(path.as_str()) {
                continue;
            }
            // The whole-disk Block object: has `Drive` set, but unlike a
            // partition's Block object, no `Partition` interface. A blank
            // disk still has this object — it just lacks `PartitionTable`
            // (previously required by this filter, which made vendor-fresh
            // drives invisible — see the partitioning plan, step 7).
            if ifaces
                .keys()
                .any(|i| i.as_str() == "org.freedesktop.UDisks2.Partition")
            {
                continue;
            }
            let Some(drive_value) = prop(ifaces, "org.freedesktop.UDisks2.Block", "Drive") else {
                continue;
            };
            let drive_path: OwnedObjectPath = drive_value.clone().try_into().unwrap_or_default();
            if drive_path.as_str() == "/" {
                continue;
            }
            let drive_ifaces = objects.get(&drive_path);

            let model = drive_ifaces
                .and_then(|d| prop(d, "org.freedesktop.UDisks2.Drive", "Model"))
                .and_then(|v| v.clone().try_into().ok())
                .unwrap_or_else(|| "Unknown disk".to_string());
            let size_bytes = drive_ifaces
                .and_then(|d| prop(d, "org.freedesktop.UDisks2.Drive", "Size"))
                .and_then(|v| v.clone().try_into().ok())
                .unwrap_or(0u64);
            let is_removable = drive_ifaces
                .and_then(|d| prop(d, "org.freedesktop.UDisks2.Drive", "Removable"))
                .and_then(|v| v.clone().try_into().ok())
                .unwrap_or(false);

            let table_type: Option<String> =
                prop(ifaces, "org.freedesktop.UDisks2.PartitionTable", "Type")
                    .and_then(|v| v.clone().try_into().ok());
            let table = match table_type.as_deref() {
                Some("gpt") => TableKind::Gpt,
                Some("dos") => TableKind::Dos,
                _ => TableKind::None,
            };

            let has_windows = objects.iter().any(|(_, part_ifaces)| {
                let same_table = prop(part_ifaces, "org.freedesktop.UDisks2.Partition", "Table")
                    .and_then(|v| v.clone().try_into().ok())
                    .map(|t: OwnedObjectPath| &t == path)
                    .unwrap_or(false);
                same_table
                    && prop(part_ifaces, "org.freedesktop.UDisks2.Block", "IdType")
                        .and_then(|v| v.clone().try_into().ok())
                        .map(|id_type: String| id_type == "ntfs")
                        .unwrap_or(false)
            });

            disks.push(DiskInfo {
                path: path.as_str().to_string(),
                model,
                size_bytes,
                is_removable,
                has_windows,
                table,
            });
        }
        Ok(disks)
    }

    async fn list_partitions(&self, disk: &str) -> mx::Result<Vec<PartitionInfo>> {
        let objects = self.managed_objects().await?;
        let disk_path =
            ObjectPath::try_from(disk).map_err(|e| mx::Error::Backend(e.to_string()))?;

        let mut partitions = Vec::new();
        for (path, ifaces) in &objects {
            let Some(table) = prop(ifaces, "org.freedesktop.UDisks2.Partition", "Table") else {
                continue;
            };
            let table: OwnedObjectPath = match table.clone().try_into() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if table.as_str() != disk_path.as_str() {
                continue;
            }

            let start_bytes = prop(ifaces, "org.freedesktop.UDisks2.Partition", "Offset")
                .and_then(|v| v.clone().try_into().ok())
                .unwrap_or(0u64);
            let size_bytes = prop(ifaces, "org.freedesktop.UDisks2.Partition", "Size")
                .and_then(|v| v.clone().try_into().ok())
                .unwrap_or(0u64);
            let fs_type = prop(ifaces, "org.freedesktop.UDisks2.Block", "IdType")
                .and_then(|v| v.clone().try_into().ok())
                .filter(|s: &String| !s.is_empty());
            let label = prop(ifaces, "org.freedesktop.UDisks2.Block", "IdLabel")
                .and_then(|v| v.clone().try_into().ok())
                .filter(|s: &String| !s.is_empty());
            let type_guid: Option<String> =
                prop(ifaces, "org.freedesktop.UDisks2.Partition", "Type")
                    .and_then(|v| v.clone().try_into().ok())
                    .filter(|s: &String| !s.is_empty());
            let uuid = prop(ifaces, "org.freedesktop.UDisks2.Block", "IdUUID")
                .and_then(|v| v.clone().try_into().ok())
                .filter(|s: &String| !s.is_empty());
            let is_esp = type_guid
                .as_deref()
                .map(|g| g.eq_ignore_ascii_case(PartitionKind::Esp.type_guid()))
                .unwrap_or(false);

            partitions.push(PartitionInfo {
                path: path.as_str().to_string(),
                disk_path: disk.to_string(),
                fs_type,
                label,
                size_bytes,
                start_bytes,
                type_guid,
                uuid,
                is_esp,
                used_bytes: None,
            });
        }
        Ok(partitions)
    }

    async fn device_node(&self, path: &str) -> mx::Result<String> {
        let objects = self.managed_objects().await?;
        let object_path =
            OwnedObjectPath::try_from(path).map_err(|e| mx::Error::Backend(e.to_string()))?;
        objects
            .get(&object_path)
            .and_then(|ifaces| prop(ifaces, "org.freedesktop.UDisks2.Block", "Device"))
            .and_then(device_path)
            .ok_or_else(|| mx::Error::Backend(format!("unknown udisks2 object: {path}")))
    }

    async fn resolve_device(&self, device_node: &str) -> mx::Result<String> {
        let canonical = tokio::fs::canonicalize(device_node).await.ok();
        const ATTEMPTS: u32 = 10;
        for attempt in 0..ATTEMPTS {
            let objects = self.managed_objects().await?;
            for (path, ifaces) in &objects {
                let Some(dev_prop) = prop(ifaces, "org.freedesktop.UDisks2.Block", "Device") else {
                    continue;
                };
                let Some(udisks_device) = device_path(dev_prop) else {
                    continue;
                };
                let matches = udisks_device == device_node
                    || canonical
                        .as_deref()
                        .map(|c| c.to_string_lossy() == udisks_device)
                        .unwrap_or(false);
                if matches {
                    return Ok(path.as_str().to_string());
                }
            }
            if attempt + 1 < ATTEMPTS {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
        Err(mx::Error::Backend(format!(
            "udisks2 never noticed new device {device_node}"
        )))
    }

    async fn create_table(&self, disk: &str, table: TableKind) -> mx::Result<()> {
        let type_ = match table {
            TableKind::Gpt => "gpt",
            TableKind::Dos => "dos",
            TableKind::None => "empty",
        };
        let proxy = BlockProxy::builder(&self.connection)
            .path(disk)?
            .build()
            .await?;
        proxy.format(type_, HashMap::new()).await?;
        Ok(())
    }

    async fn create_partition(
        &self,
        disk: &str,
        start_bytes: u64,
        size_bytes: u64,
        kind: PartitionKind,
    ) -> mx::Result<String> {
        let proxy = PartitionTableProxy::builder(&self.connection)
            .path(disk)?
            .build()
            .await?;
        let created = proxy
            .create_partition(
                start_bytes,
                size_bytes,
                kind.type_guid(),
                "",
                HashMap::new(),
            )
            .await?;
        Ok(created.as_str().to_string())
    }

    async fn delete_partition(&self, partition: &str) -> mx::Result<()> {
        let proxy = PartitionProxy::builder(&self.connection)
            .path(partition)?
            .build()
            .await?;
        proxy.delete(HashMap::new()).await?;
        Ok(())
    }

    async fn resize_partition(&self, partition: &str, new_size_bytes: u64) -> mx::Result<()> {
        let proxy = PartitionProxy::builder(&self.connection)
            .path(partition)?
            .build()
            .await?;
        proxy.resize(new_size_bytes, HashMap::new()).await?;
        Ok(())
    }

    async fn format_partition(
        &self,
        partition: &str,
        fs: FormatFs,
        label: Option<&str>,
    ) -> mx::Result<()> {
        let proxy = BlockProxy::builder(&self.connection)
            .path(partition)?
            .build()
            .await?;
        let mut options = HashMap::new();
        if let Some(label) = label {
            options.insert("label", zbus::zvariant::Value::from(label));
        }
        proxy.format(fs.as_mkfs_type(), options).await?;
        Ok(())
    }

    async fn probe_ntfs(&self, partition: &str) -> mx::Result<NtfsProbe> {
        let id_type = {
            let objects = self.managed_objects().await?;
            let object_path = OwnedObjectPath::try_from(partition)
                .map_err(|e| mx::Error::Backend(e.to_string()))?;
            objects
                .get(&object_path)
                .and_then(|ifaces| prop(ifaces, "org.freedesktop.UDisks2.Block", "IdType"))
                .and_then(|v| v.clone().try_into().ok())
                .unwrap_or_default()
        };
        let id_type: String = id_type;
        if id_type.eq_ignore_ascii_case("BitLocker") {
            return Ok(NtfsProbe {
                min_size_bytes: 0,
                current_size_bytes: 0,
                blockers: vec![NtfsBlocker::BitLocker],
                used_bytes: None,
            });
        }

        let device = self.device_node(partition).await?;
        let output = tokio::process::Command::new("ntfsresize")
            .args(["--info", "--no-action", "--force", &device])
            .output()
            .await?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        Ok(ntfs::parse_ntfsresize_info(
            &stdout,
            &stderr,
            output.status.success(),
        ))
    }

    async fn probe_usage(&self, path: &str, fs_type: Option<&str>) -> mx::Result<Option<u64>> {
        // NTFS is deliberately not probed here — `ntfsresize --info` already
        // runs once per disk load (`probe_ntfs`) and does a full cluster
        // accounting pass, seconds on a large volume; its `Space in use`
        // line (`ntfs::NtfsProbe::used_bytes`) is reused instead of paying
        // for a second pass.
        if !matches!(fs_type, Some("ext2") | Some("ext3") | Some("ext4")) {
            return Ok(None);
        }
        let device = self.device_node(path).await?;
        let output = tokio::process::Command::new("dumpe2fs")
            .args(["-h", &device])
            .output()
            .await?;
        if !output.status.success() {
            return Ok(None);
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(super::usage::parse_dumpe2fs_usage(&stdout))
    }

    async fn mount(&self, partition: &str, target: &str) -> mx::Result<()> {
        // udisks2's own Filesystem.Mount always picks a path under /run/media;
        // the installer needs an exact target (`/mnt`), so mount(8) directly.
        // Args passed as an array — never through a shell.
        let device = self.device_node(partition).await?;
        tokio::fs::create_dir_all(target).await?;
        let status = tokio::process::Command::new("mount")
            .arg(&device)
            .arg(target)
            .status()
            .await?;
        if !status.success() {
            return Err(mx::Error::Backend(format!(
                "mount {device} {target} failed: {status}"
            )));
        }
        Ok(())
    }

    async fn unmount(&self, partition: &str) -> mx::Result<()> {
        let device = self.device_node(partition).await?;
        let status = tokio::process::Command::new("umount")
            .arg(&device)
            .status()
            .await?;
        if !status.success() {
            return Err(mx::Error::Backend(format!(
                "umount {device} failed: {status}"
            )));
        }
        Ok(())
    }
}
