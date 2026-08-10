//! One row per Wi-Fi access point (network step).

use crate::backend::net::{WifiAccessPoint, WifiSecurity};
use crate::i18n::tr;
use adw::prelude::*;

/// Signal strength bucketed 1-4, matching [`wifi_strength_icon`]'s ranges —
/// what the description narrates as "signal N of 4" instead of a raw
/// percentage that means nothing spoken aloud.
fn signal_bucket(strength: u8) -> u8 {
    match strength {
        0..=24 => 1,
        25..=49 => 2,
        50..=74 => 3,
        _ => 4,
    }
}

/// One full sentence per `tr()` call (not fragments glued together), with a
/// `{level}` placeholder substituted after translation so the digit doesn't
/// need its own catalog entry.
pub fn accessible_description(security: WifiSecurity, strength: u8) -> String {
    let template = match security {
        WifiSecurity::Open => tr("Open network, signal {level} of 4"),
        WifiSecurity::Enterprise => tr("Enterprise network, signal {level} of 4"),
        WifiSecurity::Psk | WifiSecurity::Sae => tr("Secured network, signal {level} of 4"),
    };
    template.replace("{level}", &signal_bucket(strength).to_string())
}

#[derive(Clone)]
pub struct ApRow {
    row: adw::ActionRow,
    ssid: String,
    active_icon: gtk::Image,
    spinner: adw::Spinner,
    error_icon: gtk::Image,
    base_description: String,
}

impl ApRow {
    pub fn new(ap: &WifiAccessPoint) -> Self {
        let is_enterprise = ap.security == WifiSecurity::Enterprise;
        let subtitle = if is_enterprise {
            tr("Enterprise networks aren't supported by the installer")
        } else {
            format!("{}%", ap.strength)
        };
        let row = adw::ActionRow::builder()
            .title(ap.ssid.clone())
            .subtitle(subtitle)
            .activatable(!is_enterprise)
            .build();
        let base_description = accessible_description(ap.security, ap.strength);
        row.update_property(&[
            gtk::accessible::Property::Label(&ap.ssid),
            gtk::accessible::Property::Description(&base_description),
        ]);

        let active_icon = gtk::Image::from_icon_name("object-select-symbolic");
        active_icon.set_visible(ap.active);
        row.add_suffix(&active_icon);

        let strength_icon = gtk::Image::from_icon_name(wifi_strength_icon(ap.strength));
        row.add_suffix(&strength_icon);

        if ap.security != WifiSecurity::Open {
            let lock_icon = gtk::Image::from_icon_name("network-wireless-encrypted-symbolic");
            row.add_suffix(&lock_icon);
        }

        let spinner = adw::Spinner::new();
        spinner.set_visible(false);
        row.add_suffix(&spinner);

        let error_icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
        error_icon.add_css_class("error");
        error_icon.set_visible(false);
        row.add_suffix(&error_icon);

        Self {
            row,
            ssid: ap.ssid.clone(),
            active_icon,
            spinner,
            error_icon,
            base_description,
        }
    }

    pub fn widget(&self) -> gtk::Widget {
        self.row.clone().upcast()
    }

    pub fn ssid(&self) -> &str {
        &self.ssid
    }

    /// Shows a spinner suffix and disables the row while a connection attempt is in flight.
    pub fn set_busy(&self, busy: bool) {
        self.spinner.set_visible(busy);
        self.row.set_sensitive(!busy);
    }

    pub fn set_error(&self, message: Option<&str>) {
        self.error_icon.set_visible(message.is_some());
        self.error_icon.set_tooltip_text(message);
        let description = match message {
            Some(message) => format!("{}. {message}", self.base_description),
            None => self.base_description.clone(),
        };
        self.row
            .update_property(&[gtk::accessible::Property::Description(&description)]);
    }

    pub fn set_active(&self, active: bool) {
        self.active_icon.set_visible(active);
    }

    pub fn connect_activated<F: Fn(&adw::ActionRow) + 'static>(&self, f: F) {
        self.row.connect_activated(f);
    }
}

fn wifi_strength_icon(strength: u8) -> &'static str {
    match strength {
        0..=24 => "network-wireless-signal-weak-symbolic",
        25..=49 => "network-wireless-signal-ok-symbolic",
        50..=74 => "network-wireless-signal-good-symbolic",
        _ => "network-wireless-signal-excellent-symbolic",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strength_icon_bucket_boundaries() {
        assert_eq!(
            wifi_strength_icon(0),
            "network-wireless-signal-weak-symbolic"
        );
        assert_eq!(
            wifi_strength_icon(24),
            "network-wireless-signal-weak-symbolic"
        );
        assert_eq!(
            wifi_strength_icon(25),
            "network-wireless-signal-ok-symbolic"
        );
        assert_eq!(
            wifi_strength_icon(49),
            "network-wireless-signal-ok-symbolic"
        );
        assert_eq!(
            wifi_strength_icon(50),
            "network-wireless-signal-good-symbolic"
        );
        assert_eq!(
            wifi_strength_icon(74),
            "network-wireless-signal-good-symbolic"
        );
        assert_eq!(
            wifi_strength_icon(75),
            "network-wireless-signal-excellent-symbolic"
        );
        assert_eq!(
            wifi_strength_icon(100),
            "network-wireless-signal-excellent-symbolic"
        );
    }

    #[test]
    fn accessible_description_reports_security_and_bucket() {
        assert_eq!(
            accessible_description(WifiSecurity::Open, 10),
            "Open network, signal 1 of 4"
        );
        assert_eq!(
            accessible_description(WifiSecurity::Psk, 60),
            "Secured network, signal 3 of 4"
        );
        assert_eq!(
            accessible_description(WifiSecurity::Sae, 90),
            "Secured network, signal 4 of 4"
        );
        assert_eq!(
            accessible_description(WifiSecurity::Enterprise, 30),
            "Enterprise network, signal 2 of 4"
        );
    }
}
