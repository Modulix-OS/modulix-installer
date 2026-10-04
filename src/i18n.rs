//! gettext wiring. `set_language` can be called again after startup (language
//! step, step 2) — every already-built [`crate::steps::Step::retranslate`] must
//! be called afterwards for the change to become visible.
//!
//! Message catalog selection is driven by `LANGUAGE`, not `setlocale`: glibc
//! only honours `setlocale(LC_ALL, code)` for a locale that has been
//! *generated* on the running machine, and installer users routinely pick a
//! language the live ISO never generated — that call silently no-ops
//! (returns `NULL`, leaves the locale untouched) instead of erroring.
//! `LANGUAGE` needs no generated locale: gettext resolves it as a directory
//! name under the bound text domain and falls back to the `msgid` when no
//! catalog matches. `setlocale` is still used, best-effort, for
//! *formatting* (dates, numbers, collation).
//!
//! Two glibc quirks this module works around:
//! - Changing `LANGUAGE` alone doesn't retranslate anything already looked
//!   up: glibc caches translations behind the exported `_nl_msg_cat_cntr`
//!   counter, which must be bumped by hand after every `LANGUAGE` change —
//!   this is the documented GNU gettext recipe for runtime language
//!   switching, not a hack.
//! - `LANGUAGE` is ignored outright when `LC_MESSAGES` is `C`/`POSIX`
//!   (`C.UTF-8` included), so [`init`] must land `LC_MESSAGES` on some real
//!   generated locale first, or every later [`set_language`] call is a
//!   silent no-op for messages too.
//!
//! The startup language is [`DEFAULT_LANGUAGE`] (English) and never the host
//! locale — see that constant for why.
//!
//! SAFETY-critical invariant for the whole module: [`init`] seeds the
//! `LANGUAGE` slot in `environ` exactly once, before `main.rs` starts the
//! tokio runtime — so later [`set_language`] calls only swap the pointer of
//! an *existing* `environ` entry (no array growth/reallocation), and only
//! the GTK main thread ever reads it (via [`tr`]/gettext) or writes it (via
//! [`set_language`], only ever called from the language-step UI callback).

use gettextrs::{
    LocaleCategory, bind_textdomain_codeset, bindtextdomain, dgettext, gettext, setlocale,
    textdomain,
};
use std::cell::RefCell;

use crate::backend::locale::display_name::split_locale_code;

const DOMAIN: &str = "modulixos-installer";

/// xkeyboard-config's own gettext domain. `evdev.xml` carries English
/// `<description>` text only — no `xml:lang` variants — and xkeyboard-config
/// ships the translations as catalogs whose msgids are those very strings
/// (same trick GNOME's input-source list uses). See [`tr_xkb`].
const XKB_DOMAIN: &str = "xkeyboard-config";

/// Same debug/release split as modulix-core-utils' `CONFIG_DIRECTORY`, but
/// runtime-overridable: `build.rs` bakes in a default (`$OUT_DIR/locale` for
/// `cargo run` in debug, `/usr/share/locale` in release) via
/// `MODULIX_LOCALE_DIR_DEFAULT`, and `MODULIX_LOCALE_DIR` — set by the Nix
/// package's `wrapProgram` to `$out/share/locale` — overrides it at startup.
/// This lets the derivation point the release binary at the store path
/// without `substituteInPlace`-patching a Rust constant.
fn locale_dir() -> String {
    std::env::var("MODULIX_LOCALE_DIR")
        .unwrap_or_else(|_| env!("MODULIX_LOCALE_DIR_DEFAULT").to_string())
}

