//! One plugin, and where its files come from.
//!
//! A built-in plugin's files are `include_bytes!`-ed into the binary; an
//! installed one's are on disk. Every consumer of a contribution — the
//! icon-theme resolver first — needs to read a file the manifest named
//! without caring which, so [`LoadedPlugin::read_asset`] is the seam that
//! hides the difference. Nothing outside this module ever joins a plugin
//! directory to a manifest-supplied path itself.

use std::borrow::Cow;
use std::io;
use std::path::{Component, Path, PathBuf};

use plugin_api::{LoadErrorKind, PluginManifest};

/// Where a built-in plugin's materialised assets live, under the config
/// directory (jvm-build-tools plan A3, ADR-0057 §2).
const PLUGIN_ASSETS_DIR: &str = "plugin-assets";

/// Where a plugin came from.
///
/// The only thing that distinguishes a built-in: it is discovered,
/// validated and registered by the same code as an installed plugin, so
/// the path third parties take is the one exercised on every launch
/// (ADR-0026).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginSource {
    /// Embedded in the binary.
    Builtin,
    /// Read from `<config_dir>/plugins/<id>`.
    Installed,
}

/// A plugin compiled into the binary: its `plugin.toml` text and every
/// file a contribution may name, keyed by the path the manifest uses.
///
/// The manifest is kept as text rather than a pre-built [`PluginManifest`]
/// so a built-in goes through exactly the same parse and validation as an
/// installed one. A built-in with a broken manifest is therefore a load
/// error like any other and not a panic at startup.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinPlugin {
    /// The contents of `plugin.toml`.
    pub manifest: &'static str,
    /// `(relative path, bytes)`, using forward slashes as the manifest
    /// does. Order is irrelevant; lookup is linear because a plugin
    /// contributes a handful of files, not a filesystem.
    pub files: &'static [(&'static str, &'static [u8])],
}

/// Where one loaded plugin's files live.
///
/// A single field rather than a pair of `Option`s: a plugin has exactly
/// one file source, and an enum makes the impossible combinations
/// unrepresentable instead of merely unlikely.
#[derive(Debug, Clone)]
enum Assets {
    Builtin(&'static [(&'static str, &'static [u8])]),
    Installed(PathBuf),
}

/// A plugin that parsed, validated and was accepted into the registry.
#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    manifest: PluginManifest,
    assets: Assets,
}

impl LoadedPlugin {
    pub(crate) fn builtin(manifest: PluginManifest, builtin: &BuiltinPlugin) -> Self {
        Self {
            manifest,
            assets: Assets::Builtin(builtin.files),
        }
    }

    pub(crate) fn installed(manifest: PluginManifest, dir: PathBuf) -> Self {
        Self {
            manifest,
            assets: Assets::Installed(dir),
        }
    }

    /// The validated manifest.
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// The plugin's stable id — also its directory name when installed.
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn source(&self) -> PluginSource {
        match self.assets {
            Assets::Builtin(_) => PluginSource::Builtin,
            Assets::Installed(_) => PluginSource::Installed,
        }
    }

    /// The plugin's directory, or `None` for a built-in, which has no
    /// files on disk to point at.
    pub fn dir(&self) -> Option<&Path> {
        match &self.assets {
            Assets::Builtin(_) => None,
            Assets::Installed(dir) => Some(dir),
        }
    }

