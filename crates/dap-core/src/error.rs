//! Why a `dap-core` operation failed.
//!
//! Same shape as its siblings (ADR-0003): a typed variant per failure kind,
//! a stable numeric code in the 300-399 range ADR-0041 claims out of
//! ADR-0003 §4's headroom, and a `Display` message meant to be shown
//! verbatim.

use std::fmt;

/// Why a debug session could not start, or could not continue.
#[derive(Debug, PartialEq, Eq)]
pub enum DapError {
    /// No adapter is configured for this language, and the catalog ships no
    /// default for it.
    NoAdapter(String),
    /// The adapter's program is not installed, or could not be started. The
    /// message carries the install hint the catalog knows, because "codelldb
    /// is not installed" is only useful with "install it from …" attached.
    AdapterNotStarted { adapter: String, reason: String },
    /// The adapter replied with `success: false`.
    Request { command: String, message: String },
    /// The adapter died, or the session was stopped, while a request was in
    /// flight.
    Disconnected(String),
    /// The adapter did not answer in time.
    Timeout(String),
    /// The adapter said it does not support this, and the caller asked
    /// anyway — a bug in the caller, reported rather than papered over.
    Unsupported(String),
    /// The wire carried something that is not a DAP message.
    Protocol(String),
    /// A PHP debug run that cannot work: the message says why and how to
    /// fix it.
    XdebugUnavailable(String),
}

impl DapError {
    pub const CODE_NO_ADAPTER: i32 = 300;
    pub const CODE_ADAPTER_NOT_STARTED: i32 = 301;
    pub const CODE_REQUEST: i32 = 302;
    pub const CODE_DISCONNECTED: i32 = 303;
    pub const CODE_TIMEOUT: i32 = 304;
    pub const CODE_UNSUPPORTED: i32 = 305;
    pub const CODE_PROTOCOL: i32 = 306;
    pub const CODE_XDEBUG_UNAVAILABLE: i32 = 307;

    /// The variant's stable numeric code. Append-only once this crosses an
    /// FFI seam (ADR-0003).
    pub fn code(&self) -> i32 {
        match self {
            DapError::NoAdapter(_) => Self::CODE_NO_ADAPTER,
            DapError::AdapterNotStarted { .. } => Self::CODE_ADAPTER_NOT_STARTED,
            DapError::Request { .. } => Self::CODE_REQUEST,
            DapError::Disconnected(_) => Self::CODE_DISCONNECTED,
            DapError::Timeout(_) => Self::CODE_TIMEOUT,
            DapError::Unsupported(_) => Self::CODE_UNSUPPORTED,
            DapError::Protocol(_) => Self::CODE_PROTOCOL,
            DapError::XdebugUnavailable(_) => Self::CODE_XDEBUG_UNAVAILABLE,
        }
    }
}

impl fmt::Display for DapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DapError::NoAdapter(language) => {
                write!(f, "no debug adapter is configured for {language}")
            }
            DapError::AdapterNotStarted { adapter, reason } => {
                write!(f, "{adapter} could not be started: {reason}")
            }
            DapError::Request { command, message } => write!(f, "{command} failed: {message}"),
            DapError::Disconnected(what) => {
                write!(f, "the debug adapter disconnected during {what}")
            }
            DapError::Timeout(what) => write!(f, "the debug adapter did not answer {what} in time"),
            DapError::Unsupported(what) => write!(f, "this debug adapter does not support {what}"),
            DapError::Protocol(detail) => write!(f, "malformed debug adapter message: {detail}"),
            DapError::XdebugUnavailable(advice) => f.write_str(advice),
        }
    }
}

impl std::error::Error for DapError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_is_unique_and_inside_the_range() {
        let codes = [
            DapError::NoAdapter(String::new()).code(),
            DapError::AdapterNotStarted {
                adapter: String::new(),
                reason: String::new(),
            }
            .code(),
            DapError::Request {
                command: String::new(),
                message: String::new(),
            }
            .code(),
            DapError::Disconnected(String::new()).code(),
            DapError::Timeout(String::new()).code(),
            DapError::Unsupported(String::new()).code(),
            DapError::Protocol(String::new()).code(),
            DapError::XdebugUnavailable(String::new()).code(),
        ];
        for code in codes {
            assert!(
                (300..=399).contains(&code),
                "{code} left dap-core's 300-399 range (ADR-0003 §4)"
            );
        }
        let mut deduped = codes.to_vec();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(deduped.len(), codes.len(), "two variants share a code");
    }

    #[test]
    fn a_missing_adapter_names_the_language_and_the_reason_names_the_fix() {
        let err = DapError::AdapterNotStarted {
            adapter: "codelldb".into(),
            reason: "not found on PATH".into(),
        };
        assert!(err.to_string().contains("codelldb"));
        assert!(err.to_string().contains("not found on PATH"));
    }
}