/// Where xkeyboard-config's message catalogs live, tried in order:
/// `MODULIX_XKB_LOCALE_DIR` (set by `nix/package.nix` and the devShell, the
/// only reliable answer on NixOS since the package need not be in
/// `systemPackages`), then the `share/locale` sibling of the `evdev.xml` path
/// we were already given, then the usual system prefixes.
///
/// # Post-conditions
/// Returns the first existing directory, or `None` — in which case layout
/// names simply stay English instead of the binding failing loudly.
fn xkb_locale_dir() -> Option<String> {
    if let Ok(dir) = std::env::var("MODULIX_XKB_LOCALE_DIR")
        && std::path::Path::new(&dir).is_dir()
    {
        return Some(dir);
    }
    if let Ok(xml) = std::env::var("MODULIX_DEV_EVDEV_XML") {
        // <prefix>/share/X11/xkb/rules/evdev.xml -> <prefix>/share/locale
        let sibling = std::path::Path::new(&xml)
            .ancestors()
            .nth(4)
            .map(|prefix| prefix.join("locale"));
        if let Some(dir) = sibling.filter(|dir| dir.is_dir()) {
            return dir.to_str().map(str::to_string);
        }
    }
    ["/run/current-system/sw/share/locale", "/usr/share/locale"]
        .into_iter()
        .find(|dir| std::path::Path::new(dir).is_dir())
        .map(str::to_string)
}

/// Message language the installer always starts in, whatever the host
/// environment says. The live ISO runs the kiosk session as root with no
/// usable `$LANG`, and on a dev machine the host locale would otherwise pick
/// both the startup language and the language step's pre-selection; pinning
/// it keeps the first screens deterministic. The user changes it at step 2.
///
/// Kept a *full* locale code on purpose: the language step matches it against
/// `list_locales()` entries (all of which carry a territory) to pre-select the
/// dropdown, and a bare `"en"` would match none of them.
pub const DEFAULT_LANGUAGE: &str = "en_US.UTF-8";

/// Locales tried, in order, for *formatting* when none of
/// [`DEFAULT_LANGUAGE`]'s own spellings is generated on this machine. Their
/// only job is to get `LC_MESSAGES` off `C`/`POSIX`, which `LANGUAGE` is
/// ignored under; the message language never depends on this list. The last
/// entry is the host environment — a locale generated under a spelling we
/// can't guess still beats staying on `C`.
const FALLBACK_LOCALES: &[&str] = &["en_GB.UTF-8", "en_GB.utf8", ""];

thread_local! {
    static CURRENT_LANGUAGE: RefCell<String> = const { RefCell::new(String::new()) };
}

unsafe extern "C" {
    static mut _nl_msg_cat_cntr: core::ffi::c_int;
}

/// Outcome of [`set_language`]: messages always switch (see module docs);
/// formatting (`setlocale`) only switches when the target locale is
/// generated on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageOutcome {
    /// Both messages (`LANGUAGE`) and formatting (`setlocale`) landed on the requested locale.
    Applied,
    /// Messages switched; `setlocale` couldn't land on this locale (not generated on this
    /// machine), so date/number formatting stays on whatever it was before.
    MessagesOnly,
}

fn is_c_or_posix(locale: &str) -> bool {
    let base = locale.split('.').next().unwrap_or(locale);
    base.is_empty() || base.eq_ignore_ascii_case("C") || base.eq_ignore_ascii_case("POSIX")
}

fn setlocale_lcall(candidate: &str) -> Option<String> {
    // SAFETY: see module-level invariant — called only from `init` (before
    // the tokio runtime starts) and from `set_language` (GTK main thread
    // only).
    let bytes = unsafe { setlocale(LocaleCategory::LcAll, candidate) }?;
    String::from_utf8(bytes).ok()
}

/// The encoding suffix of a locale code (`"fr_BE.UTF-8"` -> `Some("UTF-8")`),
/// ignoring any modifier — orthogonal to [`split_locale_code`]'s
/// language/territory/modifier split, so it isn't "a second parser" of that
/// same identity, just the one extra piece `setlocale_candidates` needs.
fn charset_of(code: &str) -> Option<&str> {
    let without_modifier = code.split('@').next().unwrap_or(code);
    without_modifier.split_once('.').map(|(_, charset)| charset)
}

fn format_locale(
    lang: &str,
    territory: Option<&str>,
    charset: Option<&str>,
    modifier: Option<&str>,
) -> String {
    let mut out = lang.to_string();
    if let Some(territory) = territory {
        out.push('_');
        out.push_str(territory);
    }
    if let Some(charset) = charset {
        out.push('.');
        out.push_str(charset);
    }
    if let Some(modifier) = modifier {
        out.push('@');
        out.push_str(modifier);
    }
    out
}

