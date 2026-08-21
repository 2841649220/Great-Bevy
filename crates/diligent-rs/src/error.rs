//! Error type shared by the whole wrapper.

use std::ffi::NulError;

/// Errors produced by the Diligent wrapper.
#[derive(Debug)]
pub enum Error {
    /// A raw pointer that must be non-null was null.
    NullPointer(&'static str),
    /// A required vtable method field was missing (`None`).
    MissingMethod(&'static str),
    /// The engine reported failure by not producing an object.
    CreateFailed(&'static str),
    /// An argument failed validation.
    InvalidArgument(&'static str),
    /// A wgpu/engine format has no counterpart on the other side of the
    /// format mapping table (see [`crate::format`]).
    UnsupportedFormat(&'static str),
    /// A string contained a NUL byte and could not be passed to C.
    Nul(NulError),
    /// Free-form error message.
    Message(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NullPointer(what) => write!(f, "null pointer: {what}"),
            Error::MissingMethod(what) => write!(f, "missing vtable method: {what}"),
            Error::CreateFailed(what) => write!(f, "engine failed to create {what}"),
            Error::InvalidArgument(what) => write!(f, "invalid argument: {what}"),
            Error::UnsupportedFormat(what) => write!(f, "format has no counterpart in the mapping table: {what}"),
            Error::Nul(e) => write!(f, "string contains NUL byte: {e}"),
            Error::Message(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Nul(e) => Some(e),
            _ => None,
        }
    }
}

impl From<NulError> for Error {
    fn from(e: NulError) -> Self {
        Error::Nul(e)
    }
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Message(s)
    }
}

/// Convenience alias used by all public API of this crate.
pub type Result<T> = std::result::Result<T, Error>;
