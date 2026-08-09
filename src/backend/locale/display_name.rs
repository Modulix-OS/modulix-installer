//! Turns a glibc locale code (`"fr_BE.UTF-8"`) into something a human picks
//! from a list (`"Français (Belgique)"`), instead of the raw code. Language
//! names are autonyms (each language's name for itself); territory
//! qualifiers use the territory's name in that same language where we have
//! one curated, falling back to English, then to the bare territory code —
//! never fabricated.

/// Splits `"fr_BE.UTF-8"` / `"fr_BE"` / `"sr_RS@latin"` into
/// (language, territory). Charset (`.UTF-8`) and modifier (`@latin`) are
/// dropped; territory is `None` for language-only codes (e.g. `"eo"`).
fn split_locale_code(code: &str) -> (&str, Option<&str>) {
    let without_modifier = code.split('@').next().unwrap_or(code);
    let without_charset = without_modifier
        .split('.')
        .next()
        .unwrap_or(without_modifier);
    match without_charset.split_once('_') {
        Some((lang, territory)) if !territory.is_empty() => (lang, Some(territory)),
        _ => (without_charset, None),
    }
}

/// The bare language portion of a locale code (`"fr_BE.UTF-8"` -> `"fr"`) —
/// used to group codes by language when deciding which ones need a
/// territory qualifier (see [`display_name`]).
pub fn language_code_of(code: &str) -> &str {
    split_locale_code(code).0
}

/// Human-readable name for one locale code. `show_territory` should be true
/// only when this language has more than one territory variant in the list
/// being displayed — a lone `ja_JP` doesn't need "(Japon)" tacked on.
pub fn display_name(code: &str, show_territory: bool) -> String {
    let (lang, territory) = split_locale_code(code);

    if lang.eq_ignore_ascii_case("C") || lang.eq_ignore_ascii_case("POSIX") {
        return "C (POSIX)".to_string();
    }

    let Some(language_name) = language_autonym(lang) else {
        return code.to_string();
    };

    match (show_territory, territory) {
        (true, Some(territory)) => {
            let territory_name = territory_name_in(lang, territory)
                .or_else(|| territory_name_english(territory))
                .map(str::to_string)
                .unwrap_or_else(|| territory.to_string());
            format!("{language_name} ({territory_name})")
        }
        _ => language_name.to_string(),
    }
}

