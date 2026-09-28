//! One integration-test binary for app's manual timing probes (`#[ignore]`d,
//! run by hand, never part of the regular `e2e-ci` budget), so the crate's
//! dependency tree links once instead of once per file.
mod fast_project_open_timing;
mod settings_dialog_timing;
