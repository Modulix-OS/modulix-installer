//! Shared, read-only catalog lookups. Locale/timezone/keyboard-layout lists
//! are static system data, not hardware — safe to read the same way whether
//! or not `--fake` is set, unlike disk/network/crypt operations. Both
//! [`super::SystemLocaleBackend`] and [`super::FakeLocaleBackend`] go through
//! here; only [`super::LocaleBackend::apply_keyboard_layout`] (an actual
//! side effect) differs between them.

use super::{KeyboardLayout, LocaleEntry, TimezoneEntry, parse_evdev_xml, parse_zone_tab};
use crate::mx;

/// `/usr/share/...` is the FHS path; `/run/current-system/sw/share/...` is
/// where a NixOS system profile symlinks packages that provide it.
const ZONE_TAB_PATHS: &[&str] = &[
    "/usr/share/zoneinfo/zone1970.tab",
    "/usr/share/zoneinfo/zone.tab",
    "/run/current-system/sw/share/zoneinfo/zone1970.tab",
    "/run/current-system/sw/share/zoneinfo/zone.tab",
];

const EVDEV_XML_PATHS: &[&str] = &[
    "/usr/share/X11/xkb/rules/evdev.xml",
    "/run/current-system/sw/share/X11/xkb/rules/evdev.xml",
];

/// glibc's full locale catalog (name + charset per line), independent of
/// which locales happen to be generated on this system already.
const SUPPORTED_LOCALES_PATHS: &[&str] = &[
    "/run/current-system/sw/share/i18n/SUPPORTED",
    "/usr/share/i18n/SUPPORTED",
];

/// `TZDIR` is a real, standard glibc/tzdata env var — respecting it (as
/// glibc itself does) is what makes `zone.tab` lookup work in a plain `nix
/// develop` shell, where there's no `/run/current-system` NixOS profile to
/// symlink it under `/usr/share`.
async fn read_first_existing(paths: impl Iterator<Item = String>) -> Option<String> {
    for path in paths {
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            return Some(content);
        }
    }
    None
}

pub async fn read_timezones() -> mx::Result<Vec<TimezoneEntry>> {
    let tzdir_candidates = std::env::var("TZDIR")
        .into_iter()
        .flat_map(|dir| vec![format!("{dir}/zone1970.tab"), format!("{dir}/zone.tab")]);
    let candidates = tzdir_candidates.chain(ZONE_TAB_PATHS.iter().map(|s| s.to_string()));

    match read_first_existing(candidates).await {
        Some(content) => Ok(parse_zone_tab(&content)),
        None => Err(mx::Error::Backend(format!(
            "no zone.tab found (tried {ZONE_TAB_PATHS:?}, $TZDIR)"
        ))),
    }
}

pub async fn read_keyboard_layouts() -> mx::Result<Vec<KeyboardLayout>> {
    // Not a standard env var (unlike `TZDIR`) — a dev-only override so
    // `cargo run --fake` can find evdev.xml inside a `nix develop` shell,
    // which has no `/run/current-system` NixOS profile to symlink it under.
    let dev_override = std::env::var("MODULIX_DEV_EVDEV_XML").into_iter();
    let candidates = dev_override.chain(EVDEV_XML_PATHS.iter().map(|s| s.to_string()));

    match read_first_existing(candidates).await {
        Some(content) => Ok(parse_evdev_xml(&content)),
        None => Err(mx::Error::Backend(format!(
            "no evdev.xml found (tried {EVDEV_XML_PATHS:?})"
        ))),
    }
}

pub async fn read_locales() -> mx::Result<Vec<LocaleEntry>> {
    // Same dev-only override rationale as `MODULIX_DEV_EVDEV_XML` above.
    let dev_override = std::env::var("MODULIX_DEV_LOCALE_SUPPORTED").into_iter();
    let candidates = dev_override.chain(SUPPORTED_LOCALES_PATHS.iter().map(|s| s.to_string()));

    if let Some(content) = read_first_existing(candidates).await {
        let entries = parse_supported_locales(&content);
        if !entries.is_empty() {
            return Ok(entries);
        }
    }
    // Fall back to whatever glibc has actually generated on this system —
    // always available, just far less complete.
    let output = tokio::process::Command::new("locale")
        .arg("-a")
        .output()
        .await?;
    let stdout = String::from_utf8(output.stdout)?;
    Ok(stdout
        .lines()
        .map(|line| LocaleEntry {
            code: line.trim().to_string(),
        })
        .collect())
}

/// Parses glibc's `SUPPORTED` file: a `SUPPORTED-LOCALES=\` header, then one
/// `<locale>/<charset> \` per line (note the **slash**, not whitespace —
/// e.g. `fr_FR.UTF-8/UTF-8 \`, `fr_FR/ISO-8859-1 \`), comment lines starting
/// with `#`. Kept to UTF-8 entries — legacy charsets aren't worth offering
/// in a modern installer. Lines without a `/` (the header, comments, blanks)
/// are dropped by the `?` on `split_once`.
pub fn parse_supported_locales(content: &str) -> Vec<LocaleEntry> {
    content
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| {
            let line = line.trim();
            let line = line.strip_suffix('\\').unwrap_or(line).trim();
            let (name, charset) = line.split_once('/')?;
            if charset != "UTF-8" {
                return None;
            }
            Some(LocaleEntry {
                code: name.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_utf8_locales_only() {
        let content = "SUPPORTED-LOCALES=\\\naa_DJ/ISO-8859-1 \\\naa_DJ.UTF-8/UTF-8 \\\nfr_FR.UTF-8/UTF-8 \\\n";
        let entries = parse_supported_locales(content);
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.code == "aa_DJ.UTF-8"));
        assert!(entries.iter().any(|e| e.code == "fr_FR.UTF-8"));
    }

    #[test]
    fn skips_comments_and_blank_lines() {
        let content = "# comment\n\nfr_FR.UTF-8/UTF-8 \\\n";
        assert_eq!(parse_supported_locales(content).len(), 1);
    }

    #[test]
    fn strips_trailing_backslash_continuation() {
        let entries = parse_supported_locales("fr_FR.UTF-8/UTF-8 \\\n");
        assert_eq!(entries[0].code, "fr_FR.UTF-8");
    }

    #[test]
    fn empty_input_yields_no_locales() {
        assert!(parse_supported_locales("").is_empty());
    }
}
