use super::{
    Connectivity, NetworkBackend, PortalPage, WifiAccessPoint, WifiConnectRequest, WifiSecurity,
    dedup_access_points,
};
use crate::mx;
use async_trait::async_trait;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::time::Duration;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};

const DEVICE_TYPE_ETHERNET: u32 = 1;
const DEVICE_TYPE_WIFI: u32 = 2;
const DEVICE_STATE_ACTIVATED: u32 = 100;

const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(45);
const SCAN_POLL_INTERVAL: Duration = Duration::from_millis(500);
const SCAN_TIMEOUT: Duration = Duration::from_secs(10);

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

    #[zbus(name = "CheckConnectivity")]
    fn check_connectivity(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn connectivity(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn connectivity_check_uri(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn connectivity_check_enabled(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn connectivity_check_available(&self) -> zbus::Result<bool>;
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

    #[zbus(property)]
    fn last_scan(&self) -> zbus::Result<i64>;

    #[zbus(property)]
    fn active_access_point(&self) -> zbus::Result<OwnedObjectPath>;
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
    fn flags(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn wpa_flags(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn rsn_flags(&self) -> zbus::Result<u32>;
}

// NOTE: deliberately no `Device.StateChanged` proxy in this file — zbus
// generates module-scope types (`StateChangedStream`/`StateChangedArgs`) from
// the D-Bus signal name, and `ActiveConnection` below already claims that
// name for a different interface. `Connection.Active.StateChanged` already
// carries the activation `reason`, so there's nothing Device's own signal
// would add here.
#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Connection.Active",
    default_service = "org.freedesktop.NetworkManager"
)]
trait ActiveConnection {
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;

    /// Renamed on the Rust side (wire name stays `StateChanged`) so the
    /// generated `receive_activation_state_changed()` doesn't collide with
    /// `receive_state_changed()`, which zbus already generates for the
    /// `State` property above.
    #[zbus(signal, name = "StateChanged")]
    fn activation_state_changed(&self, state: u32, reason: u32) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Settings.Connection",
    default_service = "org.freedesktop.NetworkManager"
)]
trait SettingsConnection {
    #[zbus(name = "Delete")]
    fn delete(&self) -> zbus::Result<()>;
}

pub(crate) fn connectivity_from_nm(raw: u32) -> Connectivity {
    match raw {
        1 => Connectivity::None,
        2 => Connectivity::Portal,
        3 => Connectivity::Limited,
        4 => Connectivity::Full,
        _ => Connectivity::Unknown,
    }
}

/// Fallback used when NM's connectivity checking itself is unavailable/disabled
/// (see the `Unknown` guardrail on [`NetworkManagerBackend::connect`]).
/// `NMState`: `CONNECTED_GLOBAL` (70) => `Full`; `CONNECTED_LOCAL`/`_SITE`
/// (50/60) => `Limited`; anything else (asleep, disconnected, connecting…) => `None`.
pub(crate) fn connectivity_from_nm_state(state: u32) -> Connectivity {
    match state {
        70 => Connectivity::Full,
        50 | 60 => Connectivity::Limited,
        _ => Connectivity::None,
    }
}

/// Order matters: WPA3-transition APs
/// advertise both `KEY_MGMT_PSK` and `KEY_MGMT_SAE`, so SAE is only chosen
/// when PSK is absent (same rule NM's own applet uses), or drivers without
/// SAE support fail association with a correct password.
pub(crate) fn security_from_flags(flags: u32, wpa: u32, rsn: u32) -> WifiSecurity {
    let combined = wpa | rsn;
    if combined & 0x200 != 0 {
        WifiSecurity::Enterprise
    } else if rsn & 0x400 != 0 && combined & 0x100 == 0 {
        WifiSecurity::Sae
    } else if combined & 0x100 != 0 {
        WifiSecurity::Psk
    } else if rsn & 0x800 != 0 {
        WifiSecurity::Open
    } else if flags & 0x1 != 0 {
        WifiSecurity::Psk
    } else {
        WifiSecurity::Open
    }
}

pub(crate) fn key_mgmt_for(security: WifiSecurity) -> Option<&'static str> {
    match security {
        WifiSecurity::Psk => Some("wpa-psk"),
        WifiSecurity::Sae => Some("sae"),
        WifiSecurity::Open | WifiSecurity::Enterprise => None,
    }
}

/// `NM_ACTIVE_CONNECTION_STATE_REASON_*`.
pub(crate) fn activation_error_msgid(reason: u32) -> &'static str {
    match reason {
        9 => "Incorrect Wi-Fi password",
        10 => "Wi-Fi authentication failed",
        6 => "Timed out while connecting to the Wi-Fi network",
        5 => "Couldn't get an IP address from this network",
        3 => "The Wi-Fi device was disconnected",
        _ => "Couldn't connect to this Wi-Fi network",
    }
}

