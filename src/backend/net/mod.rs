mod fake;
mod network_manager;

pub use fake::FakeNetworkBackend;
pub use network_manager::NetworkManagerBackend;

use crate::mx;
use async_trait::async_trait;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Connectivity {
    #[default]
    Unknown,
    None,
    /// Behind a captive portal — step 6 opens the embedded WebKitGTK window.
    Portal,
    Limited,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiSecurity {
    Open,
    Psk,
    Sae,
    Enterprise,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiAccessPoint {
    pub ssid: String,
    /// 0-100.
    pub strength: u8,
    pub security: WifiSecurity,
    pub active: bool,
}

/// A request to join a Wi-Fi network. Fields are owned (not borrowed) because
/// every caller moves this into a `bridge::spawn`'d future, which must be `'static`.
#[derive(Clone)]
pub struct WifiConnectRequest {
    pub ssid: String,
    pub password: Option<String>,
    pub security: WifiSecurity,
    pub hidden: bool,
}

impl fmt::Debug for WifiConnectRequest {
    /// Hand-written so the password never lands in a log line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WifiConnectRequest")
            .field("ssid", &self.ssid)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("security", &self.security)
            .field("hidden", &self.hidden)
            .finish()
    }
}

pub enum PortalPage {
    Uri(String),
    Builtin,
}

/// Ethernet/Wi-Fi connectivity, real impl over NetworkManager D-Bus.
#[async_trait]
pub trait NetworkBackend: Send + Sync {
    async fn connectivity(&self) -> mx::Result<Connectivity>;
    /// First item is the current value, then one item per transition.
    async fn watch_connectivity(&self) -> mx::Result<async_channel::Receiver<Connectivity>>;
    /// NM's own `CheckConnectivity` — its periodic check defaults to every 300s.
    async fn recheck_connectivity(&self) -> mx::Result<Connectivity>;
    async fn is_ethernet_connected(&self) -> mx::Result<bool>;
    async fn scan_wifi(&self) -> mx::Result<Vec<WifiAccessPoint>>;
    async fn connect_wifi(&self, request: WifiConnectRequest) -> mx::Result<()>;
    async fn portal_page(&self) -> mx::Result<PortalPage>;
    /// `--fake`-only hook; no-op for NetworkManager (the real portal login
    /// itself is what flips connectivity there).
    async fn complete_portal(&self) -> mx::Result<()> {
        Ok(())
    }
}

fn security_rank(security: WifiSecurity) -> u8 {
    match security {
        WifiSecurity::Open => 0,
        WifiSecurity::Psk => 1,
        WifiSecurity::Sae => 2,
        WifiSecurity::Enterprise => 3,
    }
}

/// The same SSID broadcast on 2.4 and 5 GHz shows up as two access points;
/// NetworkManager's own UI collapses these into a single network, so this
/// installer does too. Keeps the best signal and the most capable security
/// seen under that SSID, ORs `active` together, and sorts the result with
/// `active` networks first, then by descending signal strength.
pub(crate) fn dedup_access_points(aps: Vec<WifiAccessPoint>) -> Vec<WifiAccessPoint> {
    let mut merged: Vec<WifiAccessPoint> = Vec::with_capacity(aps.len());
    for ap in aps {
        if let Some(existing) = merged.iter_mut().find(|e| e.ssid == ap.ssid) {
            existing.strength = existing.strength.max(ap.strength);
            if security_rank(ap.security) > security_rank(existing.security) {
                existing.security = ap.security;
            }
            existing.active |= ap.active;
        } else {
            merged.push(ap);
        }
    }
    merged.sort_by(|a, b| b.active.cmp(&a.active).then(b.strength.cmp(&a.strength)));
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ap(ssid: &str, strength: u8, security: WifiSecurity, active: bool) -> WifiAccessPoint {
        WifiAccessPoint {
            ssid: ssid.to_string(),
            strength,
            security,
            active,
        }
    }

    #[test]
    fn dedup_keeps_max_strength_across_bands() {
        let merged = dedup_access_points(vec![
            ap("Home", 40, WifiSecurity::Psk, false),
            ap("Home", 90, WifiSecurity::Psk, false),
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].strength, 90);
    }

    #[test]
    fn dedup_keeps_most_capable_security() {
        let merged = dedup_access_points(vec![
            ap("Home", 50, WifiSecurity::Open, false),
            ap("Home", 50, WifiSecurity::Sae, false),
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].security, WifiSecurity::Sae);
    }

    #[test]
    fn dedup_ors_active_flag() {
        let merged = dedup_access_points(vec![
            ap("Home", 50, WifiSecurity::Psk, false),
            ap("Home", 30, WifiSecurity::Psk, true),
        ]);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].active);
    }

    #[test]
    fn dedup_sorts_active_first_then_by_strength() {
        let merged = dedup_access_points(vec![
            ap("Weak", 10, WifiSecurity::Open, false),
            ap("Strong-Inactive", 90, WifiSecurity::Open, false),
            ap("Active", 20, WifiSecurity::Open, true),
        ]);
        let ssids: Vec<&str> = merged.iter().map(|a| a.ssid.as_str()).collect();
        assert_eq!(ssids, vec!["Active", "Strong-Inactive", "Weak"]);
    }
}
