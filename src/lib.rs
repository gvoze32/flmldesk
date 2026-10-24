//! FL ML Desk: Master League transfer & budget editor for Football Life 2026 / PES 2021 saves.

pub mod contracts;
pub mod cpk;
pub mod crypto;
pub mod masterleague;
pub mod players;
pub mod rating;
pub mod update;

use std::fmt;

/// User-facing error; messages are shown verbatim in the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}
