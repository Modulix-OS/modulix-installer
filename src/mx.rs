//! Local error type. Mirrors modulix-core-utils' `mx::Result` style: one enum,
//! no `thiserror`, propagate with `?`.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// A backend (D-Bus proxy, subprocess, file parsing) failed with a
    /// human-readable reason. Convention: backends put a bare gettext msgid
    /// in here (untranslated); callers render it with
    /// `match &e { Error::Backend(msgid) => tr(msgid), other => other.to_string() }`.
    /// `tr()` returns the string unchanged if it isn't in the catalog, so
    /// other backends that don't follow this convention don't regress.
    Backend(String),
    Io(std::io::Error),
    Utf8(std::string::FromUtf8Error),
    CoreUtils(modulix_core_utils::mx::ErrorKind),
    Zbus(zbus::Error),
    ZbusFdo(zbus::fdo::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Backend(msg) => write!(f, "backend error: {msg}"),
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Utf8(e) => write!(f, "invalid UTF-8: {e}"),
            Error::CoreUtils(e) => write!(f, "modulix-core-utils error: {e:?}"),
            Error::Zbus(e) => write!(f, "D-Bus error: {e}"),
            Error::ZbusFdo(e) => write!(f, "D-Bus error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<std::string::FromUtf8Error> for Error {
    fn from(e: std::string::FromUtf8Error) -> Self {
        Error::Utf8(e)
    }
}

impl From<modulix_core_utils::mx::ErrorKind> for Error {
    fn from(e: modulix_core_utils::mx::ErrorKind) -> Self {
        Error::CoreUtils(e)
    }
}

impl From<zbus::Error> for Error {
    fn from(e: zbus::Error) -> Self {
        Error::Zbus(e)
    }
}

impl From<zbus::fdo::Error> for Error {
    fn from(e: zbus::fdo::Error) -> Self {
        Error::ZbusFdo(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
