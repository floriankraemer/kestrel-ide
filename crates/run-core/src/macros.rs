//! Path macros in a run configuration (R1-3).
//!
//! A configuration's `cwd`, arguments and environment values may contain
//! tokens that only mean something at launch time: where the project is, and
//! which file the launch was started from. Expansion happens once, in
//! [`crate::config::RunConfigExt::to_launch_spec`], so nothing downstream has
//! to know a token from a literal.
//!
//! Two spellings are accepted for every token: the IntelliJ form
//! `$PROJECT_DIR$` and the bare `$PROJECT_DIR`. The bare form is what F4
//! shipped and what existing `.ide/settings.toml` files contain, so dropping
//! it would silently change what a saved configuration runs.

use std::path::{Path, PathBuf};

/// What the tokens are resolved against.
#[derive(Debug, Clone, Default)]
pub struct MacroContext {
    /// The open project's root. `None` only when no project is open, in
    /// which case `$PROJECT_DIR$` is left as written rather than expanded to
    /// something arbitrary.
    pub project_root: Option<PathBuf>,
    /// The file the launch was started from — set by run-from-context, empty
    /// for a configuration launched from the toolbar.
    pub file: Option<PathBuf>,
}

impl MacroContext {
    /// The common case: a project, no file.
    pub fn for_project(project_root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: Some(project_root.into()),
            file: None,
        }
    }

    /// Run-from-context: a project and the file the caret was in.
    pub fn for_file(project_root: impl Into<PathBuf>, file: impl Into<PathBuf>) -> Self {
        Self {
            project_root: Some(project_root.into()),
            file: Some(file.into()),
        }
    }

    fn value_of(&self, token: Token) -> Option<String> {
        // W4-3: a launched process runs inside the distro on a WSL project
        // root (W4-1), so a macro expanding to a path must already be the
        // Linux path — `host.argv` passes an argument string through
        // untouched, unlike `cwd`, which it does translate for `--cd`.
        // `ExecHost::for_path` on the project root, not `path` itself:
        // `$FILE_PATH$` names a file *under* the root, and only the root's
        // own spelling is guaranteed to carry the UNC prefix.
        let host = self
            .project_root
            .as_deref()
            .map(process_exec::host::ExecHost::for_path)
            .unwrap_or(process_exec::host::ExecHost::Local);
        let display = |path: &Path| {
            if host.is_remote() {
                host.to_remote(path)
            } else {
                path.display().to_string()
            }
        };
        match token {
            Token::ProjectDir => self.project_root.as_deref().map(display),
            Token::FilePath => self.file.as_deref().map(display),
            Token::FileDir => self.file.as_deref().and_then(Path::parent).map(display),
            Token::FileName => self
                .file
                .as_deref()
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().into_owned()),
            Token::FileNameWithoutExtension => self
                .file
                .as_deref()
                .and_then(Path::file_stem)
                .map(|n| n.to_string_lossy().into_owned()),
            // ponytail: stays the Windows-side $HOME/$USERPROFILE even on a
            // remote host — the distro's own home would need a login-shell
            // probe (`process_exec::host::resolve_program`'s style of I/O),
            // and no run configuration observed in this codebase uses
            // $USER_HOME$ for a path that reaches the spawned process.
            // Upgrade path: probe `echo $HOME` once per distro, memoised the
            // same way `resolve_program` is, if that ever changes.
            Token::UserHome => home_dir().as_deref().map(display),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    ProjectDir,
    FilePath,
    FileDir,
    FileName,
    FileNameWithoutExtension,
    UserHome,
}

