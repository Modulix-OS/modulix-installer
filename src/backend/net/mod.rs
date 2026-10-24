mod network_manager;

pub use network_manager::NetworkManagerBackend;

use crate::mx;
use crate::secret::Secret;
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
    /// `Secret`, so a pre-shared key typed into the wizard is wiped
    /// when the last copy of this request goes away.
    pub password: Option<Secret>,
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

/// One Wi-Fi connection profile NetworkManager has persisted on the live
/// system, as `WifiProfileTask` copies it onto the install target.
///
/// * `ssid` - network the profile connects to, for the install log only.
/// * `keyfile` - absolute path of the keyfile NetworkManager wrote it to,
///   read back from the profile's own `Filename` property rather than built
///   from the SSID (which NetworkManager escapes when naming the file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiProfile {
    pub ssid: String,
    pub keyfile: std::path::PathBuf,
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
    /// Every Wi-Fi profile NetworkManager has written to disk on the live
    /// system, so the install can carry the network the wizard ran over onto
    /// the installed system. A live ISO starts with none, and a failed
    /// association deletes the profile it created (see
    /// `NetworkManagerBackend::discard_profile`), so what this returns is
    /// exactly the networks that worked during the install.
    ///
    /// # Post-conditions
    /// Every returned `keyfile` is an absolute path NetworkManager reported
    /// itself; the file may still carry the pre-shared key, so a caller must
    /// keep its `0600` mode.
    ///
    /// # Returns
    /// One entry per saved Wi-Fi profile, empty when the install ran over
    /// ethernet or offline.
    async fn saved_wifi_profiles(&self) -> mx::Result<Vec<WifiProfile>>;
    async fn portal_page(&self) -> mx::Result<PortalPage>;
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
