//! Xdebug through vscode-php-debug (ADR-0069): the environment a PHP
//! process needs to connect back, and where the adapter listens for it.

/// Xdebug 3's default client port.
pub const DEFAULT_PORT: u16 = 9003;
/// The adapter's listen address when only this machine connects.
pub const LOOPBACK: &str = "127.0.0.1";
