use super::{Connectivity, NetworkBackend, WifiAccessPoint};
use crate::mx;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct FakeNetworkBackend {
    connected: AtomicBool,
}

impl FakeNetworkBackend {
    pub fn new() -> Self {
        Self {
            connected: AtomicBool::new(true),
        }
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
        Ok(if self.connected.load(Ordering::Relaxed) {
            Connectivity::Full
        } else {
            Connectivity::None
        })
    }

    async fn is_ethernet_connected(&self) -> mx::Result<bool> {
        Ok(self.connected.load(Ordering::Relaxed))
    }

    async fn scan_wifi(&self) -> mx::Result<Vec<WifiAccessPoint>> {
        Ok(vec![
            WifiAccessPoint {
                ssid: "Modulix-Fake-Open".into(),
                strength: 90,
                secured: false,
            },
            WifiAccessPoint {
                ssid: "Modulix-Fake-WPA2".into(),
                strength: 62,
                secured: true,
            },
        ])
    }

    async fn connect_wifi(&self, _ssid: &str, _password: Option<&str>) -> mx::Result<()> {
        self.connected.store(true, Ordering::Relaxed);
        Ok(())
    }
}
