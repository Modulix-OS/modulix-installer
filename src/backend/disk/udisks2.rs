use super::{DiskBackend, DiskInfo, FormatFs, PartitionInfo};
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
/// of re-deriving a device node from user input.
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

#[async_trait]
impl DiskBackend for Udisks2Backend {
    async fn list_disks(&self) -> mx::Result<Vec<DiskInfo>> {
        let objects = self.managed_objects().await?;

        let mut disks = Vec::new();
        for (path, ifaces) in &objects {
            if !ifaces
                .keys()
                .any(|i| i.as_str() == "org.freedesktop.UDisks2.PartitionTable")
            {
                continue;
            }
            let Some(drive_value) = prop(ifaces, "org.freedesktop.UDisks2.Block", "Drive") else {
                continue;
            };
            let drive_path: OwnedObjectPath = drive_value.clone().try_into().unwrap_or_default();
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

            let has_windows = objects.iter().any(|(_, part_ifaces)| {
                let same_table = prop(part_ifaces, "org.freedesktop.UDisks2.Partition", "Table")
                    .and_then(|v| v.clone().try_into().ok())
                    .map(|table: OwnedObjectPath| &table == path)
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

            partitions.push(PartitionInfo {
                path: path.as_str().to_string(),
                disk_path: disk.to_string(),
                fs_type,
                label,
                size_bytes,
                start_bytes,
            });
        }
        Ok(partitions)
    }

    async fn create_partition(
        &self,
        disk: &str,
        start_bytes: u64,
        size_bytes: u64,
    ) -> mx::Result<String> {
        let proxy = PartitionTableProxy::builder(&self.connection)
            .path(disk)?
            .build()
            .await?;
        let created = proxy
            .create_partition(start_bytes, size_bytes, "", "", HashMap::new())
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

    async fn format_partition(&self, partition: &str, fs: FormatFs) -> mx::Result<()> {
        let proxy = BlockProxy::builder(&self.connection)
            .path(partition)?
            .build()
            .await?;
        proxy.format(fs.as_mkfs_type(), HashMap::new()).await?;
        Ok(())
    }

    async fn mount(&self, partition: &str, target: &str) -> mx::Result<()> {
        // udisks2's own Filesystem.Mount always picks a path under /run/media;
        // the installer needs an exact target (`/mnt`), so mount(8) directly.
        // Args passed as an array — never through a shell.
        let device = {
            let objects = self.managed_objects().await?;
            let path = OwnedObjectPath::try_from(partition)
                .map_err(|e| mx::Error::Backend(e.to_string()))?;
            objects
                .get(&path)
                .and_then(|ifaces| prop(ifaces, "org.freedesktop.UDisks2.Block", "Device"))
                .and_then(device_path)
                .ok_or_else(|| mx::Error::Backend(format!("unknown partition: {partition}")))?
        };
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
}
