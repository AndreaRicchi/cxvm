//! Per-project state: `.cxvmrc` discovery and parsing, the `.cxvm/` folder, and the
//! resolution order env var, nearest `.cxvmrc`, global default, `system` (M1, M2).

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::{self, Config};
use crate::error::{Error, Result};
use crate::files;
use crate::toolchain::Spec;

/// Name of the committed per-project pin file.
pub const RC_FILE: &str = ".cxvmrc";

/// Environment variable that overrides every other source.
pub const ENV_VAR: &str = "CXVM_COMPILER";

/// Contents of a `.cxvmrc`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRc {
    /// The pinned toolchain.
    pub compiler: Spec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProjectRc {
    compiler: String,
}

impl ProjectRc {
    /// Reads and parses the `.cxvmrc` at `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let raw: RawProjectRc = files::read_toml(path, FIX_HINT)?.ok_or_else(|| Error::Io {
            action: "read",
            path: path.to_owned(),
            source: std::io::ErrorKind::NotFound.into(),
            hint: "run `cxvm use <id>` to pin a toolchain here",
        })?;
        let compiler = config::parse_compiler(path, &raw.compiler)?;
        Ok(Self { compiler })
    }
}

const FIX_HINT: &str = "the file must contain `compiler = \"<id>\"`, e.g. \
                        `compiler = \"gcc@13.3.0\"`; fix it or run `cxvm use <id>`";

/// Writes `compiler = "<spec>"` to the `.cxvmrc` at `path`, replacing its content.
pub fn write_rc(path: &Path, spec: &Spec) -> Result<()> {
    files::write_atomic(path, &format!("compiler = \"{spec}\"\n"))
}

/// The project root for `cwd`: the directory of the nearest `.cxvmrc`, or `cwd` itself.
pub fn project_root(cwd: &Path) -> PathBuf {
    find_rc(cwd)
        .and_then(|rc| rc.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| cwd.to_owned())
}

/// The nearest `.cxvmrc` in `start` or one of its ancestors.
///
/// Anything named `.cxvmrc` counts, even a directory or a dangling symlink, so that a broken pin
/// is reported by [`ProjectRc::load`] instead of silently skipped.
pub fn find_rc(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|dir| dir.join(RC_FILE))
        .find(|path| path.symlink_metadata().is_ok())
}

/// Where the active toolchain came from, in resolution order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The `CXVM_COMPILER` environment variable.
    Env,
    /// The `.cxvmrc` at this path.
    ProjectRc(PathBuf),
    /// The global default in the `config.toml` at this path.
    Global(PathBuf),
    /// Nothing is configured, so `system` applies.
    Default,
}

impl fmt::Display for Source {
    /// A reason phrase such as `set by /p/.cxvmrc`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Env => write!(f, "set by {ENV_VAR}"),
            Source::ProjectRc(path) => write!(f, "set by {}", path.display()),
            Source::Global(path) => write!(f, "global default in {}", path.display()),
            Source::Default => write!(f, "no {RC_FILE} or global default"),
        }
    }
}

/// The toolchain that should be active, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// As written in the source; may be partial.
    pub spec: Spec,
    pub source: Source,
}

