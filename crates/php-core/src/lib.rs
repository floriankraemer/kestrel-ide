//! PHP support (the PHP parity plan, ADR-0068): the Composer model, the
//! language-level rule, the interpreter probe, the language-server settings
//! mapping and the Composer tool window's model.
//!
//! Qt-free and tokio-free, like every support crate. Run-time concerns
//! (`ToolchainId::Php`, composer-script run configs, `php -S`) stay in
//! `run-core`, which must not depend on this crate (ADR-0039).

pub mod composer;
