use super::{
    Connectivity, NetworkBackend, PortalPage, WifiAccessPoint, WifiConnectRequest, WifiSecurity,
};
use crate::mx;
use async_trait::async_trait;
use std::sync::Mutex;
use std::time::Duration;

pub const FAKE_OPEN_SSID: &str = "Modulix-Fake-Open";
pub const FAKE_WPA2_SSID: &str = "Modulix-Fake-WPA2";
pub const FAKE_WPA3_SSID: &str = "Modulix-Fake-WPA3";
pub const FAKE_PORTAL_SSID: &str = "Modulix-Fake-Portal";
pub const FAKE_HIDDEN_SSID: &str = "Modulix-Fake-Hidden";
/// Only password accepted in `--fake`, so a wrong-password rejection is exercisable.
pub const FAKE_PASSWORD: &str = "modulix";

struct FakeState {
    connectivity: Connectivity,
    connected_ssid: Option<String>,
    watchers: Vec<async_channel::Sender<Connectivity>>,
}

/// Never held across an `.await` — every method below does its locking in a
/// tight, synchronous scope.
pub struct FakeNetworkBackend {
    state: Mutex<FakeState>,
}

impl FakeNetworkBackend {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(FakeState {
                connectivity: Connectivity::None,
                connected_ssid: None,
                watchers: Vec::new(),
            }),
        }
    }

    fn publish(&self, next: Connectivity) {
        let mut s = self
            .state
            .lock()
            .expect("fake network state mutex poisoned");
        if s.connectivity == next {
            return;
        }
        s.connectivity = next;
        // Unbounded channel: `try_send` only fails when the receiver was dropped.
        s.watchers.retain(|tx| tx.try_send(next).is_ok());
    }

    fn set_connected(&self, ssid: &str, connectivity: Connectivity) {
        {
            let mut s = self
                .state
                .lock()
                .expect("fake network state mutex poisoned");
            s.connected_ssid = Some(ssid.to_string());
        }
        self.publish(connectivity);
    }
}

impl Default for FakeNetworkBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl NetworkBackend for FakeNetworkBackend {
    async fn connectivity(&self) -> mx::Result<Connectivity> {
        Ok(self
            .state
            .lock()
            .expect("fake network state mutex poisoned")
            .connectivity)
    }

    async fn watch_connectivity(&self) -> mx::Result<async_channel::Receiver<Connectivity>> {
        let (tx, rx) = async_channel::unbounded();
        let mut s = self
            .state
            .lock()
            .expect("fake network state mutex poisoned");
        let _ = tx.try_send(s.connectivity);
        s.watchers.push(tx);
        Ok(rx)
    }

    async fn recheck_connectivity(&self) -> mx::Result<Connectivity> {
        self.connectivity().await
    }

    async fn is_ethernet_connected(&self) -> mx::Result<bool> {
        // The `--fake` run must exercise Wi-Fi, never the ethernet shortcut.
        Ok(false)
    }

    async fn scan_wifi(&self) -> mx::Result<Vec<WifiAccessPoint>> {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let connected = self
            .state
            .lock()
            .expect("fake network state mutex poisoned")
            .connected_ssid
            .clone();
        let is_active = |ssid: &str| connected.as_deref() == Some(ssid);
        Ok(vec![
            WifiAccessPoint {
                ssid: FAKE_OPEN_SSID.to_string(),
                strength: 90,
                security: WifiSecurity::Open,
                active: is_active(FAKE_OPEN_SSID),
            },
            // Deliberately not deduplicated — exercises `dedup_access_points` in `--fake`.
            WifiAccessPoint {
                ssid: FAKE_OPEN_SSID.to_string(),
                strength: 40,
                security: WifiSecurity::Open,
                active: is_active(FAKE_OPEN_SSID),
            },
            WifiAccessPoint {
                ssid: FAKE_WPA2_SSID.to_string(),
                strength: 70,
                security: WifiSecurity::Psk,
                active: is_active(FAKE_WPA2_SSID),
            },
            WifiAccessPoint {
                ssid: FAKE_WPA2_SSID.to_string(),
                strength: 30,
                security: WifiSecurity::Psk,
                active: is_active(FAKE_WPA2_SSID),
            },
            WifiAccessPoint {
                ssid: FAKE_WPA3_SSID.to_string(),
                strength: 65,
                security: WifiSecurity::Sae,
                active: is_active(FAKE_WPA3_SSID),
            },
            WifiAccessPoint {
                ssid: FAKE_PORTAL_SSID.to_string(),
                strength: 55,
                security: WifiSecurity::Open,
                active: is_active(FAKE_PORTAL_SSID),
            },
            // `Modulix-Fake-Hidden` is intentionally absent: hidden networks
            // aren't broadcast, so they never show up in a scan — the step
            // joins it through the dedicated "hidden network" row instead.
        ])
    }