/// Applies the resolution order from PLAN.md §4: `env_compiler` (the value of
/// `CXVM_COMPILER`, empty meaning unset), the nearest `.cxvmrc` from `cwd`, the global default
/// in `config_path`, then `system`. The first source that is set wins, even if it names a
/// toolchain that is not installed.
pub fn resolve(cwd: &Path, env_compiler: Option<&str>, config_path: &Path) -> Result<Resolution> {
    if let Some(value) = env_compiler.filter(|value| !value.is_empty()) {
        let spec = value.parse().map_err(|err| match err {
            Error::InvalidId { input, reason } => Error::InvalidEnvCompiler { input, reason },
            other => other,
        })?;
        return Ok(Resolution {
            spec,
            source: Source::Env,
        });
    }
    if let Some(rc) = find_rc(cwd) {
        let spec = ProjectRc::load(&rc)?.compiler;
        return Ok(Resolution {
            spec,
            source: Source::ProjectRc(rc),
        });
    }
    if let Some(spec) = Config::load(config_path)?.compiler {
        return Ok(Resolution {
            spec,
            source: Source::Global(config_path.to_owned()),
        });
    }
    Ok(Resolution {
        spec: Spec::System,
        source: Source::Default,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        config: PathBuf,
    }

    /// A project tree `root/a/b/c` and an empty store next to it.
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        fs::create_dir_all(root.join("a/b/c")).unwrap();
        let config = dir.path().join("store/config.toml");
        Fixture {
            root,
            config,
            _dir: dir,
        }
    }

    impl Fixture {
        fn pin(&self, dir: &str, content: &str) -> PathBuf {
            let path = self.root.join(dir).join(RC_FILE);
            fs::write(&path, content).unwrap();
            path
        }

        fn global(&self, id: &str) {
            let config = Config {
                compiler: Some(id.parse().unwrap()),
            };
            config.save(&self.config).unwrap();
        }

        fn resolve(&self, dir: &str, env: Option<&str>) -> Result<Resolution> {
            resolve(&self.root.join(dir), env, &self.config)
        }
    }

    fn spec(s: &str) -> Spec {
        s.parse().unwrap()
    }

    #[test]
    fn nothing_configured_resolves_to_system() {
        let f = fixture();
        let r = f.resolve("a/b/c", None).unwrap();
        assert_eq!(r.spec, Spec::System);
        assert_eq!(r.source, Source::Default);
    }

    #[test]
    fn global_default_applies_outside_projects() {
        let f = fixture();
        f.global("gcc@13");
        let r = f.resolve("a", None).unwrap();
        assert_eq!(r.spec, spec("gcc@13"));
        assert_eq!(r.source, Source::Global(f.config.clone()));
    }

    #[test]
    fn cxvmrc_in_the_current_directory_applies() {
        let f = fixture();
        let rc = f.pin("", "compiler = \"clang@18.1.8\"\n");
        let r = f.resolve("", None).unwrap();
        assert_eq!(r.spec, spec("clang@18.1.8"));
        assert_eq!(r.source, Source::ProjectRc(rc));
    }

    #[test]
    fn cxvmrc_is_found_from_nested_directories() {
        let f = fixture();
        let rc = f.pin("", "compiler = \"gcc@13.3.0\"\n");
        let r = f.resolve("a/b/c", None).unwrap();
        assert_eq!(r.spec, spec("gcc@13.3.0"));
        assert_eq!(r.source, Source::ProjectRc(rc));
    }

    #[test]
    fn nearest_cxvmrc_wins() {
        let f = fixture();
        f.pin("", "compiler = \"gcc@13.3.0\"\n");
        let inner = f.pin("a", "compiler = \"clang@18.1.8\"\n");
        let r = f.resolve("a/b", None).unwrap();
        assert_eq!(r.spec, spec("clang@18.1.8"));
        assert_eq!(r.source, Source::ProjectRc(inner));
    }

    #[test]
    fn cxvmrc_wins_over_the_global_default() {
        let f = fixture();
        f.global("gcc@14.2.0");
        f.pin("", "compiler = \"system\"\n");
        let r = f.resolve("a", None).unwrap();
        assert_eq!(r.spec, Spec::System);
        assert!(matches!(r.source, Source::ProjectRc(_)));
    }

    #[test]
    fn env_var_wins_over_everything() {
        let f = fixture();
        f.global("gcc@14.2.0");
        f.pin("", "compiler = \"gcc@13.3.0\"\n");
        let r = f.resolve("a", Some("clang@18")).unwrap();
        assert_eq!(r.spec, spec("clang@18"));
        assert_eq!(r.source, Source::Env);
    }

    #[test]
    fn empty_env_var_counts_as_unset() {
        let f = fixture();
        f.global("gcc@14.2.0");
        let r = f.resolve("", Some("")).unwrap();
        assert_eq!(r.source, Source::Global(f.config.clone()));
    }

    #[test]
    fn invalid_env_var_is_an_error_naming_the_variable() {
        let f = fixture();
        let err = f.resolve("", Some("gcc13")).unwrap_err().to_string();
        assert!(err.contains(ENV_VAR), "{err}");
        assert!(err.contains("`gcc13`"), "{err}");
        assert!(err.contains("hint:"), "{err}");
    }

    #[test]
    fn a_broken_global_config_is_not_read_when_a_cxvmrc_applies() {
        let f = fixture();
        fs::create_dir_all(f.config.parent().unwrap()).unwrap();
        fs::write(&f.config, "not toml").unwrap();
        f.pin("", "compiler = \"gcc@13.3.0\"\n");
        assert!(f.resolve("a", None).is_ok());
        assert!(f.resolve("a", Some("gcc@13")).is_ok());
    }

    #[test]
    fn invalid_cxvmrc_is_an_error_naming_it() {
        let f = fixture();
        for content in [
            "compiler = \"gcc13\"\n",
            "complier = \"gcc@13\"\n",
            "",
            "[x",
        ] {
            let rc = f.pin("", content);
            let err = f.resolve("a/b", None).unwrap_err().to_string();
            assert!(
                err.contains(&rc.display().to_string()),
                "{content:?}: {err}"
            );
            assert!(err.contains("hint:"), "{content:?}: {err}");
        }
    }

    #[test]
    fn cxvmrc_that_is_a_directory_is_an_error_not_skipped() {
        let f = fixture();
        f.pin("", "compiler = \"gcc@13.3.0\"\n");
        let rc_dir = f.root.join("a").join(RC_FILE);
        fs::create_dir(&rc_dir).unwrap();
        let err = f.resolve("a/b", None).unwrap_err().to_string();
        assert!(err.contains(&rc_dir.display().to_string()), "{err}");
    }

    #[test]
    fn find_rc_returns_none_without_a_pin() {
        let f = fixture();
        assert_eq!(find_rc(&f.root.join("a/b/c")), None);
    }

    #[test]
    fn written_pins_read_back() {
        let f = fixture();
        let rc = f.root.join(RC_FILE);
        for id in ["gcc@13.3.0", "clang@18.1.8-local", "system"] {
            write_rc(&rc, &spec(id)).unwrap();
            assert_eq!(
                fs::read_to_string(&rc).unwrap(),
                format!("compiler = \"{id}\"\n")
            );
            assert_eq!(ProjectRc::load(&rc).unwrap().compiler, spec(id));
        }
    }

    #[test]
    fn project_root_is_the_nearest_pin_or_the_cwd() {
        let f = fixture();
        assert_eq!(project_root(&f.root.join("a/b")), f.root.join("a/b"));
        f.pin("", "compiler = \"gcc@13.3.0\"\n");
        assert_eq!(project_root(&f.root.join("a/b")), f.root);
        f.pin("a", "compiler = \"gcc@13.3.0\"\n");
        assert_eq!(project_root(&f.root.join("a/b")), f.root.join("a"));
    }

    #[test]
    fn sources_explain_themselves() {
        assert_eq!(Source::Env.to_string(), "set by CXVM_COMPILER");
        assert_eq!(
            Source::ProjectRc("/p/.cxvmrc".into()).to_string(),
            "set by /p/.cxvmrc"
        );
        assert_eq!(
            Source::Global("/s/config.toml".into()).to_string(),
            "global default in /s/config.toml"
        );
        assert_eq!(Source::Default.to_string(), "no .cxvmrc or global default");
    }
}
