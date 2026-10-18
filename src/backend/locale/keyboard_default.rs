//! Maps a glibc locale onto the xkb layout a speaker of that locale expects,
//! so picking a language in step 2 pre-selects a usable keyboard in step 4
//! instead of leaving whatever `evdev.xml` happened to list first.
//!
//! There is no authoritative machine-readable mapping for this: `evdev.xml`'s
//! `<languageList>` is many-to-many (five layouts claim `fra`), so GNOME and
//! Calamares both carry a curated table. This module is the same idea, kept as
//! small as possible: a handful of exceptions, then two mechanical rules.

use super::KeyboardLayout;
use super::display_name::split_locale_code;

/// Last resort when nothing matches — and the layout the ISO boots with.
const FALLBACK_LAYOUT: &str = "us";

/// Layouts whose expected default is *not* their bare form, keyed by layout
/// code. `fr` is the case this exists for: French installs default to
/// `fr(oss)` — "French (alt.)" in `evdev.xml`, "Français (variante)" once
/// localized — which is the azerty map French users expect, the bare `fr`
/// one being the legacy map missing the extra dead keys and `ŒœÆæ`.
const DEFAULT_VARIANTS: &[(&str, &str)] = &[("fr", "oss")];

/// Locales whose layout is *not* what the mechanical rules below would pick.
/// Matched on `language_territory`, or on `language` alone when the territory
/// is absent or unlisted (the `""` territory entries).
const OVERRIDES: &[(&str, &str, &str)] = &[
    // The territory rule would send every anglophone country to its own
    // layout: `ca` is French (Canada), `au`/`nz`/`za` do not exist as layouts.
    ("en", "GB", "gb"),
    ("en", "IE", "ie"),
    ("en", "", "us"),
    // CJK layouts are named after the country, but the territory rule only
    // works when the locale carries one — `zh`, `ja`, `ko` often do not.
    ("zh", "TW", "tw"),
    ("zh", "HK", "tw"),
    ("zh", "", "cn"),
    ("ja", "", "jp"),
    ("ko", "", "kr"),
    // Portuguese: `pt` is the layout for Portugal, `br` for Brazil.
    ("pt", "BR", "br"),
    ("pt", "", "pt"),
    // Spanish outside Spain is one shared Latin-American layout.
    ("es", "ES", "es"),
    ("es", "", "latam"),
    // Languages whose layout is named after the script's country rather than
    // the language, where the locale's own territory would also work but is
    // frequently missing.
    ("he", "", "il"),
    ("el", "", "gr"),
    ("uk", "", "ua"),
    ("cs", "", "cz"),
    ("da", "", "dk"),
    ("sv", "", "se"),
    ("nb", "", "no"),
    ("nn", "", "no"),
    ("et", "", "ee"),
];

/// The variant a layout should start on when nothing has been picked by hand.
///
/// * `layout` - the layout about to become the selected one.
///
/// # Post-conditions
/// Returns a variant code present in `layout.variants`, or `""` (the "Default"
/// sentinel of the variant dropdown) when [`DEFAULT_VARIANTS`] lists none for
/// this layout or this `evdev.xml` does not offer the listed one.
pub fn default_variant_for_layout(layout: &KeyboardLayout) -> &'static str {
    DEFAULT_VARIANTS
        .iter()
        .find(|(code, _)| *code == layout.code)
        .map(|(_, variant)| *variant)
        .filter(|variant| layout.variants.iter().any(|v| v.code == *variant))
        .unwrap_or("")
}

fn has_layout(layouts: &[KeyboardLayout], code: &str) -> bool {
    layouts.iter().any(|layout| layout.code == code)
}

