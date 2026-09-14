//! SELinux `:z` bind-mount relabeling (C9), one rule shared by every argv
//! builder that emits a bind mount: [`crate::run_config::image_run_argv`]
//! (via `mount_arg`), [`crate::target::wrap_launch`] and
//! [`crate::recreate::recreate_argv`].
//!
//! `[containers].selinux_relabel` is an opt-in advanced setting (there is
//! no way to detect "this host runs SELinux" from here that is not itself
//! a guess), so this module only ever answers "is this particular bind
//! mount safe to relabel", never "should relabeling happen at all" — the
//! caller already knows that from the setting.

/// Top-level directories a bind mount must never get the SELinux `:z`
/// relabel suffix for, even with `[containers].selinux_relabel` on:
/// relabeling one of these is either a system-breaking mistake (`/`, `/etc`,
/// `/usr`, `/bin`, `/lib`, `/lib64`, `/sbin`, `/boot`, `/dev`, `/proc`,
/// `/sys`, `/run`) or almost always the wrong container-security tradeoff
/// for a whole shared directory (`/home`, `/root`, `/tmp`, `/var`, `/opt`,
/// `/mnt`, `/media`, `/srv`). A project directory under `/home/<user>/...`
/// is unaffected — only the bare top-level path itself matches, and only
/// as an *exact* segment: `/home2` or `/var-data` are ordinary
/// subdirectories as far as this rule is concerned.
const NEVER_RELABEL: &[&str] = &[
    "/", "/bin", "/usr", "/etc", "/home", "/lib", "/lib64", "/sbin", "/boot", "/dev", "/proc",
    "/sys", "/root", "/tmp", "/var", "/opt", "/mnt", "/media", "/srv", "/run",
];

/// The `:z`/... suffix to append to a bind mount's option list for
/// `host_path`, or `None` when this path must never be relabeled — WSL
/// paths are ordinary Linux paths from the container engine's point of
/// view (the distro's own filesystem, not a Windows drive), so they take
/// the same rule as every other Linux path; there is no macOS/Windows
/// host case to skip here because Docker Desktop's own VM applies its
/// labels and does not expose `:z` at all — this function is only ever
/// called when `[containers].selinux_relabel` is on, which the Settings
/// page only offers on a Linux host.
pub fn relabel_suffix(host_path: &str) -> Option<&'static str> {
    let trimmed = host_path.trim_end_matches('/');
    let trimmed = if trimmed.is_empty() { "/" } else { trimmed };
    if NEVER_RELABEL.contains(&trimmed) {
        None
    } else {
        Some("z")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relabels_an_ordinary_project_path() {
        assert_eq!(relabel_suffix("/home/f/project"), Some("z"));
        assert_eq!(relabel_suffix("/home/f/project/"), Some("z"));
    }

    #[test]
    fn skips_every_top_level_directory_exactly() {
        for top in NEVER_RELABEL {
            assert_eq!(relabel_suffix(top), None, "must not relabel {top}");
            assert_eq!(
                relabel_suffix(&format!("{top}/")),
                None,
                "must not relabel {top}/ (trailing slash)"
            );
        }
    }

    #[test]
    fn does_not_skip_a_lookalike_subdirectory_or_sibling() {
        assert_eq!(relabel_suffix("/home2"), Some("z"));
        assert_eq!(relabel_suffix("/var-data"), Some("z"));
        assert_eq!(relabel_suffix("/opt/myapp/data"), Some("z"));
    }

    #[test]
    fn wsl_style_path_is_an_ordinary_linux_path() {
        // A WSL-translated Docker Desktop bind-mount source is a real
        // subdirectory under `/run`, not the bare top-level `/run` itself
        // — the exact-segment rule leaves it relabelable, same as any
        // other project path.
        assert_eq!(
            relabel_suffix("/run/desktop/mnt/host/wsl/docker-desktop-bind-mounts/x"),
            Some("z"),
        );
        assert_eq!(
            relabel_suffix("/run"),
            None,
            "the bare top-level path is skipped"
        );
        assert_eq!(relabel_suffix("/home/f/repo"), Some("z"));
    }
}