fn language_autonym(lang: &str) -> Option<&'static str> {
    Some(match lang.to_ascii_lowercase().as_str() {
        "af" => "Afrikaans",
        "am" => "አማርኛ",
        "ar" => "العربية",
        "as" => "অসমীয়া",
        "az" => "Azərbaycan dili",
        "be" => "Беларуская",
        "bg" => "Български",
        "bn" => "বাংলা",
        "bo" => "བོད་སྐད་",
        "br" => "Brezhoneg",
        "bs" => "Bosanski",
        "ca" => "Català",
        "cs" => "Čeština",
        "cy" => "Cymraeg",
        "da" => "Dansk",
        "de" => "Deutsch",
        "el" => "Ελληνικά",
        "en" => "English",
        "eo" => "Esperanto",
        "es" => "Español",
        "et" => "Eesti",
        "eu" => "Euskara",
        "fa" => "فارسی",
        "fi" => "Suomi",
        "fil" | "tl" => "Filipino",
        "fo" => "Føroyskt",
        "fr" => "Français",
        "ga" => "Gaeilge",
        "gd" => "Gàidhlig",
        "gl" => "Galego",
        "gu" => "ગુજરાતી",
        "he" => "עברית",
        "hi" => "हिन्दी",
        "hr" => "Hrvatski",
        "hu" => "Magyar",
        "hy" => "Հայերեն",
        "id" => "Bahasa Indonesia",
        "is" => "Íslenska",
        "it" => "Italiano",
        "ja" => "日本語",
        "ka" => "ქართული",
        "kk" => "Қазақ тілі",
        "km" => "ខ្មែរ",
        "kn" => "ಕನ್ನಡ",
        "ko" => "한국어",
        "ku" => "Kurdî",
        "ky" => "Кыргызча",
        "lb" => "Lëtzebuergesch",
        "lo" => "ລາວ",
        "lt" => "Lietuvių",
        "lv" => "Latviešu",
        "mk" => "Македонски",
        "ml" => "മലയാളം",
        "mn" => "Монгол",
        "mr" => "मराठी",
        "ms" => "Bahasa Melayu",
        "mt" => "Malti",
        "my" => "မြန်မာ",
        "nb" | "no" => "Norsk Bokmål",
        "ne" => "नेपाली",
        "nl" => "Nederlands",
        "nn" => "Norsk Nynorsk",
        "oc" => "Occitan",
        "or" => "ଓଡ଼ିଆ",
        "pa" => "ਪੰਜਾਬੀ",
        "pl" => "Polski",
        "ps" => "پښتو",
        "pt" => "Português",
        "ro" => "Română",
        "ru" => "Русский",
        "rw" => "Kinyarwanda",
        "si" => "සිංහල",
        "sk" => "Slovenčina",
        "sl" => "Slovenščina",
        "sq" => "Shqip",
        "sr" => "Српски",
        "sv" => "Svenska",
        "sw" => "Kiswahili",
        "ta" => "தமிழ்",
        "te" => "తెలుగు",
        "tg" => "Тоҷикӣ",
        "th" => "ไทย",
        "ti" => "ትግርኛ",
        "tk" => "Türkmençe",
        "tr" => "Türkçe",
        "tt" => "Татар теле",
        "ug" => "ئۇيغۇرچە",
        "uk" => "Українська",
        "ur" => "اردو",
        "uz" => "O'zbek",
        "vi" => "Tiếng Việt",
        "wa" => "Walon",
        "xh" => "isiXhosa",
        "yi" => "ייִדיש",
        "zh" => "中文",
        "zu" => "isiZulu",
        _ => return None,
    })
}

