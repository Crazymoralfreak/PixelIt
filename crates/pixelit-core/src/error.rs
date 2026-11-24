//! One error type for every PixelIt crate.
//!
//! An error carries a stable `code` and named `params`. The interface turns the
//! pair into a localized message through the `errors.<code>` dictionary keys, so
//! error text lives in the translations and never in Rust code.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Code of the error raised when the user cancels an operation.
pub const CANCELLED: &str = "cancelled";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Error {
    pub code: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn new(code: &str) -> Self {
        Self {
            code: code.to_owned(),
            params: BTreeMap::new(),
        }
    }

    /// Adds a named parameter used by the localized message.
    #[must_use]
    pub fn with(mut self, key: &str, value: impl fmt::Display) -> Self {
        self.params.insert(key.to_owned(), value.to_string());
        self
    }

    /// Adds the technical description of an underlying failure.
    #[must_use]
    pub fn detail(self, value: impl fmt::Display) -> Self {
        self.with("detail", value)
    }

    pub fn cancelled() -> Self {
        Self::new(CANCELLED)
    }

    pub fn is_cancelled(&self) -> bool {
        self.code == CANCELLED
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.code)?;
        let mut first = true;
        for (key, value) in &self.params {
            f.write_str(if first { " (" } else { ", " })?;
            write!(f, "{key}={value}")?;
            first = false;
        }
        if !first {
            f.write_str(")")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        let code = match error.kind() {
            std::io::ErrorKind::NotFound => "notFound",
            std::io::ErrorKind::PermissionDenied => "accessDenied",
            _ => "io",
        };
        Self::new(code).detail(error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::new("badFile").detail(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_lists_params() {
        let error = Error::new("tooLarge").with("width", 10).with("height", 20);
        assert_eq!(error.to_string(), "tooLarge (height=20, width=10)");
        assert_eq!(Error::new("io").to_string(), "io");
    }

    #[test]
    fn serializes_without_empty_params() {
        let json = serde_json::to_string(&Error::new("io")).unwrap();
        assert_eq!(json, r#"{"code":"io"}"#);
        let back: Error = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Error::new("io"));
    }

    #[test]
    fn cancelled_is_recognized() {
        assert!(Error::cancelled().is_cancelled());
        assert!(!Error::new("io").is_cancelled());
    }
}
