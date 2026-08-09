use super::catalog;
use super::{KeyboardLayout, KeyboardVariant, LocaleBackend, LocaleEntry, TimezoneEntry};
use crate::mx;
use async_trait::async_trait;

pub struct FakeLocaleBackend;

impl FakeLocaleBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FakeLocaleBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// `--fake` never needs to fake locale/timezone/keyboard *catalogs* — reading
/// `SUPPORTED`/`zone.tab`/`evdev.xml` touches no hardware and needs no root,
/// unlike disk/network/crypt. Only [`apply_keyboard_layout`] (a real side
/// effect on the running session) is actually faked here; everything else
/// delegates to the same reads [`super::SystemLocaleBackend`] uses, falling
/// back to a tiny hardcoded catalog only if none of those files exist at all
/// (e.g. developing outside a Linux/glibc environment).
#[async_trait]
impl LocaleBackend for FakeLocaleBackend {
    async fn list_locales(&self) -> mx::Result<Vec<LocaleEntry>> {
        match catalog::read_locales().await {
            Ok(entries) if !entries.is_empty() => Ok(entries),
            _ => Ok(fallback_locales()),
        }
    }

    async fn list_timezones(&self) -> mx::Result<Vec<TimezoneEntry>> {
        match catalog::read_timezones().await {
            Ok(entries) if !entries.is_empty() => Ok(entries),
            _ => Ok(fallback_timezones()),
        }
    }

    async fn list_keyboard_layouts(&self) -> mx::Result<Vec<KeyboardLayout>> {
        match catalog::read_keyboard_layouts().await {
            Ok(layouts) if !layouts.is_empty() => Ok(layouts),
            _ => Ok(fallback_keyboard_layouts()),
        }
    }

    async fn apply_keyboard_layout(&self, _layout: &str, _variant: &str) -> mx::Result<()> {
        Ok(())
    }
}

fn fallback_locales() -> Vec<LocaleEntry> {
    ["en_US.UTF-8", "fr_FR.UTF-8", "de_DE.UTF-8", "es_ES.UTF-8"]
        .into_iter()
        .map(|code| LocaleEntry {
            code: code.to_string(),
        })
        .collect()
}

fn fallback_timezones() -> Vec<TimezoneEntry> {
    vec![
        TimezoneEntry {
            name: "Europe/Paris".into(),
            latitude: 48.8667,
            longitude: 2.3333,
        },
        TimezoneEntry {
            name: "America/New_York".into(),
            latitude: 40.7141,
            longitude: -74.0063,
        },
        TimezoneEntry {
            name: "Asia/Tokyo".into(),
            latitude: 35.6833,
            longitude: 139.75,
        },
        TimezoneEntry {
            name: "Australia/Sydney".into(),
            latitude: -33.8833,
            longitude: 151.2167,
        },
    ]
}

fn fallback_keyboard_layouts() -> Vec<KeyboardLayout> {
    vec![
        KeyboardLayout {
            code: "us".into(),
            description: "English (US)".into(),
            variants: vec![],
        },
        KeyboardLayout {
            code: "fr".into(),
            description: "French".into(),
            variants: vec![
                KeyboardVariant {
                    code: "oss".into(),
                    description: "French (alt.)".into(),
                },
                KeyboardVariant {
                    code: "bepo".into(),
                    description: "French (BEPO)".into(),
                },
            ],
        },
        KeyboardLayout {
            code: "de".into(),
            description: "German".into(),
            variants: vec![],
        },
    ]
}