/// Candidate spellings tried, in order, when applying `code` via
/// `setlocale`: the exact code, glibc's lowercase-no-dash charset spelling,
/// the code with charset dropped, the bare language with charset, and the
/// bare language alone. Every fallback is a strictly *looser* request than
/// the last, since real locales aren't guaranteed to be generated under any
/// one particular spelling.
pub(crate) fn setlocale_candidates(code: &str) -> Vec<String> {
    let (lang, territory, modifier) = split_locale_code(code);
    let charset = charset_of(code);
    let is_utf8 =
        charset.is_some_and(|c| c.eq_ignore_ascii_case("utf-8") || c.eq_ignore_ascii_case("utf8"));

    let mut candidates = vec![code.to_string()];
    if is_utf8 {
        candidates.push(format_locale(lang, territory, Some("utf8"), modifier));
    }
    candidates.push(format_locale(lang, territory, None, modifier));
    if let Some(charset) = charset {
        candidates.push(format_locale(lang, None, Some(charset), None));
    }
    candidates.push(format_locale(lang, None, None, None));
    candidates.dedup();
    candidates
}

/// Value to assign to `LANGUAGE` for `code`: the full code (charset
/// stripped, modifier kept), then progressively less specific fallbacks,
/// colon-separated per the `LANGUAGE` syntax gettext expects.
pub(crate) fn language_env_value(code: &str) -> String {
    let (lang, territory, modifier) = split_locale_code(code);
    let mut parts = Vec::new();
    if modifier.is_some() {
        parts.push(format_locale(lang, territory, None, modifier));
    }
    if territory.is_some() {
        parts.push(format_locale(lang, territory, None, None));
    }
    parts.push(lang.to_string());
    parts.dedup();
    parts.join(":")
}

/// True when `a` and `b` name the same locale once charset is stripped and
/// language/territory/modifier are case-folded — `"fr_FR.utf8"` and
/// `"fr_FR.UTF-8"` are the same locale under two spellings.
pub(crate) fn normalized_eq(a: &str, b: &str) -> bool {
    let (a_lang, a_territory, a_modifier) = split_locale_code(a);
    let (b_lang, b_territory, b_modifier) = split_locale_code(b);
    a_lang.eq_ignore_ascii_case(b_lang)
        && opt_eq_ignore_ascii_case(a_territory, b_territory)
        && opt_eq_ignore_ascii_case(a_modifier, b_modifier)
}

fn opt_eq_ignore_ascii_case(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        (None, None) => true,
        _ => false,
    }
}

fn set_current_language(code: &str) {
    CURRENT_LANGUAGE.with_borrow_mut(|current| *current = code.to_string());
}

/// Currently active message-language code — always reflects the last
/// [`init`]/[`set_language`] call, regardless of whether `setlocale` managed
/// to follow along (see [`LanguageOutcome`]).
pub fn current_language_code() -> String {
    CURRENT_LANGUAGE.with_borrow(|current| current.clone())
}

fn set_language_env(value: &str) {
    // SAFETY: see module-level invariant — `LANGUAGE` already exists in
    // `environ` after the first call from `init`, so this only ever swaps
    // that entry's value pointer; only the GTK main thread calls this.
    unsafe { std::env::set_var("LANGUAGE", value) };
    // SAFETY: see module-level invariant — single write, GTK main thread
    // only, matches the documented GNU gettext runtime-language-switch
    // recipe.
    unsafe { _nl_msg_cat_cntr += 1 };
}