fn wireless_settings(request: &WifiConnectRequest) -> HashMap<&str, HashMap<&str, Value<'_>>> {
    let mut wireless: HashMap<&str, Value<'_>> = HashMap::new();
    wireless.insert("ssid", Value::from(request.ssid.as_bytes().to_vec()));
    if request.hidden {
        wireless.insert("hidden", Value::from(true));
    }

    let mut connection: HashMap<&str, HashMap<&str, Value<'_>>> = HashMap::new();
    connection.insert("802-11-wireless", wireless);

    if let Some(key_mgmt) = key_mgmt_for(request.security) {
        let mut security: HashMap<&str, Value<'_>> = HashMap::new();
        security.insert("key-mgmt", Value::from(key_mgmt));
        if let Some(password) = &request.password {
            security.insert("psk", Value::from(password.as_str()));
        }
        connection.insert("802-11-wireless-security", security);
    }

    connection
}

pub struct NetworkManagerBackend {
    connection: zbus::Connection,
    /// Cached once at [`Self::connect`]: whether NM's own connectivity
    /// checking is actually functional here. If it isn't, `Connectivity`
    /// would read `Unknown` forever — see the guardrail note below.
    connectivity_checking_enabled: bool,
}

impl NetworkManagerBackend {
    pub async fn connect() -> mx::Result<Self> {
        let connection = zbus::Connection::system().await?;
        let manager = ManagerProxy::new(&connection).await?;
        // NM reports `Connectivity = UNKNOWN` permanently when it was built
        // without connectivity checking, or when `[connectivity] uri=` is
        // absent from NetworkManager.conf (`ConnectivityCheckEnabled ==
        // false`). The step's gate blocks on `Unknown`, so without this
        // fallback a machine with perfectly working Internet would be stuck.
        let available = manager
            .connectivity_check_available()
            .await
            .unwrap_or(false);
        let enabled = manager.connectivity_check_enabled().await.unwrap_or(false);
        Ok(Self {
            connection,
            connectivity_checking_enabled: available && enabled,
        })
    }

    async fn manager(&self) -> mx::Result<ManagerProxy<'_>> {
        Ok(ManagerProxy::new(&self.connection).await?)
    }

    async fn current_connectivity(&self) -> mx::Result<Connectivity> {
        let manager = self.manager().await?;
        if self.connectivity_checking_enabled {
            Ok(connectivity_from_nm(manager.connectivity().await?))
        } else {
            Ok(connectivity_from_nm_state(manager.state().await?))
        }
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
            .ok_or_else(|| mx::Error::Backend("No Wi-Fi device found".to_string()))
    }

    /// `None` when the SSID isn't in NM's current scan cache — the caller
    /// then activates with `specific_object = "/"`, letting NM match it itself.
    async fn ap_path_for_ssid(
        &self,
        device_path: &OwnedObjectPath,
        ssid: &str,
    ) -> mx::Result<Option<OwnedObjectPath>> {
        let wireless = WirelessProxy::builder(&self.connection)
            .path(device_path)?
            .build()
            .await?;
        for ap_path in wireless.get_all_access_points().await? {
            let ap = AccessPointProxy::builder(&self.connection)
                .path(&ap_path)?
                .build()
                .await?;
            let ssid_bytes = ap.ssid().await.unwrap_or_default();
            if String::from_utf8(ssid_bytes).as_deref() == Ok(ssid) {
                return Ok(Some(ap_path));
            }
        }
        Ok(None)
    }

    /// `AddAndActivateConnection` persists the profile to
    /// `/etc/NetworkManager/system-connections/` with `autoconnect = true`.
    /// Left behind after a failed attempt, NM retries a bad PSK forever for
    /// the rest of the live session, stealing the radio from anything that
    /// worked; repeated attempts each add another duplicate profile; and a
    /// string of failed 4-way handshakes can get this MAC blacklisted by some
    /// APs. `nmcli device wifi connect` does the same cleanup on failure.
    async fn discard_profile(&self, profile_path: &OwnedObjectPath) {
        let Ok(builder) = SettingsConnectionProxy::builder(&self.connection).path(profile_path)
        else {
            return;
        };
        if let Ok(settings) = builder.build().await {
            let _ = settings.delete().await;
        }
    }

    async fn await_activation(&self, active_path: &OwnedObjectPath) -> mx::Result<()> {
        let active = ActiveConnectionProxy::builder(&self.connection)
            .path(active_path)?
            .build()
            .await?;
        // Subscribe before reading the current state: a transition landing in
        // between is still delivered through the signal stream.
        let mut states = active.receive_activation_state_changed().await?;
        if let Ok(state) = active.state().await {
            match state {
                2 => return Ok(()),
                4 => return Err(mx::Error::Backend(activation_error_msgid(0).to_string())),
                _ => {}
            }
        }
        // The signal (not the property stream) is used deliberately: the
        // property only carries the new state, not the failure `reason` —
        // and "wrong password" *is* a reason. NM also destroys the
        // `ActiveConnection` object on failure, so property reads made after
        // that point start failing even though the signal already fired.
        while let Some(signal) = states.next().await {
            let args = signal.args()?;
            match args.state {
                2 => return Ok(()),
                4 => {
                    return Err(mx::Error::Backend(
                        activation_error_msgid(args.reason).to_string(),
                    ));
                }
                _ => {}
            }
        }
        Err(mx::Error::Backend(
            "The Wi-Fi connection was dropped".to_string(),
        ))
    }
}

