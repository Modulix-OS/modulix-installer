//! One row per Wi-Fi access point (network step).

use crate::backend::net::WifiAccessPoint;
use adw::prelude::*;

pub struct ApRow {
    row: adw::ActionRow,
}

impl ApRow {
    pub fn new(ap: &WifiAccessPoint) -> Self {
        let row = adw::ActionRow::builder()
            .title(ap.ssid.clone())
            .subtitle(format!("{}%", ap.strength))
            .activatable(true)
            .build();

        let strength_icon = gtk::Image::from_icon_name(wifi_strength_icon(ap.strength));
        row.add_suffix(&strength_icon);

        if ap.secured {
            let lock_icon = gtk::Image::from_icon_name("network-wireless-encrypted-symbolic");
            row.add_suffix(&lock_icon);
        }

        Self { row }
    }

    pub fn widget(&self) -> gtk::Widget {
        self.row.clone().upcast()
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