/// Territory name written in `lang` itself — only curated for the language
/// groups that actually have several glibc territory variants.
fn territory_name_in(lang: &str, territory: &str) -> Option<&'static str> {
    let territory = territory.to_ascii_uppercase();
    Some(
        match (lang.to_ascii_lowercase().as_str(), territory.as_str()) {
            ("en", "US") => "United States",
            ("en", "GB") => "United Kingdom",
            ("en", "CA") => "Canada",
            ("en", "AU") => "Australia",
            ("en", "NZ") => "New Zealand",
            ("en", "IE") => "Ireland",
            ("en", "ZA") => "South Africa",
            ("en", "IN") => "India",
            ("en", "SG") => "Singapore",
            ("en", "PH") => "Philippines",
            ("en", "HK") => "Hong Kong",
            ("en", "BZ") => "Belize",
            ("en", "JM") => "Jamaica",
            ("en", "TT") => "Trinidad and Tobago",
            ("en", "NG") => "Nigeria",
            ("en", "BW") => "Botswana",
            ("en", "ZM") => "Zambia",
            ("en", "ZW") => "Zimbabwe",

            ("fr", "FR") => "France",
            ("fr", "BE") => "Belgique",
            ("fr", "CA") => "Canada",
            ("fr", "CH") => "Suisse",
            ("fr", "LU") => "Luxembourg",
            ("fr", "MC") => "Monaco",
            ("fr", "SN") => "Sénégal",

            ("de", "DE") => "Deutschland",
            ("de", "AT") => "Österreich",
            ("de", "CH") => "Schweiz",
            ("de", "LU") => "Luxemburg",
            ("de", "BE") => "Belgien",
            ("de", "LI") => "Liechtenstein",

            ("es", "ES") => "España",
            ("es", "MX") => "México",
            ("es", "AR") => "Argentina",
            ("es", "CO") => "Colombia",
            ("es", "CL") => "Chile",
            ("es", "PE") => "Perú",
            ("es", "VE") => "Venezuela",
            ("es", "EC") => "Ecuador",
            ("es", "GT") => "Guatemala",
            ("es", "CU") => "Cuba",
            ("es", "BO") => "Bolivia",
            ("es", "DO") => "República Dominicana",
            ("es", "HN") => "Honduras",
            ("es", "PY") => "Paraguay",
            ("es", "SV") => "El Salvador",
            ("es", "NI") => "Nicaragua",
            ("es", "CR") => "Costa Rica",
            ("es", "PA") => "Panamá",
            ("es", "UY") => "Uruguay",
            ("es", "PR") => "Puerto Rico",
            ("es", "US") => "Estados Unidos",

            ("pt", "PT") => "Portugal",
            ("pt", "BR") => "Brasil",

            ("nl", "NL") => "Nederland",
            ("nl", "BE") => "België",

            ("it", "IT") => "Italia",
            ("it", "CH") => "Svizzera",

            ("zh", "CN") => "中国",
            ("zh", "TW") => "台灣",
            ("zh", "HK") => "香港",
            ("zh", "SG") => "新加坡",

            ("ar", "SA") => "السعودية",
            ("ar", "EG") => "مصر",
            ("ar", "AE") => "الإمارات",
            ("ar", "DZ") => "الجزائر",
            ("ar", "MA") => "المغرب",
            ("ar", "TN") => "تونس",
            ("ar", "LY") => "ليبيا",
            ("ar", "JO") => "الأردن",
            ("ar", "LB") => "لبنان",
            ("ar", "SY") => "سوريا",
            ("ar", "IQ") => "العراق",
            ("ar", "KW") => "الكويت",
            ("ar", "QA") => "قطر",
            ("ar", "BH") => "البحرين",
            ("ar", "OM") => "عُمان",
            ("ar", "YE") => "اليمن",
            ("ar", "SD") => "السودان",

            ("sv", "SE") => "Sverige",
            ("sv", "FI") => "Finland",

            _ => return None,
        },
    )
}

