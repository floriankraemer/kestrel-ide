//! Which contributed test framework a project's run actually uses
//! (jvm-build-tools plan, review fix for the Tests dock regression the
//! `junit-gradle`/`junit-maven` rows introduced).
//!
//! `TestServiceRust::detect_framework` (`ui-shell`) used to take "the first
//! contributed framework whose program resolves" with no other check —
//! right when the only contribution was PHPUnit, wrong the moment a
//! framework can also require a toolchain (`junit-gradle` needs Gradle
//! detected, not just `gradle` resolving on `PATH`, or a Rust project with
//! `gradle` installed globally would "detect" it) or name an output format
//! this build cannot stream yet (`junit-maven`'s `junit-xml` has no runner
//! until `test-core`'s C1 phase). This is the rule, unit-tested here so the
//! adapter stays translation-only.

use plugin_api::TestFrameworkContribution;

/// Output formats [`crate::runner::run`] can actually stream today.
/// `junit-xml` (a post-run file read, not a stream) is not one of them —
/// added when C1 gives it a runner.
pub const SUPPORTED_OUTPUT_FORMATS: &[&str] = &["teamcity"];

/// The first framework, in contribution order, that is runnable against
/// this project: its `requires_toolchain` (if any) is among
/// `detected_toolchains`, its `output_format` is one this build can stream,
/// and `resolve_program` finds one of its `program_candidates`.
///
/// Returns the winning index (so a caller holding a parallel list of
/// owning plugins, which this crate does not know about, can look one up)
/// alongside the contribution and the resolved program path.
pub fn select_framework<'a>(
    frameworks: &'a [TestFrameworkContribution],
    detected_toolchains: &[&str],
    supported_output_formats: &[&str],
    mut resolve_program: impl FnMut(&[String]) -> Option<std::path::PathBuf>,
) -> Option<(usize, &'a TestFrameworkContribution, std::path::PathBuf)> {
    frameworks
        .iter()
        .enumerate()
        .find_map(|(index, framework)| {
            if let Some(required) = &framework.requires_toolchain {
                if !detected_toolchains.contains(&required.as_str()) {
                    return None;
                }
            }
            if !supported_output_formats.contains(&framework.output_format.as_str()) {
                return None;
            }
            resolve_program(&framework.program_candidates)
                .map(|program| (index, framework, program))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn framework(id: &str) -> TestFrameworkContribution {
        TestFrameworkContribution {
            id: id.to_string(),
            name: id.to_string(),
            program_candidates: vec![id.to_string()],
            args: vec![],
            filter_flag: Some("--filter".to_string()),
            filter_template: None,
            output_format: "teamcity".to_string(),
            config_file_candidates: vec![],
            requires_toolchain: None,
            report_glob: None,
        }
    }

    fn always_resolves(candidates: &[String]) -> Option<PathBuf> {
        candidates.first().map(PathBuf::from)
    }

    #[test]
    fn the_first_runnable_framework_wins() {
        let frameworks = vec![framework("phpunit")];
        let result = select_framework(&frameworks, &[], &["teamcity"], always_resolves);
        assert_eq!(result.unwrap().0, 0);
    }

    #[test]
    fn a_framework_requiring_an_undetected_toolchain_is_skipped() {
        let mut gradle = framework("junit-gradle");
        gradle.requires_toolchain = Some("gradle".to_string());
        let frameworks = vec![gradle];
        // No toolchains detected (e.g. a Rust project with `gradle`
        // installed globally): must not "detect" junit-gradle just because
        // the binary happens to resolve on PATH.
        let result = select_framework(&frameworks, &[], &["teamcity"], always_resolves);
        assert!(result.is_none());
    }

    #[test]
    fn a_framework_requiring_a_detected_toolchain_is_selected() {
        let mut gradle = framework("junit-gradle");
        gradle.requires_toolchain = Some("gradle".to_string());
        let frameworks = vec![gradle];
        let result = select_framework(&frameworks, &["gradle"], &["teamcity"], always_resolves);
        assert!(result.is_some());
    }

    #[test]
    fn a_framework_with_an_unsupported_output_format_is_skipped() {
        let mut maven = framework("junit-maven");
        maven.output_format = "junit-xml".to_string();
        let frameworks = vec![maven];
        let result = select_framework(&frameworks, &[], &["teamcity"], always_resolves);
        assert!(result.is_none(), "junit-xml has no runner yet (C1)");
    }

    #[test]
    fn a_framework_whose_program_does_not_resolve_is_skipped_and_the_next_one_tried() {
        let frameworks = vec![framework("missing"), framework("present")];
        let result = select_framework(&frameworks, &[], &["teamcity"], |candidates| {
            if candidates[0] == "missing" {
                None
            } else {
                Some(PathBuf::from(&candidates[0]))
            }
        });
        assert_eq!(result.unwrap().0, 1);
    }

    #[test]
    fn no_frameworks_at_all_yields_none() {
        assert!(select_framework(&[], &[], &["teamcity"], always_resolves).is_none());
    }
}
