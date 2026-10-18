//! Keeps the gettext catalogs in sync with the source tree.
//!
//! There is no extraction script in this repo (see `CLAUDE.md`): a new
//! `tr("…")` msgid has to be added to `po/modulixos-installer.pot` *and* to
//! every `po/<lang>.po` by hand. These tests are the guard rail for that —
//! a forgotten msgid silently falls back to English at runtime, which is how
//! ten of them went unnoticed.

use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Languages shipped in `po/`, i.e. the catalogs `build.rs` compiles.
const LANGUAGES: [&str; 3] = ["fr", "es", "de"];

/// Absolute path of a repository-relative path.
///
/// * `rel` - path relative to the crate root.
fn repo_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Every `.rs` file under `src/`, recursively.
///
/// # Post-conditions
/// The returned paths all exist and the list is never empty.
fn rust_sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read src/ directory") {
            let path = entry.expect("read src/ directory entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo_path("src"), &mut out);
    assert!(!out.is_empty(), "no Rust source found under src/");
    out
}

/// Turns the escaped body of a Rust or po string literal into the string
/// gettext actually sees at runtime.
///
/// * `raw` - literal body, without its surrounding quotes.
///
/// # Returns
/// `raw` with `\n`, `\t`, `\r`, `\"`, `\\` resolved, and with Rust's
/// backslash-newline line continuation (backslash, newline, leading
/// whitespace of the next line) removed. Both sides of every comparison go
/// through this, so entries like `…run \"chkdsk\"` or the multi-line
/// password hint match instead of differing by their escaping.
fn unescape(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '\\' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        match chars.get(i + 1) {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            // Rust line continuation: swallow the newline and the next
            // line's indentation.
            Some('\n') => {
                i += 2;
                while chars.get(i).is_some_and(|c| c.is_whitespace()) {
                    i += 1;
                }
                continue;
            }
            Some(other) => {
                out.push('\\');
                out.push(*other);
            }
            None => out.push('\\'),
        }
        i += 2;
    }
    out
}

/// Every msgid passed to `i18n::tr` as a string literal anywhere in `src/`.
///
/// # Post-conditions
/// Excludes `tr_xkb` (that domain is xkeyboard-config's, not ours) and any
/// identifier merely *ending* in `tr`, such as `push_str("ss")`. Only literal
/// arguments are seen: msgids reaching `tr` through a constant (`Task::label`,
/// `widgets::slideshow`, the desktop descriptions in `config.rs`) are not —
/// which is why the pot is a superset, never an equal.
fn source_msgids() -> BTreeSet<String> {
    let call = Regex::new(r#"(?s)tr\(\s*"((?:[^"\\]|\\.)*)""#).expect("valid regex");
    let mut ids = BTreeSet::new();
    for path in rust_sources() {
        let text = fs::read_to_string(&path).expect("read Rust source");
        for m in call.captures_iter(&text) {
            let whole = m.get(0).expect("whole match");
            // `regex` has no lookbehind: reject `…_tr("…")`-style matches by
            // inspecting the byte before the match.
            let prev = text[..whole.start()].chars().next_back();
            if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            ids.insert(unescape(&m[1]));
        }
    }
    assert!(!ids.is_empty(), "no tr(\"…\") literal found in src/");
    ids
}

/// Strips the surrounding quotes of one po string-literal fragment.
///
/// * `line` - a trimmed `"…"` fragment, possibly ending in an escaped quote.
///
/// # Returns
/// The body between the outermost quotes (exactly one on each side, so a
/// trailing `\\"` keeps its escaped quote), or the line unchanged when it is
/// not quoted.
fn unquote(line: &str) -> &str {
    line.strip_prefix('"')
        .and_then(|l| l.strip_suffix('"'))
        .unwrap_or(line)
}

/// Parses a `.po`/`.pot` file into its msgid → msgstr map.
///
/// * `rel` - repository-relative path of the catalog.
///
/// # Post-conditions
/// Continuation lines (`"…"` following a `msgid`/`msgstr`) are concatenated,
/// so a wrapped msgid compares equal to the one-line literal it came from.
/// Obsolete (`#~`) and commented entries are skipped, as is the header
/// (empty msgid).
fn catalog(rel: &str) -> BTreeMap<String, String> {
    let text = fs::read_to_string(repo_path(rel)).expect("read catalog");
    let mut entries = BTreeMap::new();
    let (mut id, mut str_) = (String::new(), String::new());
    // 0 = outside, 1 = inside msgid, 2 = inside msgstr
    let mut field = 0u8;
    let mut flush = |id: &mut String, str_: &mut String| {
        if !id.is_empty() {
            entries.insert(std::mem::take(id), std::mem::take(str_));
        } else {
            id.clear();
            str_.clear();
        }
    };
    for line in text.lines() {
        let line = line.trim_end();
        if let Some(rest) = line.strip_prefix("msgid ") {
            flush(&mut id, &mut str_);
            id = unescape(unquote(rest.trim()));
            field = 1;
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            str_ = unescape(unquote(rest.trim()));
            field = 2;
        } else if line.starts_with('"') && field != 0 {
            let part = unescape(unquote(line.trim()));
            if field == 1 {
                id.push_str(&part);
            } else {
                str_.push_str(&part);
            }
        } else {
            flush(&mut id, &mut str_);
            field = 0;
        }
    }
    flush(&mut id, &mut str_);
    assert!(!entries.is_empty(), "{rel} holds no entry");
    entries
}

/// Renders a missing-msgid list into an assertion failure message.
///
/// * `what` - clause describing what is wrong with these msgids, inserted
///   right after their count.
/// * `missing` - the offending msgids, one per line in the output.
///
/// # Returns
/// A multi-line message ending in the reminder that both the `.pot` and
/// every `po/<lang>.po` have to be edited by hand.
fn report(what: &str, missing: &BTreeSet<&String>) -> String {
    let list = missing
        .iter()
        .map(|m| format!("  - {m:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{} msgid(s) {what}:\n{list}\n\
         Add each one to po/modulixos-installer.pot and to every po/<lang>.po \
         (there is no extraction script — see CLAUDE.md).",
        missing.len()
    )
}

#[test]
fn every_tr_literal_is_in_the_pot() {
    let pot = catalog("po/modulixos-installer.pot");
    let ids = source_msgids();
    let missing: BTreeSet<&String> = ids.iter().filter(|id| !pot.contains_key(*id)).collect();
    assert!(
        missing.is_empty(),
        "{}",
        report(
            "used in src/ but absent from po/modulixos-installer.pot",
            &missing
        )
    );
}

#[test]
fn every_pot_msgid_is_translated_in_every_language() {
    let pot = catalog("po/modulixos-installer.pot");
    for lang in LANGUAGES {
        let po = catalog(&format!("po/{lang}.po"));
        let missing: BTreeSet<&String> = pot.keys().filter(|id| !po.contains_key(*id)).collect();
        assert!(
            missing.is_empty(),
            "{}",
            report(
                &format!("in the pot but absent from po/{lang}.po"),
                &missing
            )
        );
        let untranslated: BTreeSet<&String> = po
            .iter()
            .filter(|(id, text)| pot.contains_key(*id) && text.is_empty())
            .map(|(id, _)| id)
            .collect();
        assert!(
            untranslated.is_empty(),
            "{}",
            report(
                &format!("left with an empty msgstr in po/{lang}.po"),
                &untranslated
            )
        );
    }
}
