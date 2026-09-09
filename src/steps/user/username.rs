use crate::i18n::tr;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// shadow(5) NAME_REGEX: `[a-z_][a-z0-9_-]*`, 32 characters max.
const MAX_LEN: usize = 32;

/// NFD then strip combining marks (Mn), plus a table for letters that don't
/// decompose: ß→ss, ø→o, đ→d, ł→l, æ→ae, œ→oe, ð→d, þ→th.
fn fold_ascii(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().nfd() {
        if is_combining_mark(c) {
            continue;
        }
        match c {
            'ß' => out.push_str("ss"),
            'ø' => out.push('o'),
            'đ' => out.push('d'),
            'ł' => out.push('l'),
            'æ' => out.push_str("ae"),
            'œ' => out.push_str("oe"),
            'ð' => out.push('d'),
            'þ' => out.push_str("th"),
            _ => out.push(c),
        }
    }
    out
}

fn is_zero_width(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200D}' | '\u{FEFF}')
}

/// First word, transliterated, lowercased,
/// reduced to the allowed set, truncated to `MAX_LEN`. Empty string if
/// nothing valid survives.
pub fn from_full_name(full: &str) -> String {
    let word = full
        .split(|c: char| c.is_whitespace() || is_zero_width(c))
        .find(|w| !w.is_empty())
        .unwrap_or("");

    let mut candidate: String = fold_ascii(word)
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_' || *c == '-')
        .collect();
    candidate.truncate(MAX_LEN);

    if !candidate.chars().any(|c| c.is_ascii_lowercase()) {
        return String::new();
    }
    candidate
}

/// Live filter applied to what the user types into the username field:
/// lowercase, anything outside `[a-z0-9_-]` dropped (so every kind of space,
/// including NBSP and zero-width, disappears), truncated to `MAX_LEN`.
pub fn sanitize(input: &str) -> String {
    let mut out: String = input
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_' || *c == '-')
        .collect();
    out.truncate(MAX_LEN);
    out
}

/// Blocked reason for the `ValidityTracker`, or `None` if valid. Length is
/// never a blocking reason: `sanitize` already truncates.
pub fn blocked_reason(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some(tr("Enter a username"));
    }
    let first = name.chars().next().expect("checked non-empty above");
    if !(first.is_ascii_lowercase() || first == '_') {
        return Some(tr("The username must start with a letter or an underscore"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_full_name_transliterates_first_word() {
        assert_eq!(from_full_name("Pierre Paul"), "pierre");
        assert_eq!(from_full_name("Esquie"), "esquie");
        assert_eq!(from_full_name("Björn Ångström"), "bjorn");
        assert_eq!(from_full_name("François"), "francois");
        assert_eq!(from_full_name("  \u{a0}Jean\tPaul"), "jean");
    }

    #[test]
    fn from_full_name_empty_when_nothing_valid_survives() {
        assert_eq!(from_full_name(""), "");
        assert_eq!(from_full_name("123"), "");
        assert_eq!(from_full_name("Δημήτρης"), "");
    }

    #[test]
    fn from_full_name_truncates() {
        let long = "a".repeat(40);
        assert_eq!(from_full_name(&long).len(), MAX_LEN);
    }

    #[test]
    fn sanitize_strips_and_lowercases() {
        assert_eq!(sanitize("Jean Dupont"), "jeandupont");
        assert_eq!(sanitize("A.B-C_1"), "ab-c_1");
        assert_eq!(sanitize("a\u{a0}b\u{200B}c"), "abc");
        let long = "a".repeat(40);
        assert_eq!(sanitize(&long).len(), MAX_LEN);
    }

    #[test]
    fn blocked_reason_checks_first_char() {
        assert!(blocked_reason("").is_some());
        assert!(blocked_reason("1abc").is_some());
        assert!(blocked_reason("-abc").is_some());
        assert!(blocked_reason("jean").is_none());
        assert!(blocked_reason("_svc").is_none());
        assert!(blocked_reason("a-b_1").is_none());
    }
}