/// Longest name first: `$FILE_NAME_WITHOUT_EXTENSION` starts with
/// `$FILE_NAME`, so a shorter match must never be tried first.
const TOKENS: &[(&str, Token)] = &[
    (
        "$FILE_NAME_WITHOUT_EXTENSION",
        Token::FileNameWithoutExtension,
    ),
    ("$PROJECT_DIR", Token::ProjectDir),
    ("$FILE_PATH", Token::FilePath),
    ("$FILE_NAME", Token::FileName),
    ("$FILE_DIR", Token::FileDir),
    ("$USER_HOME", Token::UserHome),
];

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Expand every known token in `value`.
///
/// A token the context cannot resolve — `$FILE_PATH$` with no file — is left
/// exactly as written. Leaving it visible is what makes the failure legible
/// in the console's command line; substituting an empty string would produce
/// a command that looks right and runs on the wrong path.
pub fn expand(value: &str, context: &MacroContext) -> String {
    if !value.contains('$') {
        return value.to_string();
    }
    let mut out = value.to_string();
    for (name, token) in TOKENS {
        let Some(replacement) = context.value_of(*token) else {
            continue;
        };
        // The `$`-terminated spelling first: replacing the bare one first
        // would turn `$PROJECT_DIR$` into `/path$`.
        out = out.replace(&format!("{name}$"), &replacement);
        out = out.replace(name, &replacement);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> MacroContext {
        MacroContext::for_file("/home/me/project", "/home/me/project/src/main.rs")
    }

    #[test]
    fn both_spellings_of_project_dir_expand() {
        assert_eq!(expand("$PROJECT_DIR/x", &context()), "/home/me/project/x");
        assert_eq!(expand("$PROJECT_DIR$/x", &context()), "/home/me/project/x");
    }

    #[test]
    fn the_dollar_terminated_form_does_not_leave_a_trailing_dollar() {
        assert_eq!(expand("$PROJECT_DIR$", &context()), "/home/me/project");
    }

    #[test]
    fn file_tokens_expand_from_the_context_file() {
        let ctx = context();
        assert_eq!(expand("$FILE_PATH$", &ctx), "/home/me/project/src/main.rs");
        assert_eq!(expand("$FILE_DIR$", &ctx), "/home/me/project/src");
        assert_eq!(expand("$FILE_NAME$", &ctx), "main.rs");
        assert_eq!(expand("$FILE_NAME_WITHOUT_EXTENSION$", &ctx), "main");
    }

    // W4-3: on a WSL project root, macros expand to the Linux path a
    // launched process actually understands, not the Windows UNC spelling
    // `host.argv` would otherwise pass through untouched.
    #[test]
    fn project_dir_expands_to_the_linux_path_on_a_wsl_root() {
        let ctx = MacroContext::for_project("//wsl.localhost/Ubuntu/home/f/proj");
        assert_eq!(expand("$PROJECT_DIR$", &ctx), "/home/f/proj");
    }

    #[test]
    fn file_path_expands_to_the_linux_path_on_a_wsl_root() {
        // Forward slashes: what actually reaches this crate — Qt hands
        // Windows paths (drive letters included) with `/` separators, per
        // `diagnostics_core::uri_from_path`'s own doc comment.
        let ctx = MacroContext::for_file(
            "//wsl.localhost/Ubuntu/home/f/proj",
            "//wsl.localhost/Ubuntu/home/f/proj/src/main.rs",
        );
        assert_eq!(expand("$FILE_PATH$", &ctx), "/home/f/proj/src/main.rs");
        assert_eq!(expand("$FILE_DIR$", &ctx), "/home/f/proj/src");
    }

    #[test]
    fn an_unresolvable_token_is_left_visible_rather_than_emptied() {
        let ctx = MacroContext::for_project("/p");
        assert_eq!(expand("$FILE_PATH$", &ctx), "$FILE_PATH$");
    }

    #[test]
    fn text_without_a_dollar_is_returned_unchanged() {
        assert_eq!(expand("/absolute/path", &context()), "/absolute/path");
    }

    #[test]
    fn several_tokens_in_one_value_all_expand() {
        assert_eq!(
            expand("$PROJECT_DIR$/out/$FILE_NAME$", &context()),
            "/home/me/project/out/main.rs"
        );
    }

    #[test]
    fn user_home_comes_from_the_environment() {
        // `HOME` is set in every environment this runs in; if it were not,
        // the token would be left visible, which is the case above.
        if home_dir().is_some() {
            assert_ne!(expand("$USER_HOME$", &context()), "$USER_HOME$");
        }
    }
}