    /// Read one file the manifest named, relative to the plugin.
    ///
    /// `relative` is re-validated here even though `plugin-api` already
    /// rejected an unsafe path when the manifest was parsed. That is
    /// deliberate duplication: this is the one function in the crate that
    /// turns a string out of a config file into a filesystem read, and the
    /// caller need not be a manifest field — P3's resolver composes paths
    /// out of pack contents too. A check that only ran at parse time would
    /// protect the arguments nobody worries about and none of the ones
    /// that arrive later.
    ///
    /// The disk case adds a check `plugin-api` structurally cannot make:
    /// after resolving symlinks, the file must still sit inside the plugin
    /// directory. A relative path with no `..` in it can still leave the
    /// directory by pointing at a symlink, and only a filesystem knows
    /// that.
    pub fn read_asset(&self, relative: &Path) -> Result<Cow<'_, [u8]>, LoadErrorKind> {
        check_relative(relative)?;
        match &self.assets {
            Assets::Builtin(files) => {
                // Manifests spell paths with forward slashes; on Windows
                // `Path` would render the same value with backslashes, so
                // the lookup key is rebuilt from components rather than
                // taken from `Path::display`.
                let key = relative
                    .components()
                    .filter_map(|c| match c {
                        Component::Normal(part) => Some(part.to_string_lossy()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("/");
                files
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, bytes)| Cow::Borrowed(*bytes))
                    .ok_or_else(|| LoadErrorKind::Unreadable {
                        file: key,
                        message: "no such file in this built-in plugin".to_string(),
                    })
            }
            Assets::Installed(dir) => {
                let path = dir.join(relative);
                let unreadable = |err: std::io::Error| LoadErrorKind::Unreadable {
                    file: path.display().to_string(),
                    message: err.to_string(),
                };
                let resolved = path.canonicalize().map_err(unreadable)?;
                // The directory is canonicalised on every read rather than
                // cached at load time: a plugin directory can be replaced
                // underneath a running editor, and a stale prefix would
                // either refuse a legitimate read or accept an escaped one.
                let root = dir.canonicalize().map_err(unreadable)?;
                if !resolved.starts_with(&root) {
                    return Err(LoadErrorKind::UnsafePath {
                        field: "asset",
                        value: relative.display().to_string(),
                    });
                }
                std::fs::read(&resolved).map(Cow::Owned).map_err(unreadable)
            }
        }
    }

    /// A real directory on disk this plugin's files can be found in.
    ///
    /// For an installed plugin, its own directory — already on disk. For a
    /// built-in, `<config_dir>/plugin-assets/<id>/`: every one of
    /// [`Self::manifest`]'s [`BuiltinPlugin::files`] is written there, but
    /// only when missing or when its content differs from what is already
    /// on disk (a byte compare, not a hash — a builtin's files are few and
    /// small, so a fresh library dependency buys nothing here).
    ///
    /// Materialising happens on demand rather than at [`load`]/`reload`
    /// time, because `load` runs on every settings-page scan and most
    /// built-ins (an icon pack, a preview renderer) never need a real path
    /// at all — only a consumer that must hand the file's *path* to an
    /// external process (`gradlew --init-script <path>`) does.
    pub fn asset_dir(&self, config_dir: &Path) -> io::Result<PathBuf> {
        match &self.assets {
            Assets::Installed(dir) => Ok(dir.clone()),
            Assets::Builtin(files) => {
                let dir = config_dir.join(PLUGIN_ASSETS_DIR).join(self.id());
                std::fs::create_dir_all(&dir)?;
                for (name, bytes) in files.iter() {
                    let path = dir.join(name);
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let up_to_date = std::fs::read(&path)
                        .map(|existing| existing == *bytes)
                        .unwrap_or(false);
                    if !up_to_date {
                        std::fs::write(&path, bytes)?;
                    }
                }
                Ok(dir)
            }
        }
    }
}

/// Replace every `${asset_dir}` token in `args` with `dir`, joined the same
/// way [`plugin_api::expand_capability_path`] joins `${plugin_dir}` —
/// [`LoadedPlugin::asset_dir`] is what a caller hands as `dir`.
///
/// A whole-argument token (`"${asset_dir}"` alone) becomes `dir` itself;
/// a token used as a path prefix (`"${asset_dir}/ide-model.init.gradle"`)
/// is joined onto it. Neither shape needs the substring case a capability
/// path allows, because a manifest's `args` are whole argv entries, not
/// arbitrary strings a token might appear in the middle of.
pub fn expand_asset_dir(args: &[String], dir: &Path) -> Vec<String> {
    const TOKEN: &str = "${asset_dir}";
    args.iter()
        .map(|arg| {
            if arg == TOKEN {
                dir.display().to_string()
            } else if let Some(rest) = arg.strip_prefix(TOKEN) {
                dir.join(rest.trim_start_matches(['/', '\\']))
                    .display()
                    .to_string()
            } else {
                arg.clone()
            }
        })
        .collect()
}

