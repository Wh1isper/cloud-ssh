//! Shared model types for Cloud SSH.

use serde::{Deserialize, Serialize};

/// Unique identifier for a collaborative terminal room.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
pub struct RoomId(String);

impl RoomId {
    /// Creates a room identifier.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Unique identifier for a connected client.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
pub struct ClientId(String);

impl ClientId {
    /// Creates a client identifier.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Client transport family.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    /// Classic SSH client.
    Ssh,
    /// Browser or API client over WebSocket.
    Web,
}

/// Terminal grid size in character cells.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct TerminalSize {
    /// Terminal rows.
    pub rows: u16,
    /// Terminal columns.
    pub cols: u16,
}

impl TerminalSize {
    /// Creates a terminal size.
    ///
    /// Both dimensions are clamped to at least one cell.
    #[must_use]
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            rows: rows.max(1),
            cols: cols.max(1),
        }
    }
}

/// High-level resize policy for the real PTY.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizePolicy {
    /// The current input authority holder controls the real PTY size.
    Controller,
    /// The room uses a fixed configured PTY size.
    Pinned,
}

#[cfg(test)]
mod tests {
    use super::TerminalSize;

    #[test]
    fn terminal_size_is_never_zero() {
        let size = TerminalSize::new(0, 0);

        assert_eq!(size.rows, 1);
        assert_eq!(size.cols, 1);
    }
}
