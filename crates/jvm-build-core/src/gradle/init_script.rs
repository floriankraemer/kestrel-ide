//! The Gradle init-script asset name, and the argv a sync/test run builds
//! around it (A4).
//!
//! The script itself lives at
//! `crates/plugin-host/builtin/jvm-build-tools/ide-model.init.gradle` — the
//! plugin's own asset, embedded into the binary via `include_bytes!` in
//! `plugin-host/src/builtins.rs` and materialised to a real path on disk by
//! `LoadedPlugin::asset_dir` (A3) before a sync ever runs. This crate never
//! embeds the script itself; [`SOURCE_FOR_TESTS`] exists only so this
//! crate's own unit tests can sanity-check the two are still the one file
//! (`include_str!` through a relative path, not a second copy) — the plugin
//! host's own "every asset the manifest names exists" test is what proves
//! the *registration* is correct.

use std::path::Path;

/// The asset's filename, as `plugin.toml`'s `init-script` and `args`
/// entries spell it — kept as one constant so a rename is a one-place edit.
pub const ASSET_NAME: &str = "ide-model.init.gradle";

/// Read only by this crate's own tests, to prove the file A3 registered is
/// still there and still looks like the script this crate's `sync`/
/// `model_json` modules were written against.
#[cfg(test)]
pub(crate) const SOURCE_FOR_TESTS: &str =
    include_str!("../../../plugin-host/builtin/jvm-build-tools/ide-model.init.gradle");

/// `gradlew --init-script <script> -q ideModel --console=plain
/// --no-configuration-cache -Pide.modelOut=<model_out>`, plus `--offline`
/// when asked.
///
/// `--no-configuration-cache` is required on every version this script
/// supports (>= 6.6) — see the script's own header comment — so it is
/// never conditional here.
pub fn model_args(script_path: &Path, model_out: &Path, offline: bool) -> Vec<String> {
    let mut args = vec![
        "--init-script".to_string(),
        script_path.display().to_string(),
        "-q".to_string(),
        "ideModel".to_string(),
        "--console=plain".to_string(),
        "--no-configuration-cache".to_string(),
        format!("-Pide.modelOut={}", model_out.display()),
    ];
    if offline {
        args.push("--offline".to_string());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registered_script_still_defines_the_ide_model_task_and_the_listener() {
        assert!(SOURCE_FOR_TESTS.contains("ideModel"));
        assert!(SOURCE_FOR_TESTS.contains("IdeTeamCityListener"));
        assert!(SOURCE_FOR_TESTS.contains("ide.modelOut"));
    }

    #[test]
    fn model_args_names_the_script_and_the_output_file() {
        let args = model_args(
            Path::new("/plugins/jvm-build-tools/ide-model.init.gradle"),
            Path::new("/tmp/model.json"),
            false,
        );
        assert!(args.contains(&"--init-script".to_string()));
        assert!(args.contains(&"/plugins/jvm-build-tools/ide-model.init.gradle".to_string()));
        assert!(args.contains(&"-Pide.modelOut=/tmp/model.json".to_string()));
        assert!(args.contains(&"--no-configuration-cache".to_string()));
        assert!(!args.contains(&"--offline".to_string()));
    }

    #[test]
    fn offline_appends_the_flag() {
        let args = model_args(Path::new("script.gradle"), Path::new("out.json"), true);
        assert!(args.contains(&"--offline".to_string()));
    }
}
