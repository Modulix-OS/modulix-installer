//! Passwords, passphrases and PINs, held so that their heap buffer is wiped
//! when the last copy goes away.
//!
//! Every secret this installer collects reaches it as one `String` built from
//! a GTK entry, is cloned along with `InstallConfig` into the install
//! pipeline, and is then handed to a child process' stdin. Plain `String`
//! drops leave all of those copies in freed heap memory for the rest of the
//! session, which matters here for two reasons the kiosk makes concrete: the
//! live ISO keeps running after a failed install (its error page is
//! interactive), and `MountTask` calls `swapon` on the install target, so a
//! page-out can put the installer's heap on a disk that survives the reboot
//! — unencrypted, when the user chose no encryption.
//!
//! [`Secret`] is the whole mechanism: `zeroize`'s `Zeroizing` wrapper plus a
//! `Debug` that prints nothing. `Clone` is deliberately kept — the wizard's
//! answers are cloned into `TaskCtx` — and each clone wipes its own buffer on
//! drop.
//!
//! Two limits are structural and must not be mistaken for coverage:
//!
//! * **GTK owns the first copy.** `adw::PasswordEntryRow` keeps the typed
//!   text in a `GtkEntryBuffer`, and GTK4 offers no wiping buffer; the
//!   installer's own copy starts at the `text().to_string()` that leaves it.
//! * **Only the current allocation is wiped.** A `Secret` built from a
//!   finished `String` (the only way this installer builds one) owns a single
//!   buffer, but mutating one into growing would leave the pre-realloc copy
//!   behind — which is why [`Secret`] exposes no mutation at all.
//!
//! What a process boundary does with a secret is equally out of reach:
//! `chpasswd` and `cryptsetup` read theirs from a pipe, and
//! `systemd-cryptenroll` from a `0600` file in `/run` that
//! `backend::crypt::cryptsetup`'s `SecretDir` unlinks on drop.

use std::fmt;
use std::ops::Deref;
use zeroize::Zeroizing;

/// One secret string, zeroed on drop and never printed.
///
/// Derefs to `str`, so the usual read-only string methods (`trim`,
/// `is_empty`, `contains`, …) work unchanged on it.
#[derive(Clone, Default)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    /// Takes ownership of a secret string.
    ///
    /// * `value` - the secret. Its buffer is the one wiped on drop, so the
    ///   caller must not keep another copy of it.
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    /// Borrows the secret for the duration of a call that needs a `&str`
    /// (a process' stdin, a backend argument).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for Secret {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self::new(value.to_string())
    }
}

impl fmt::Debug for Secret {
    /// Prints a placeholder, never the secret: `InstallConfig` and
    /// `PartitioningConfig` derive `Debug`, and a derived one would otherwise
    /// put every password of the install into any `{:?}` that reaches a log
    /// line or an error report.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_secret() {
        let secret = Secret::from("hunter2");
        assert_eq!(format!("{secret:?}"), "Secret(***)");
        assert!(!format!("{secret:?}").contains("hunter2"));
    }

    #[test]
    fn deref_exposes_the_string_api() {
        let secret = Secret::from("  hunter2  ");
        assert_eq!(secret.trim(), "hunter2");
        assert!(!secret.is_empty());
        assert!(Secret::default().is_empty());
    }

    #[test]
    fn as_str_round_trips() {
        assert_eq!(Secret::new("hunter2".to_string()).as_str(), "hunter2");
    }
}