/// Binds the message catalogs and seeds the startup language.
///
/// # Post-conditions
/// The message language is [`DEFAULT_LANGUAGE`] — the host locale is never
/// consulted for it, so the installer always opens in English and the
/// language step comes up pre-selected on it. `setlocale` is applied
/// best-effort, for formatting only; if nothing generated on this machine
/// resolves, `LC_*` stays on `C` and a warning goes to stderr (under the
/// kiosk: to the journal).
pub fn init() {
    // Formatting only — but it has to happen before the `LANGUAGE` seed
    // below: glibc ignores `LANGUAGE` entirely while `LC_MESSAGES` is
    // `C`/`POSIX`, which would make every later `set_language` call a silent
    // no-op for messages too (see module docs).
    let applied = setlocale_candidates(DEFAULT_LANGUAGE)
        .iter()
        .map(String::as_str)
        .chain(FALLBACK_LOCALES.iter().copied())
        .find_map(|candidate| setlocale_lcall(candidate).filter(|l| !is_c_or_posix(l)));

    let _ = bindtextdomain(DOMAIN, locale_dir());
    let _ = bind_textdomain_codeset(DOMAIN, "UTF-8");
    let _ = textdomain(DOMAIN);

    if let Some(dir) = xkb_locale_dir() {
        let _ = bindtextdomain(XKB_DOMAIN, dir);
        let _ = bind_textdomain_codeset(XKB_DOMAIN, "UTF-8");
    }

    if applied.is_none() {
        eprintln!(
            "i18n: no generated non-C locale found ($LANG={:?}); LC_* formatting stays C, \
             and so will the messages — every later language switch is a no-op",
            std::env::var("LANG").unwrap_or_default()
        );
    }

    set_language_env(&language_env_value(DEFAULT_LANGUAGE));
    set_current_language(DEFAULT_LANGUAGE);
}

/// Switches the message language. Always changes what [`tr`] returns
/// (`LANGUAGE`-driven — see module docs); best-effort switches `setlocale`
/// for formatting too. Missing catalogs fall back to the source `msgid`
/// strings, so it's safe to call with a language that has no `.mo` yet.
pub fn set_language(code: &str) -> LanguageOutcome {
    set_language_env(&language_env_value(code));
    set_current_language(code);

    let applied = setlocale_candidates(code)
        .iter()
        .find_map(|candidate| setlocale_lcall(candidate));

    if applied.is_some() {
        LanguageOutcome::Applied
    } else {
        LanguageOutcome::MessagesOnly
    }
}

pub fn tr(msgid: &str) -> String {
    gettext(msgid)
}

/// Translates a keyboard layout/variant description through xkeyboard-config's
/// own catalogs.
///
/// * `msgid` - the English `<description>` read from `evdev.xml`, verbatim.
///
/// # Post-conditions
/// Returns the localized name, or `msgid` unchanged when no catalog covers it
/// (missing translation, or no catalog directory found at [`init`] time).
/// Follows `LANGUAGE` exactly like [`tr`]: the `_nl_msg_cat_cntr` bump in
/// [`set_language`] invalidates glibc's cache for every bound domain, not just
/// ours.
pub fn tr_xkb(msgid: &str) -> String {
    dgettext(XKB_DOMAIN, msgid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_env_value_builds_cascade() {
        assert_eq!(language_env_value("fr_BE.UTF-8"), "fr_BE:fr");
        assert_eq!(
            language_env_value("sr_RS.UTF-8@latin"),
            "sr_RS@latin:sr_RS:sr"
        );
        assert_eq!(language_env_value("eo"), "eo");
    }

    #[test]
    fn default_language_is_a_full_english_locale() {
        // A bare "en" would match none of `list_locales()`'s territory-carrying
        // codes, leaving the language step's dropdown on whatever sorts first.
        assert_eq!(language_env_value(DEFAULT_LANGUAGE), "en_US:en");
        assert!(normalized_eq(DEFAULT_LANGUAGE, "en_US.utf8"));
    }

    #[test]
    fn setlocale_candidates_cascade_from_specific_to_generic() {
        assert_eq!(
            setlocale_candidates("fr_BE.UTF-8"),
            vec!["fr_BE.UTF-8", "fr_BE.utf8", "fr_BE", "fr.UTF-8", "fr"]
        );
    }

    #[test]
    fn setlocale_candidates_dedup_language_only_code() {
        assert_eq!(setlocale_candidates("eo"), vec!["eo"]);
    }

    #[test]
    fn normalized_eq_ignores_charset_and_case() {
        assert!(normalized_eq("fr_FR.utf8", "fr_FR.UTF-8"));
        assert!(!normalized_eq("fr_FR", "fr_BE"));
    }
}
