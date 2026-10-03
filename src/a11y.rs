//! Shared accessibility state (step 5 + the global header popover) and its
//! live effects on the installer itself. `high-contrast`/`large-text` apply
//! immediately (CSS provider / `gtk-xft-dpi`) because they're needed to use
//! the rest of the wizard; `screen-magnifier`/`sticky-keys` are compositor
//! level and the kiosk compositor doesn't expose them to us, so they're only best-effort
//! `gsettings` calls (useful on a dev machine under GNOME) plus a value
//! carried into `InstallConfig` for the system being installed.

use crate::backend::Backends;
use crate::bridge;

glib::wrapper! {
    pub struct A11ySettings(ObjectSubclass<imp::A11ySettings>);
}

impl A11ySettings {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Wires `notify::*` handlers that make `high-contrast`/`large-text`
    /// take effect on the installer's own window right away, and forward
    /// every toggle to the (best-effort) backend. Call once, after the
    /// window's `Display` exists.
    pub fn connect_live_effects(&self, backends: &Backends, runtime: &tokio::runtime::Handle) {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(HIGH_CONTRAST_CSS);

        self.connect_high_contrast_notify(move |settings| {
            let Some(display) = gtk::gdk::Display::default() else {
                return;
            };
            if settings.high_contrast() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_USER,
                );
                adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceLight);
            } else {
                gtk::style_context_remove_provider_for_display(&display, &provider);
                adw::StyleManager::default().set_color_scheme(adw::ColorScheme::PreferDark);
            }
        });

        let base_dpi = gtk::Settings::default()
            .map(|s| s.gtk_xft_dpi())
            .unwrap_or(0);
        self.connect_large_text_notify(move |settings| {
            let Some(gtk_settings) = gtk::Settings::default() else {
                return;
            };
            gtk_settings.set_gtk_xft_dpi(xft_dpi_for(base_dpi, settings.large_text()));
        });

        {
            let a11y_backend = backends.a11y.clone();
            let runtime = runtime.clone();
            self.connect_narrator_notify(move |settings| {
                let enabled = settings.narrator();
                let backend = a11y_backend.clone();
                bridge::spawn(
                    &runtime,
                    async move { backend.set_narrator_enabled(enabled).await },
                    |result| {
                        if let Err(e) = result {
                            eprintln!("failed to toggle narrator: {e}");
                        }
                    },
                );
            });
        }

        {
            let a11y_backend = backends.a11y.clone();
            let runtime = runtime.clone();
            self.connect_screen_magnifier_notify(move |settings| {
                let enabled = settings.screen_magnifier();
                let backend = a11y_backend.clone();
                bridge::spawn(
                    &runtime,
                    async move { backend.set_screen_magnifier(enabled).await },
                    |result| {
                        if let Err(e) = result {
                            eprintln!("failed to apply screen magnifier setting: {e}");
                        }
                    },
                );
            });
        }

        {
            let a11y_backend = backends.a11y.clone();
            let runtime = runtime.clone();
            self.connect_sticky_keys_notify(move |settings| {
                let enabled = settings.sticky_keys();
                let backend = a11y_backend.clone();
                bridge::spawn(
                    &runtime,
                    async move { backend.set_sticky_keys(enabled).await },
                    |result| {
                        if let Err(e) = result {
                            eprintln!("failed to apply sticky keys setting: {e}");
                        }
                    },
                );
            });
        }
    }
}

impl Default for A11ySettings {
    fn default() -> Self {
        Self::new()
    }
}

/// libadwaita >= 1.6 reads its palette from CSS variables, so redefining
/// them at `STYLE_PROVIDER_PRIORITY_USER` is enough to force a legible high
/// contrast theme — no GTK theme switch needed (libadwaita ignores
/// `gtk-theme-name` and a `HighContrast` theme isn't guaranteed present on
/// the ISO anyway). `ForceLight` is required alongside this provider: the
/// values below are written for a light base, and GNOME's own HighContrast
/// theme is light too.
const HIGH_CONTRAST_CSS: &str = "
:root {
  --window-bg-color:#fff; --window-fg-color:#000;
  --view-bg-color:#fff;   --view-fg-color:#000;
  --headerbar-bg-color:#fff; --headerbar-fg-color:#000;
  --sidebar-bg-color:#fff;   --sidebar-fg-color:#000;
  --card-bg-color:#fff; --popover-bg-color:#fff; --dialog-bg-color:#fff;
  --accent-bg-color:#00c; --accent-color:#00c; --accent-fg-color:#fff;
  --borders:#000;
}
row, button, entry, .card { border:1px solid #000; }
*:focus-visible { outline:3px solid #00c; outline-offset:1px; }
";

/// `text-scaling-factor`'s exact equivalent for `GtkSettings`: `gtk-xft-dpi`
/// is expressed in 1024ths of a point. `base` is read once at startup (not
/// re-read on every toggle) so this never clobbers an existing HiDPI setting
/// — flipping "large text" off always returns to the base the session
/// started with, not a hardcoded 96 DPI.
pub fn xft_dpi_for(base: i32, large_text: bool) -> i32 {
    let base = if base > 0 { base } else { 96 * 1024 };
    if large_text {
        (base as f64 * 1.25) as i32
    } else {
        base
    }
}

mod imp {
    use glib::prelude::*;
    use glib::subclass::prelude::*;
    use std::cell::Cell;

    #[derive(glib::Properties, Default)]
    #[properties(wrapper_type = super::A11ySettings)]
    pub struct A11ySettings {
        #[property(get, set)]
        narrator: Cell<bool>,
        #[property(get, set, name = "high-contrast")]
        high_contrast: Cell<bool>,
        #[property(get, set, name = "large-text")]
        large_text: Cell<bool>,
        #[property(get, set, name = "screen-magnifier")]
        screen_magnifier: Cell<bool>,
        #[property(get, set, name = "sticky-keys")]
        sticky_keys: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for A11ySettings {
        const NAME: &'static str = "ModulixA11ySettings";
        type Type = super::A11ySettings;
    }

    #[glib::derived_properties]
    impl ObjectImpl for A11ySettings {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xft_dpi_defaults_to_96_when_base_unknown() {
        assert_eq!(xft_dpi_for(0, false), 96 * 1024);
    }

    #[test]
    fn xft_dpi_scales_large_text_by_1_25() {
        assert_eq!(xft_dpi_for(96 * 1024, true), (96.0 * 1024.0 * 1.25) as i32);
    }

    #[test]
    fn xft_dpi_preserves_existing_hidpi_base() {
        let hidpi_base = 192 * 1024;
        assert_eq!(xft_dpi_for(hidpi_base, false), hidpi_base);
        assert_eq!(
            xft_dpi_for(hidpi_base, true),
            (hidpi_base as f64 * 1.25) as i32
        );
    }
}