    async fn connect_wifi(&self, request: WifiConnectRequest) -> mx::Result<()> {
        match request.ssid.as_str() {
            FAKE_OPEN_SSID => {
                self.set_connected(FAKE_OPEN_SSID, Connectivity::Full);
                Ok(())
            }
            FAKE_PORTAL_SSID => {
                self.set_connected(FAKE_PORTAL_SSID, Connectivity::Portal);
                Ok(())
            }
            ssid @ (FAKE_WPA2_SSID | FAKE_WPA3_SSID | FAKE_HIDDEN_SSID) => {
                if request.password.as_deref() == Some(FAKE_PASSWORD) {
                    self.set_connected(ssid, Connectivity::Full);
                    Ok(())
                } else {
                    Err(mx::Error::Backend("Incorrect Wi-Fi password".to_string()))
                }
            }
            _ => Err(mx::Error::Backend("Wi-Fi network not found".to_string())),
        }
    }

    async fn portal_page(&self) -> mx::Result<PortalPage> {
        Ok(PortalPage::Builtin)
    }

    async fn complete_portal(&self) -> mx::Result<()> {
        self.publish(Connectivity::Full);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::net::dedup_access_points;

    fn request(ssid: &str, password: Option<&str>, hidden: bool) -> WifiConnectRequest {
        WifiConnectRequest {
            ssid: ssid.to_string(),
            password: password.map(str::to_string),
            security: WifiSecurity::Psk,
            hidden,
        }
    }

    #[tokio::test]
    async fn starts_disconnected() {
        let backend = FakeNetworkBackend::new();
        assert_eq!(backend.connectivity().await.unwrap(), Connectivity::None);
    }

    #[tokio::test]
    async fn watch_seeds_current_value() {
        let backend = FakeNetworkBackend::new();
        let rx = backend.watch_connectivity().await.unwrap();
        assert_eq!(rx.recv().await.unwrap(), Connectivity::None);
    }

    #[tokio::test]
    async fn wrong_password_is_rejected_and_leaves_state_alone() {
        let backend = FakeNetworkBackend::new();
        let err = backend
            .connect_wifi(request(FAKE_WPA2_SSID, Some("wrong"), false))
            .await
            .unwrap_err();
        assert!(matches!(err, mx::Error::Backend(_)));
        assert_eq!(backend.connectivity().await.unwrap(), Connectivity::None);
    }

    #[tokio::test]
    async fn correct_password_reaches_full() {
        let backend = FakeNetworkBackend::new();
        backend
            .connect_wifi(request(FAKE_WPA2_SSID, Some(FAKE_PASSWORD), false))
            .await
            .unwrap();
        assert_eq!(backend.connectivity().await.unwrap(), Connectivity::Full);
    }

    #[tokio::test]
    async fn portal_ap_goes_portal_then_full() {
        let backend = FakeNetworkBackend::new();
        backend
            .connect_wifi(request(FAKE_PORTAL_SSID, None, false))
            .await
            .unwrap();
        assert_eq!(backend.connectivity().await.unwrap(), Connectivity::Portal);
        backend.complete_portal().await.unwrap();
        assert_eq!(backend.connectivity().await.unwrap(), Connectivity::Full);
    }

    #[tokio::test]
    async fn every_watcher_sees_every_change() {
        let backend = FakeNetworkBackend::new();
        let rx1 = backend.watch_connectivity().await.unwrap();
        let rx2 = backend.watch_connectivity().await.unwrap();
        assert_eq!(rx1.recv().await.unwrap(), Connectivity::None);
        assert_eq!(rx2.recv().await.unwrap(), Connectivity::None);

        backend
            .connect_wifi(request(FAKE_OPEN_SSID, None, false))
            .await
            .unwrap();

        assert_eq!(rx1.recv().await.unwrap(), Connectivity::Full);
        assert_eq!(rx2.recv().await.unwrap(), Connectivity::Full);
    }

    #[tokio::test]
    async fn dropped_watcher_is_pruned() {
        let backend = FakeNetworkBackend::new();
        {
            let _rx = backend.watch_connectivity().await.unwrap();
            assert_eq!(
                backend.state.lock().unwrap().watchers.len(),
                1,
                "watcher should be registered while the receiver is alive"
            );
        }
        // `_rx` dropped here; the next publish should prune it via `retain`.
        backend
            .connect_wifi(request(FAKE_OPEN_SSID, None, false))
            .await
            .unwrap();
        assert_eq!(backend.state.lock().unwrap().watchers.len(), 0);
    }

    #[tokio::test]
    async fn hidden_network_requires_the_documented_password() {
        let backend = FakeNetworkBackend::new();
        backend
            .connect_wifi(request(FAKE_HIDDEN_SSID, Some(FAKE_PASSWORD), true))
            .await
            .unwrap();
        assert_eq!(backend.connectivity().await.unwrap(), Connectivity::Full);
    }

    #[tokio::test]
    async fn scan_returns_duplicates_that_dedup_collapses() {
        let backend = FakeNetworkBackend::new();
        let aps = backend.scan_wifi().await.unwrap();
        assert_eq!(aps.len(), 6);
        let deduped = dedup_access_points(aps);
        assert_eq!(deduped.len(), 4);
    }
}
