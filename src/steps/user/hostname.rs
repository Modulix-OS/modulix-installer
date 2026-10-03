use crate::i18n::tr;

/// RFC 1123 host label: `[a-z0-9-]`, 63 characters max, no leading or
/// trailing `-`. NixOS rejects anything else in `networking.hostName`.
const MAX_LEN: usize = 63;

/// Default host name proposed by the user step, used when the field is left
/// untouched.
pub use crate::config::DEFAULT_HOSTNAME as DEFAULT;

/// Live filter applied to what the user types into the host name field.
///
/// * `input` - raw field content.
///
/// # Returns
/// `input` lowercased, with everything outside `[a-z0-9-]` dropped (so every
/// kind of space, including NBSP, disappears) and truncated to 63 characters.
/// A leading `-` is never blocked here, only reported by [`blocked_reason`],
/// so the user can still type a name starting with one and fix it.
pub fn sanitize(input: &str) -> String {
    let mut out: String = input
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .collect();
    out.truncate(MAX_LEN);
    out
}

/// Blocked reason for the `ValidityTracker`, or `None` if valid.
///
/// * `name` - an already [`sanitize`]d host name.
///
/// # Returns
/// A translated, user-facing reason when `name` is empty or starts/ends with
/// `-`, `None` otherwise. Length is never a reason: [`sanitize`] truncates.
pub fn blocked_reason(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some(tr("Enter a computer name"));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Some(tr("The computer name cannot start or end with a hyphen"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_and_lowercases() {
        assert_eq!(sanitize("Mon PC"), "monpc");
        assert_eq!(sanitize("Modulix_OS-1"), "modulixos-1");
        assert_eq!(sanitize("a\u{a0}b"), "ab");
        let long = "a".repeat(80);
        assert_eq!(sanitize(&long).len(), MAX_LEN);
    }

    #[test]
    fn blocked_reason_rejects_empty_and_edge_hyphens() {
        assert!(blocked_reason("").is_some());
        assert!(blocked_reason("-pc").is_some());
        assert!(blocked_reason("pc-").is_some());
        assert!(blocked_reason(DEFAULT).is_none());
        assert!(blocked_reason("mon-pc-2").is_none());
    }
}
