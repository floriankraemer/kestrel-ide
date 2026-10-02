//! PHP support (the PHP parity plan, ADR-0068): the Composer model, the
//! language-level rule, the interpreter probe, the language-server settings
//! mapping and the Composer tool window's model.
//!
//! Qt-free and tokio-free, like every support crate. Run-time concerns
//! (`ToolchainId::Php`, composer-script run configs, `php -S`) stay in
//! `run-core`, which must not depend on this crate (ADR-0039).

pub mod composer;
pub mod level;
pub mod probe;

/// The `secret-store` service name for PHP secrets, distinct from the
/// other features' so no two share one keychain namespace.
pub const SECRET_SERVICE: &str = "ide.php";

/// The `secret-store` entry id holding the Intelephense licence key.
pub const INTELEPHENSE_LICENCE_ID: &str = "intelephense-licence";