/// ISO 3166-1 English short names — universal fallback when we don't have a
/// `territory_name_in` entry for this particular (language, territory) pair.
fn territory_name_english(territory: &str) -> Option<&'static str> {
    let territory = territory.to_ascii_uppercase();
    Some(match territory.as_str() {
        "US" => "United States",
        "GB" => "United Kingdom",
        "CA" => "Canada",
        "AU" => "Australia",
        "NZ" => "New Zealand",
        "IE" => "Ireland",
        "ZA" => "South Africa",
        "IN" => "India",
        "SG" => "Singapore",
        "PH" => "Philippines",
        "HK" => "Hong Kong",
        "MO" => "Macau",
        "TW" => "Taiwan",
        "CN" => "China",
        "JP" => "Japan",
        "KR" => "South Korea",
        "KP" => "North Korea",
        "VN" => "Vietnam",
        "TH" => "Thailand",
        "MY" => "Malaysia",
        "ID" => "Indonesia",
        "PK" => "Pakistan",
        "BD" => "Bangladesh",
        "LK" => "Sri Lanka",
        "NP" => "Nepal",
        "AF" => "Afghanistan",
        "IR" => "Iran",
        "IQ" => "Iraq",
        "IL" => "Israel",
        "TR" => "Turkey",
        "SA" => "Saudi Arabia",
        "AE" => "United Arab Emirates",
        "EG" => "Egypt",
        "MA" => "Morocco",
        "DZ" => "Algeria",
        "TN" => "Tunisia",
        "LY" => "Libya",
        "NG" => "Nigeria",
        "KE" => "Kenya",
        "ET" => "Ethiopia",
        "GH" => "Ghana",
        "TZ" => "Tanzania",
        "UG" => "Uganda",
        "RW" => "Rwanda",
        "SN" => "Senegal",
        "CI" => "Ivory Coast",
        "CM" => "Cameroon",
        "CD" => "DR Congo",
        "MZ" => "Mozambique",
        "AO" => "Angola",
        "ZM" => "Zambia",
        "ZW" => "Zimbabwe",
        "BW" => "Botswana",
        "NA" => "Namibia",
        "FR" => "France",
        "DE" => "Germany",
        "IT" => "Italy",
        "ES" => "Spain",
        "PT" => "Portugal",
        "NL" => "Netherlands",
        "BE" => "Belgium",
        "CH" => "Switzerland",
        "AT" => "Austria",
        "LU" => "Luxembourg",
        "LI" => "Liechtenstein",
        "SE" => "Sweden",
        "NO" => "Norway",
        "DK" => "Denmark",
        "FI" => "Finland",
        "IS" => "Iceland",
        "PL" => "Poland",
        "CZ" => "Czechia",
        "SK" => "Slovakia",
        "HU" => "Hungary",
        "RO" => "Romania",
        "BG" => "Bulgaria",
        "GR" => "Greece",
        "HR" => "Croatia",
        "SI" => "Slovenia",
        "RS" => "Serbia",
        "BA" => "Bosnia and Herzegovina",
        "MK" => "North Macedonia",
        "AL" => "Albania",
        "ME" => "Montenegro",
        "UA" => "Ukraine",
        "BY" => "Belarus",
        "RU" => "Russia",
        "MD" => "Moldova",
        "LT" => "Lithuania",
        "LV" => "Latvia",
        "EE" => "Estonia",
        "GE" => "Georgia",
        "AM" => "Armenia",
        "AZ" => "Azerbaijan",
        "KZ" => "Kazakhstan",
        "UZ" => "Uzbekistan",
        "KG" => "Kyrgyzstan",
        "TJ" => "Tajikistan",
        "TM" => "Turkmenistan",
        "MN" => "Mongolia",
        "MX" => "Mexico",
        "BR" => "Brazil",
        "AR" => "Argentina",
        "CO" => "Colombia",
        "CL" => "Chile",
        "PE" => "Peru",
        "VE" => "Venezuela",
        "EC" => "Ecuador",
        "GT" => "Guatemala",
        "CU" => "Cuba",
        "BO" => "Bolivia",
        "DO" => "Dominican Republic",
        "HN" => "Honduras",
        "PY" => "Paraguay",
        "SV" => "El Salvador",
        "NI" => "Nicaragua",
        "CR" => "Costa Rica",
        "PA" => "Panama",
        "UY" => "Uruguay",
        "PR" => "Puerto Rico",
        "JM" => "Jamaica",
        "TT" => "Trinidad and Tobago",
        "BZ" => "Belize",
        "MC" => "Monaco",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_language_territory_charset() {
        assert_eq!(split_locale_code("fr_BE.UTF-8"), ("fr", Some("BE")));
    }

    #[test]
    fn splits_language_only() {
        assert_eq!(split_locale_code("eo.UTF-8"), ("eo", None));
    }

    #[test]
    fn strips_modifier() {
        assert_eq!(split_locale_code("sr_RS@latin"), ("sr", Some("RS")));
    }

    #[test]
    fn hides_territory_when_not_requested() {
        assert_eq!(display_name("fr_FR.UTF-8", false), "Français");
    }

    #[test]
    fn shows_curated_territory_name() {
        assert_eq!(display_name("fr_BE.UTF-8", true), "Français (Belgique)");
    }

    #[test]
    fn falls_back_to_english_territory_name() {
        assert_eq!(display_name("en_ZA.UTF-8", true), "English (South Africa)");
    }

    #[test]
    fn falls_back_to_raw_code_for_unknown_language() {
        assert_eq!(display_name("xx_YY.UTF-8", true), "xx_YY.UTF-8");
    }

    #[test]
    fn c_locale_is_labeled() {
        assert_eq!(display_name("C", false), "C (POSIX)");
    }
}
