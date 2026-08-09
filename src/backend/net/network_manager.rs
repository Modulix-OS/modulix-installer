use super::{Connectivity, NetworkBackend, WifiAccessPoint};
use crate::mx;
use async_trait::async_trait;
use std::collections::HashMap;
use std::time::Duration;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};

const SERVICE: &str = "org.freedesktop.NetworkManager";
const MANAGER_PATH: &str = "/org/freedesktop/NetworkManager";

const DEVICE_TYPE_ETHERNET: u32 = 1;
const DEVICE_TYPE_WIFI: u32 = 2;
const DEVICE_STATE_ACTIVATED: u32 = 100;

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager"
)]
trait Manager {
    #[zbus(name = "GetDevices")]
    fn get_devices(&self) -> zbus::Result<Vec<OwnedObjectPath>>;

    #[zbus(name = "AddAndActivateConnection")]
    fn add_and_activate_connection(
        &self,
        connection: HashMap<&str, HashMap<&str, Value<'_>>>,
        device: &ObjectPath<'_>,
        specific_object: &ObjectPath<'_>,
    ) -> zbus::Result<(OwnedObjectPath, OwnedObjectPath)>;

    #[zbus(property)]
    fn connectivity(&self) -> zbus::Result<u32>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Device",
    default_service = "org.freedesktop.NetworkManager"
)]
trait Device {
    #[zbus(property, name = "DeviceType")]
    fn device_type(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Device.Wireless",
    default_service = "org.freedesktop.NetworkManager"
)]
trait Wireless {
    #[zbus(name = "RequestScan")]
    fn request_scan(&self, options: HashMap<&str, Value<'_>>) -> zbus::Result<()>;

    #[zbus(name = "GetAllAccessPoints")]
    fn get_all_access_points(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.AccessPoint",
    default_service = "org.freedesktop.NetworkManager"
)]
trait AccessPoint {
    #[zbus(property)]
    fn ssid(&self) -> zbus::Result<Vec<u8>>;

    #[zbus(property)]
    fn strength(&self) -> zbus::Result<u8>;

    #[zbus(property)]
    fn wpa_flags(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn rsn_flags(&self) -> zbus::Result<u32>;
}

pub struct NetworkManagerBackend {
    connection: zbus::Connection,
}

impl NetworkManagerBackend {
    pub async fn connect() -> mx::Result<Self> {
        let connection = zbus::Connection::system().await?;
        Ok(Self { connection })
    }

    async fn manager(&self) -> mx::Result<ManagerProxy<'_>> {
        Ok(ManagerProxy::new(&self.connection).await?)
    }

    async fn devices_of_type(&self, device_type: u32) -> mx::Result<Vec<OwnedObjectPath>> {
        let manager = self.manager().await?;
        let mut matching = Vec::new();
        for path in manager.get_devices().await? {
            let device = DeviceProxy::builder(&self.connection)
                .path(&path)?
                .build()
                .await?;
            if device.device_type().await? == device_type {
                matching.push(path);
            }
        }
        Ok(matching)
    }

    async fn first_wifi_device(&self) -> mx::Result<OwnedObjectPath> {
        self.devices_of_type(DEVICE_TYPE_WIFI)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| mx::Error::Backend("no Wi-Fi device found".to_string()))
    }
}

#[async_trait]
impl NetworkBackend for NetworkManagerBackend {
    async fn connectivity(&self) -> mx::Result<Connectivity> {
        let manager = self.manager().await?;
        Ok(match manager.connectivity().await? {
            1 => Connectivity::None,
            2 => Connectivity::Portal,
            3 => Connectivity::Limited,
            4 => Connectivity::Full,
            _ => Connectivity::Unknown,
        })
    }

    async fn is_ethernet_connected(&self) -> mx::Result<bool> {
        for path in self.devices_of_type(DEVICE_TYPE_ETHERNET).await? {
            let device = DeviceProxy::builder(&self.connection)
                .path(&path)?
                .build()
                .await?;
            if device.state().await? == DEVICE_STATE_ACTIVATED {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn scan_wifi(&self) -> mx::Result<Vec<WifiAccessPoint>> {
        let device_path = self.first_wifi_device().await?;
        let wireless = WirelessProxy::builder(&self.connection)
            .path(&device_path)?
            .build()
            .await?;

        let _ = wireless.request_scan(HashMap::new()).await;
        tokio::time::sleep(Duration::from_secs(3)).await;

        let mut access_points = Vec::new();
        for ap_path in wireless.get_all_access_points().await? {
            let ap = AccessPointProxy::builder(&self.connection)
                .path(&ap_path)?
                .build()
                .await?;
            let ssid_bytes = ap.ssid().await.unwrap_or_default();
            let Ok(ssid) = String::from_utf8(ssid_bytes) else {
                continue;
            };
            if ssid.is_empty() {
                continue;
            }
            let secured =
                ap.wpa_flags().await.unwrap_or(0) != 0 || ap.rsn_flags().await.unwrap_or(0) != 0;
            access_points.push(WifiAccessPoint {
                ssid,
                strength: ap.strength().await.unwrap_or(0),
                secured,
            });
        }
        Ok(access_points)
    }

    async fn connect_wifi(&self, ssid: &str, password: Option<&str>) -> mx::Result<()> {
        let device_path = self.first_wifi_device().await?;
        let manager = self.manager().await?;

        let mut wireless_settings: HashMap<&str, Value<'_>> = HashMap::new();
        wireless_settings.insert("ssid", Value::from(ssid.as_bytes().to_vec()));

        let mut connection: HashMap<&str, HashMap<&str, Value<'_>>> = HashMap::new();
        connection.insert("802-11-wireless", wireless_settings);

        if let Some(password) = password {
            let mut security: HashMap<&str, Value<'_>> = HashMap::new();
            security.insert("key-mgmt", Value::from("wpa-psk"));
            security.insert("psk", Value::from(password));
            connection.insert("802-11-wireless-security", security);
        }

        let no_specific_object = ObjectPath::try_from("/").expect("well-formed root object path");
        manager
            .add_and_activate_connection(connection, &device_path.as_ref(), &no_specific_object)
            .await?;
        Ok(())
    }
}