#[async_trait]
impl NetworkBackend for NetworkManagerBackend {
    async fn connectivity(&self) -> mx::Result<Connectivity> {
        self.current_connectivity().await
    }

    async fn watch_connectivity(&self) -> mx::Result<async_channel::Receiver<Connectivity>> {
        let (tx, rx) = async_channel::unbounded();
        let current = self.current_connectivity().await?;
        let _ = tx.send(current).await;

        let connection = self.connection.clone();
        let checking_enabled = self.connectivity_checking_enabled;
        tokio::spawn(async move {
            let Ok(manager) = ManagerProxy::new(&connection).await else {
                return;
            };
            if checking_enabled {
                let mut changes = manager.receive_connectivity_changed().await;
                while let Some(change) = changes.next().await {
                    let Ok(raw) = change.get().await else {
                        continue;
                    };
                    if tx.send(connectivity_from_nm(raw)).await.is_err() {
                        break;
                    }
                }
            } else {
                let mut changes = manager.receive_state_changed().await;
                while let Some(change) = changes.next().await {
                    let Ok(raw) = change.get().await else {
                        continue;
                    };
                    if tx.send(connectivity_from_nm_state(raw)).await.is_err() {
                        break;
                    }
                }
            }
        });
        Ok(rx)
    }

    async fn recheck_connectivity(&self) -> mx::Result<Connectivity> {
        let manager = self.manager().await?;
        if self.connectivity_checking_enabled {
            Ok(connectivity_from_nm(manager.check_connectivity().await?))
        } else {
            Ok(connectivity_from_nm_state(manager.state().await?))
        }
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

        let last_scan_before = wireless.last_scan().await.unwrap_or(-1);
        // NM rate-limits this itself; errors (e.g. a scan already in flight)
        // are fine to ignore, `LastScan` polling below still catches the result.
        let _ = wireless.request_scan(HashMap::new()).await;

        let deadline = tokio::time::Instant::now() + SCAN_TIMEOUT;
        while tokio::time::Instant::now() < deadline {
            if wireless
                .last_scan()
                .await
                .is_ok_and(|s| s != last_scan_before)
            {
                break;
            }
            tokio::time::sleep(SCAN_POLL_INTERVAL).await;
        }

        let active_ap_path = wireless.active_access_point().await.ok();

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
            let flags = ap.flags().await.unwrap_or(0);
            let wpa = ap.wpa_flags().await.unwrap_or(0);
            let rsn = ap.rsn_flags().await.unwrap_or(0);
            access_points.push(WifiAccessPoint {
                ssid,
                strength: ap.strength().await.unwrap_or(0),
                security: security_from_flags(flags, wpa, rsn),
                active: active_ap_path.as_ref() == Some(&ap_path),
            });
        }
        Ok(dedup_access_points(access_points))
    }

    async fn connect_wifi(&self, request: WifiConnectRequest) -> mx::Result<()> {
        if request.security == WifiSecurity::Enterprise {
            return Err(mx::Error::Backend(
                "Enterprise networks aren't supported by the installer".to_string(),
            ));
        }
        let device_path = self.first_wifi_device().await?;
        let specific_path = if request.hidden {
            None
        } else {
            self.ap_path_for_ssid(&device_path, &request.ssid).await?
        };
        let root = ObjectPath::try_from("/").expect("well-formed root object path");
        let specific = specific_path
            .as_ref()
            .map(|p| p.as_ref())
            .unwrap_or_else(|| root.as_ref());

        let manager = self.manager().await?;
        let settings = wireless_settings(&request);
        let (profile_path, active_path) = manager
            .add_and_activate_connection(settings, &device_path.as_ref(), &specific)
            .await?;

        match tokio::time::timeout(ACTIVATION_TIMEOUT, self.await_activation(&active_path)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                self.discard_profile(&profile_path).await;
                Err(e)
            }
            Err(_) => {
                self.discard_profile(&profile_path).await;
                Err(mx::Error::Backend(
                    "Timed out while connecting to the Wi-Fi network".to_string(),
                ))
            }
        }
    }

    async fn portal_page(&self) -> mx::Result<PortalPage> {
        let manager = self.manager().await?;
        // There is no D-Bus call that returns the captive portal's actual
        // login URL. NM's own connectivity-check URI is what triggers the
        // portal's redirect when loaded on a captive network (the same trick
        // desktop shells use) — fall back to the offline demo page if
        // connectivity checking is disabled/unavailable (see `connect()`).
        match manager.connectivity_check_uri().await {
            Ok(uri) if !uri.is_empty() => Ok(PortalPage::Uri(uri)),
            _ => Ok(PortalPage::Builtin),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connectivity_from_nm_maps_known_values() {
        assert_eq!(connectivity_from_nm(0), Connectivity::Unknown);
        assert_eq!(connectivity_from_nm(1), Connectivity::None);
        assert_eq!(connectivity_from_nm(2), Connectivity::Portal);
        assert_eq!(connectivity_from_nm(3), Connectivity::Limited);
        assert_eq!(connectivity_from_nm(4), Connectivity::Full);
        assert_eq!(connectivity_from_nm(99), Connectivity::Unknown);
    }

    #[test]
    fn connectivity_from_nm_state_maps_known_values() {
        assert_eq!(connectivity_from_nm_state(20), Connectivity::None);
        assert_eq!(connectivity_from_nm_state(50), Connectivity::Limited);
        assert_eq!(connectivity_from_nm_state(60), Connectivity::Limited);
        assert_eq!(connectivity_from_nm_state(70), Connectivity::Full);
    }

    #[test]
    fn security_open_has_no_flags() {
        assert_eq!(security_from_flags(0, 0, 0), WifiSecurity::Open);
    }

    #[test]
    fn security_wpa2_from_rsn_psk() {
        assert_eq!(security_from_flags(0x1, 0, 0x100), WifiSecurity::Psk);
    }

    #[test]
    fn security_wpa3_only_is_sae() {
        assert_eq!(security_from_flags(0x1, 0, 0x400), WifiSecurity::Sae);
    }

    #[test]
    fn security_wpa3_transition_prefers_psk() {
        assert_eq!(security_from_flags(0x1, 0, 0x500), WifiSecurity::Psk);
    }

    #[test]
    fn security_enterprise_from_8021x() {
        assert_eq!(security_from_flags(0x1, 0x200, 0), WifiSecurity::Enterprise);
    }

    #[test]
    fn security_owe_is_open() {
        assert_eq!(security_from_flags(0, 0, 0x800), WifiSecurity::Open);
    }

    #[test]
    fn security_bare_wep_is_psk() {
        assert_eq!(security_from_flags(0x1, 0, 0), WifiSecurity::Psk);
    }

    #[test]
    fn key_mgmt_for_each_security() {
        assert_eq!(key_mgmt_for(WifiSecurity::Open), None);
        assert_eq!(key_mgmt_for(WifiSecurity::Psk), Some("wpa-psk"));
        assert_eq!(key_mgmt_for(WifiSecurity::Sae), Some("sae"));
        assert_eq!(key_mgmt_for(WifiSecurity::Enterprise), None);
    }

    #[test]
    fn activation_error_msgid_maps_known_reasons() {
        assert_eq!(activation_error_msgid(9), "Incorrect Wi-Fi password");
        assert_eq!(activation_error_msgid(10), "Wi-Fi authentication failed");
        assert_eq!(
            activation_error_msgid(6),
            "Timed out while connecting to the Wi-Fi network"
        );
        assert_eq!(
            activation_error_msgid(5),
            "Couldn't get an IP address from this network"
        );
        assert_eq!(
            activation_error_msgid(3),
            "The Wi-Fi device was disconnected"
        );
        assert_eq!(
            activation_error_msgid(255),
            "Couldn't connect to this Wi-Fi network"
        );
    }
}
