//! Password strength check per CNIL's password recommendation (deliberation
//! n° 2022-100, "authentication by password alone" case) — a password is
//! strong enough if it meets *any one* of the three published examples:
//!
//! 1. 12+ characters: uppercase, lowercase, digits **and** special characters.
//! 2. 14+ characters: uppercase, lowercase and digits (no special character required).
//! 3. A passphrase of 7+ words.
//!
//! This is advisory only — the wizard never blocks on it.

use regex::Regex;
use std::sync::LazyLock;

const EXAMPLE_1_MIN_LEN: usize = 12;
const EXAMPLE_2_MIN_LEN: usize = 14;
const PASSPHRASE_MIN_WORDS: usize = 7;

// `regex` has no lookahead, so each character class is matched by its own
// pattern and combined below — rather than a single "contains all classes"
// regex.
static LOWER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{Ll}").unwrap());
static UPPER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{Lu}").unwrap());
static DIGIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9]").unwrap());
static SPECIAL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}]").unwrap());
static WORD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\S+").unwrap());

pub fn is_strong_enough(password: &str) -> bool {
    matches_example_1(password)
        || matches_example_2(password)
        || matches_example_3_passphrase(password)
}

struct CharClasses {
    lower: bool,
    upper: bool,
    digit: bool,
    special: bool,
}

fn char_classes(password: &str) -> CharClasses {
    CharClasses {
        lower: LOWER_RE.is_match(password),
        upper: UPPER_RE.is_match(password),
        digit: DIGIT_RE.is_match(password),
        special: SPECIAL_RE.is_match(password),
    }
}

/// 12+ characters, uppercase + lowercase + digits + special characters.
fn matches_example_1(password: &str) -> bool {
    if password.chars().count() < EXAMPLE_1_MIN_LEN {
        return false;
    }
    let c = char_classes(password);
    c.lower && c.upper && c.digit && c.special
}

/// 14+ characters, uppercase + lowercase + digits, no special character required.
fn matches_example_2(password: &str) -> bool {
    if password.chars().count() < EXAMPLE_2_MIN_LEN {
        return false;
    }
    let c = char_classes(password);
    c.lower && c.upper && c.digit
}

/// A passphrase of 7+ words.
fn matches_example_3_passphrase(password: &str) -> bool {
    WORD_RE.find_iter(password).count() >= PASSPHRASE_MIN_WORDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_password_is_weak() {
        assert!(!is_strong_enough(""));
    }

    #[test]
    fn example_1_needs_all_four_classes() {
        assert!(is_strong_enough("Correct1Horse!"));
        assert!(!is_strong_enough("Correct1Horse")); // missing special char, only 13 chars
    }

    #[test]
    fn example_1_needs_twelve_chars() {
        assert!(!is_strong_enough("Cor1Hor!")); // 8 chars, all classes present but too short
    }

    #[test]
    fn example_2_fourteen_chars_no_special_needed() {
        assert!(is_strong_enough("Correct1Horse2"));
    }

    #[test]
    fn example_2_rejects_under_fourteen_chars() {
        assert!(!is_strong_enough("Correct1Horse")); // 13 chars
    }

    #[test]
    fn example_2_still_needs_upper_lower_digit() {
        assert!(!is_strong_enough("correcthorsebattery")); // 19 chars but no uppercase/digit
    }

    #[test]
    fn example_3_seven_word_passphrase_is_strong() {
        assert!(is_strong_enough(
            "correct horse battery staple wombat forest lantern"
        ));
    }

    #[test]
    fn example_3_six_words_is_not_enough() {
        assert!(!is_strong_enough(
            "correct horse battery staple wombat forest"
        ));
    }
}
