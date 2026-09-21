//! `DbError`/`DbErrorCode`: the one error type every `db-core` operation
//! and every backend driver returns, crossing the FFI seam as a typed code
//! plus message (ADR-0003) rather than a string a caller has to parse.

/// Append-only: a variant's discriminant is part of the FFI contract once
/// shipped (`ui-shell`'s bridge matches on the numeric code), so a
/// retired case is never renumbered or removed, only stops being
/// produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum DbErrorCode {
    Unknown = 0,
    ConnectionFailed = 1,
    Timeout = 2,
    Cancelled = 3,
    /// The client disconnected (or the session closed) while a statement
    /// was still running, distinct from an explicit user cancel — the two
    /// `executionFinished` outcomes `database-tools.md` §4 names.
    CancelledByDisconnect = 4,
    ReadOnlyViolation = 5,
    NotSupported = 6,
    DriverChecksum = 7,
    CapReached = 8,
    NoPrimaryKey = 9,
    InvalidStatement = 10,
    TunnelFailed = 11,
    SecretUnavailable = 12,
    Io = 13,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbError {
    pub code: DbErrorCode,
    pub message: String,
}

impl DbError {
    pub fn new(code: DbErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for DbError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_shows_the_message_not_the_code() {
        let error = DbError::new(DbErrorCode::Timeout, "connect timed out after 10s");
        assert_eq!(error.to_string(), "connect timed out after 10s");
    }

    #[test]
    fn discriminants_stay_pinned() {
        // A gate against accidental renumbering: this test fails loudly if
        // anyone reorders the enum instead of appending.
        assert_eq!(DbErrorCode::Unknown as u32, 0);
        assert_eq!(DbErrorCode::ConnectionFailed as u32, 1);
        assert_eq!(DbErrorCode::Timeout as u32, 2);
        assert_eq!(DbErrorCode::Cancelled as u32, 3);
        assert_eq!(DbErrorCode::CancelledByDisconnect as u32, 4);
        assert_eq!(DbErrorCode::ReadOnlyViolation as u32, 5);
        assert_eq!(DbErrorCode::NotSupported as u32, 6);
        assert_eq!(DbErrorCode::DriverChecksum as u32, 7);
        assert_eq!(DbErrorCode::CapReached as u32, 8);
        assert_eq!(DbErrorCode::NoPrimaryKey as u32, 9);
        assert_eq!(DbErrorCode::InvalidStatement as u32, 10);
        assert_eq!(DbErrorCode::TunnelFailed as u32, 11);
        assert_eq!(DbErrorCode::SecretUnavailable as u32, 12);
        assert_eq!(DbErrorCode::Io as u32, 13);
    }
}