/// Reject anything that is not a plain relative path.
///
/// Kept in step with `plugin-api`'s manifest-time rule on purpose; see
/// [`LoadedPlugin::read_asset`] for why it runs twice.
fn check_relative(relative: &Path) -> Result<(), LoadErrorKind> {
    let unsafe_path = || LoadErrorKind::UnsafePath {
        field: "asset",
        value: relative.display().to_string(),
    };
    if relative.as_os_str().is_empty() || relative.is_absolute() {
        return Err(unsafe_path());
    }
    for component in relative.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(unsafe_path()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
        id = "jvm-build-tools"
        name = "Gradle and Maven"
        version = "1.0.0"
        api_version = 1
    "#;

    fn builtin_plugin() -> LoadedPlugin {
        let manifest = PluginManifest::from_toml_str(MANIFEST).expect("valid");
        let builtin = BuiltinPlugin {
            manifest: MANIFEST,
            files: &[("ide-model.init.gradle", b"// init script\n")],
        };
        LoadedPlugin::builtin(manifest, &builtin)
    }

    #[test]
    fn a_builtin_asset_dir_materialises_its_files() {
        let plugin = builtin_plugin();
        let config_dir = tempfile::tempdir().unwrap();
        let dir = plugin.asset_dir(config_dir.path()).unwrap();
        assert_eq!(
            dir,
            config_dir
                .path()
                .join("plugin-assets")
                .join("jvm-build-tools")
        );
        let contents = std::fs::read(dir.join("ide-model.init.gradle")).unwrap();
        assert_eq!(contents, b"// init script\n");
    }

    #[test]
    fn materialising_twice_does_not_rewrite_unchanged_content() {
        let plugin = builtin_plugin();
        let config_dir = tempfile::tempdir().unwrap();
        let dir = plugin.asset_dir(config_dir.path()).unwrap();
        let asset = dir.join("ide-model.init.gradle");
        let before = std::fs::metadata(&asset).unwrap().modified().unwrap();

        // A filesystem's mtime resolution can be coarser than this test's
        // wall-clock gap, so the proof is content-based, not time-based:
        // hand-edit the file, then confirm a second `asset_dir()` call
        // restores the shipped content rather than leaving the edit alone.
        std::fs::write(&asset, b"tampered\n").unwrap();
        let dir_again = plugin.asset_dir(config_dir.path()).unwrap();
        assert_eq!(dir_again, dir);
        assert_eq!(std::fs::read(&asset).unwrap(), b"// init script\n");
        let _ = before;
    }

    #[test]
    fn an_installed_plugins_asset_dir_is_its_own_directory() {
        let manifest = PluginManifest::from_toml_str(MANIFEST).expect("valid");
        let dir = tempfile::tempdir().unwrap();
        let plugin = LoadedPlugin::installed(manifest, dir.path().to_path_buf());
        assert_eq!(plugin.asset_dir(Path::new("/unused")).unwrap(), dir.path());
    }

    #[test]
    fn expand_asset_dir_replaces_a_whole_argument_token() {
        let dir = Path::new("/home/u/.config/ide/plugin-assets/jvm-build-tools");
        let args = vec!["${asset_dir}".to_string(), "-q".to_string()];
        let expanded = expand_asset_dir(&args, dir);
        assert_eq!(expanded[0], dir.display().to_string());
        assert_eq!(expanded[1], "-q");
    }

    #[test]
    fn expand_asset_dir_joins_a_prefixed_token() {
        let dir = Path::new("/home/u/.config/ide/plugin-assets/jvm-build-tools");
        let args = vec!["${asset_dir}/ide-model.init.gradle".to_string()];
        let expanded = expand_asset_dir(&args, dir);
        assert_eq!(
            expanded[0],
            dir.join("ide-model.init.gradle").display().to_string()
        );
    }

    #[test]
    fn expand_asset_dir_leaves_unrelated_arguments_alone() {
        let dir = Path::new("/tmp/x");
        let args = vec!["--console=plain".to_string()];
        assert_eq!(expand_asset_dir(&args, dir), args);
    }
}
