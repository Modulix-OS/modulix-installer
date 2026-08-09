//! gettext wiring. `set_language` can be called again after startup (language
//! step, step 2) — every already-built [`crate::steps::Step::retranslate`] must
//! be called afterwards for the change to become visible.

use gettextrs::{
    LocaleCategory, bind_textdomain_codeset, bindtextdomain, gettext, setlocale, textdomain,
};

const DOMAIN: &str = "modulixos-installer";

/// Same debug/release split as modulix-core-utils' `CONFIG_DIRECTORY` (see
/// CLAUDE.md): debug builds bind to the `.mo` files `build.rs` compiles from
/// `po/*.po` into `$OUT_DIR/locale`, so `cargo run` works without installing
/// anything; release builds use the real system locale dir.
#[cfg(debug_assertions)]
const LOCALE_DIR: &str = env!("MODULIX_LOCALE_DIR");
#[cfg(not(debug_assertions))]
const LOCALE_DIR: &str = "/usr/share/locale";

pub fn init() {
    // SAFETY: called once at startup on the main thread, before any other
    // thread is spawned; no concurrent access to the C locale globals.
    unsafe {
        setlocale(LocaleCategory::LcAll, "");
    }
    let _ = bindtextdomain(DOMAIN, LOCALE_DIR);
    let _ = bind_textdomain_codeset(DOMAIN, "UTF-8");
    let _ = textdomain(DOMAIN);
}

/// Switches the process locale. Missing catalogs fall back to the source
/// `msgid` strings — safe to call with a locale that has no `.mo` yet.
pub fn set_language(locale: &str) {
    // SAFETY: only ever called from the GTK main thread (language step
    // callback); no other thread reads/writes the C locale concurrently.
    unsafe {
        setlocale(LocaleCategory::LcAll, locale);
    }
}

pub fn tr(msgid: &str) -> String {
    gettext(msgid)
}