/// Picks the xkb layout that best matches a glibc locale.
///
/// * `locale` - locale code as listed by the language step, e.g.
///   `"fr_BE.UTF-8"`. The charset and modifier are ignored.
/// * `layouts` - layouts actually offered by this machine's `evdev.xml`; a
///   candidate absent from it is never returned.
///
/// Resolution order: curated [`OVERRIDES`], then the territory lowercased
/// (`fr_FR` -> `fr`, `ru_RU` -> `ru`), then the language code itself
/// (`ar` -> `ar`), then [`FALLBACK_LAYOUT`].
///
/// # Post-conditions
/// The returned code is always present in `layouts`, or `None` when even
/// [`FALLBACK_LAYOUT`] is missing (an `evdev.xml` that failed to parse).
pub fn layout_for_locale(locale: &str, layouts: &[KeyboardLayout]) -> Option<String> {
    let (lang, territory, _modifier) = split_locale_code(locale);
    let lang = lang.to_ascii_lowercase();
    let territory = territory.map(str::to_ascii_uppercase);

    let override_match = OVERRIDES
        .iter()
        .find(|(ov_lang, ov_territory, _)| {
            *ov_lang == lang
                && (*ov_territory == territory.as_deref().unwrap_or("") || ov_territory.is_empty())
        })
        .map(|(_, _, layout)| *layout);

    let territory_layout = territory.as_deref().map(str::to_ascii_lowercase);
    let candidates = [
        override_match,
        territory_layout.as_deref(),
        Some(lang.as_str()),
        Some(FALLBACK_LAYOUT),
    ];

    candidates
        .into_iter()
        .flatten()
        .find(|code| has_layout(layouts, code))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layouts() -> Vec<KeyboardLayout> {
        [
            "us", "gb", "fr", "ca", "be", "de", "ch", "es", "latam", "br", "pt", "ru", "jp", "kr",
            "cn", "tw", "il", "ar",
        ]
        .into_iter()
        .map(|code| KeyboardLayout {
            code: code.to_string(),
            description: code.to_string(),
            variants: Vec::new(),
        })
        .collect()
    }

    #[test]
    fn territory_wins_for_the_common_case() {
        let layouts = layouts();
        assert_eq!(layout_for_locale("fr_FR.UTF-8", &layouts).unwrap(), "fr");
        assert_eq!(layout_for_locale("fr_BE.UTF-8", &layouts).unwrap(), "be");
        assert_eq!(layout_for_locale("de_CH.UTF-8", &layouts).unwrap(), "ch");
        assert_eq!(layout_for_locale("ru_RU.UTF-8", &layouts).unwrap(), "ru");
    }

    #[test]
    fn english_never_falls_into_a_country_layout() {
        let layouts = layouts();
        assert_eq!(layout_for_locale("en_US.UTF-8", &layouts).unwrap(), "us");
        assert_eq!(layout_for_locale("en_CA.UTF-8", &layouts).unwrap(), "us");
        assert_eq!(layout_for_locale("en_GB.UTF-8", &layouts).unwrap(), "gb");
    }

    #[test]
    fn overrides_cover_the_named_exceptions() {
        let layouts = layouts();
        assert_eq!(layout_for_locale("pt_BR.UTF-8", &layouts).unwrap(), "br");
        assert_eq!(layout_for_locale("pt_PT.UTF-8", &layouts).unwrap(), "pt");
        assert_eq!(layout_for_locale("es_AR.UTF-8", &layouts).unwrap(), "latam");
        assert_eq!(layout_for_locale("es_ES.UTF-8", &layouts).unwrap(), "es");
        assert_eq!(layout_for_locale("ja_JP.UTF-8", &layouts).unwrap(), "jp");
        assert_eq!(layout_for_locale("zh_TW.UTF-8", &layouts).unwrap(), "tw");
        assert_eq!(layout_for_locale("zh_CN.UTF-8", &layouts).unwrap(), "cn");
        assert_eq!(layout_for_locale("he_IL.UTF-8", &layouts).unwrap(), "il");
    }

    #[test]
    fn language_code_is_tried_before_giving_up() {
        let layouts = layouts();
        assert_eq!(layout_for_locale("ar_EG.UTF-8", &layouts).unwrap(), "ar");
    }

    #[test]
    fn unknown_locale_falls_back_to_us() {
        let layouts = layouts();
        assert_eq!(layout_for_locale("xx_YY.UTF-8", &layouts).unwrap(), "us");
        assert_eq!(layout_for_locale("", &layouts).unwrap(), "us");
    }

    #[test]
    fn empty_catalog_yields_nothing() {
        assert_eq!(layout_for_locale("fr_FR.UTF-8", &[]), None);
    }

    fn layout_with_variants(code: &str, variants: &[&str]) -> KeyboardLayout {
        KeyboardLayout {
            code: code.to_string(),
            description: code.to_string(),
            variants: variants
                .iter()
                .map(|variant| super::super::KeyboardVariant {
                    code: variant.to_string(),
                    description: variant.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn french_defaults_to_the_oss_variant() {
        let layout = layout_with_variants("fr", &["nodeadkeys", "oss", "bepo"]);
        assert_eq!(default_variant_for_layout(&layout), "oss");
    }

    #[test]
    fn a_layout_without_a_listed_default_starts_bare() {
        let layout = layout_with_variants("de", &["nodeadkeys"]);
        assert_eq!(default_variant_for_layout(&layout), "");
    }

    #[test]
    fn a_missing_variant_is_not_forced() {
        let layout = layout_with_variants("fr", &["bepo"]);
        assert_eq!(default_variant_for_layout(&layout), "");
    }
}
