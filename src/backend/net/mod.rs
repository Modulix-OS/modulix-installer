mod fake;
mod network_manager;

pub use fake::FakeNetworkBackend;
pub use network_manager::NetworkManagerBackend;

use crate::mx;
use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connectivity {
    Unknown,
    None,
    /// Behind a captive portal — step 6 must open the embedded WebKitGTK window (iteration 2).
    Portal,
    Limited,
    Full,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WifiAccessPoint {
    pub ssid: String,
    /// 0-100.
    pub strength: u8,
    pub secured: bool,
}

/// Ethernet/Wi-Fi connectivity, real impl over NetworkManager D-Bus.
#[async_trait]
pub trait NetworkBackend: Send + Sync {
    async fn connectivity(&self) -> mx::Result<Connectivity>;
    async fn is_ethernet_connected(&self) -> mx::Result<bool>;
    async fn scan_wifi(&self) -> mx::Result<Vec<WifiAccessPoint>>;
    async fn connect_wifi(&self, ssid: &str, password: Option<&str>) -> mx::Result<()>;
}
