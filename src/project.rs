//! Per-project state: `.cxvmrc` discovery and parsing, the `.cxvm/` folder, and the
//! resolution order env var, nearest `.cxvmrc`, global default, `system` (M1, M2).

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use toml_edit::{DocumentMut, Item, Table, TableLike, Value};

use crate::config::{self, Config};
use crate::error::{Error, Result};
use crate::files;
use crate::output::debug;
use crate::toolchain::{Kind, Spec};

/// Name of the committed per-project pin file.
pub const RC_FILE: &str = ".cxvmrc";

/// Environment variable that overrides every other source.
pub const ENV_VAR: &str = "CXVM_COMPILER";

/// Contents of a `.cxvmrc`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRc {
    /// The pinned toolchain.
    pub compiler: Spec,
    /// `[clang] gcc`: the GCC whose headers and libstdc++ clang uses. `None`, also for
    /// `gcc = "system"`, leaves clang to pick a GCC on the host as it always does.
    pub clang_gcc: Option<Spec>,
    pub integrations: Integrations,
}

/// The `[integrations]` table: which files outside `.cxvm/` cxvm keeps up to date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Integrations {
    /// A configure preset per pinned toolchain in `CMakeUserPresets.json`.
    pub cmake_presets: bool,
    /// clangd and CMake Tools settings in `.vscode/settings.json`.
    pub vscode: bool,
    /// `.cxvm/` in `.gitignore`.
    pub gitignore: bool,
}

impl Default for Integrations {
    /// Only the `.gitignore` entry is on unless the project asks.
    fn default() -> Self {
        Self {
            cmake_presets: false,
            vscode: false,
            gitignore: true,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProjectRc {
    compiler: String,
    clang: Option<RawClang>,
    integrations: Option<RawIntegrations>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClang {
    gcc: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIntegrations {
    cmake_presets: Option<bool>,
    vscode: Option<bool>,
    gitignore: Option<bool>,
}

impl ProjectRc {
    /// Reads and parses the `.cxvmrc` at `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let text = files::read_text(path)?.ok_or_else(|| Error::Io {
            action: "read",
            path: path.to_owned(),
            source: std::io::ErrorKind::NotFound.into(),
            hint: "run `cxvm use <id>` to pin a toolchain here",
        })?;
        Self::parse(path, &text)
    }

    /// Parses `text`, the content of the `.cxvmrc` at `path`.
    fn parse(path: &Path, text: &str) -> Result<Self> {
        let raw: RawProjectRc = files::parse_toml(path, text, FIX_HINT)?;
        let compiler = config::parse_compiler(path, &raw.compiler)?;
        let clang_gcc = raw
            .clang
            .and_then(|clang| clang.gcc)
            .map(|id| parse_clang_gcc(path, &id))
            .transpose()?
            .filter(|spec| *spec != Spec::System);
        let defaults = Integrations::default();
        let raw = raw.integrations.unwrap_or_default();
        Ok(Self {
            compiler,
            clang_gcc,
            integrations: Integrations {
                cmake_presets: raw.cmake_presets.unwrap_or(defaults.cmake_presets),
                vscode: raw.vscode.unwrap_or(defaults.vscode),
                gitignore: raw.gitignore.unwrap_or(defaults.gitignore),
            },
        })
    }
}

/// Parses the value of `[clang] gcc`: a GCC id or `system`.
fn parse_clang_gcc(path: &Path, id: &str) -> Result<Spec> {
    let invalid = |reason: String| Error::InvalidFile {
        path: path.to_owned(),
        message: format!("invalid toolchain id `{id}` in `[clang] gcc`: {reason}"),
        hint: "name the GCC whose libstdc++ clang should use, e.g. `gcc = \"gcc@13.3.0\"`, \
               or `gcc = \"system\"` to let clang pick one on the host"
            .into(),
    };
    match id.parse() {
        Ok(Spec::Toolchain {
            kind: Kind::Clang, ..
        }) => Err(invalid("it must be a GCC, not a clang".into())),
        Ok(spec) => Ok(spec),
        Err(Error::InvalidId { reason, .. }) => Err(invalid(reason)),
        Err(other) => Err(other),
    }
}

const FIX_HINT: &str = "the file must contain `compiler = \"<id>\"`, e.g. \
                        `compiler = \"gcc@13.3.0\"`, and may have a `[clang]` table with `gcc` \
                        and an `[integrations]` table with `cmake_presets`, `vscode` and \
                        `gitignore`; fix it, or delete it and run `cxvm use <id>`";

/// A change to a `.cxvmrc` that has been checked but not written yet.
#[derive(Debug, Clone)]
pub struct PendingRc {
    path: PathBuf,
    text: String,
    /// The file's contents once written.
    pub rc: ProjectRc,
}

impl PendingRc {
    /// Replaces the file atomically.
    pub fn write(&self) -> Result<()> {
        files::write_atomic(&self.path, &self.text)
    }
}

/// Pins `compiler` in the `.cxvmrc` at `path`, and with `clang_gcc` sets `[clang] gcc` too.
/// Everything else in an existing file is kept as written, comments included. A file that
/// would still be invalid afterwards is an error.
pub fn edit_rc(path: &Path, compiler: &Spec, clang_gcc: Option<&Spec>) -> Result<PendingRc> {
    let text = files::read_text(path)?.unwrap_or_default();
    let mut doc: DocumentMut =
        text.parse()
            .map_err(|err: toml_edit::TomlError| Error::InvalidFile {
                path: path.to_owned(),
                message: err.to_string().trim_end().to_owned(),
                hint: FIX_HINT.to_owned(),
            })?;
    set_string(doc.as_table_mut(), "compiler", &compiler.to_string());
    if let Some(gcc) = clang_gcc {
        let clang = doc
            .entry("clang")
            .or_insert_with(|| Item::Table(Table::new()));
        // Anything but a table is reported by the check below.
        if let Some(clang) = clang.as_table_like_mut() {
            set_string(clang, "gcc", &gcc.to_string());
        }
    }
    let text = doc.to_string();
    let rc = ProjectRc::parse(path, &text)?;
    Ok(PendingRc {
        path: path.to_owned(),
        text,
        rc,
    })
}

/// [`edit_rc`], written at once.
pub fn write_rc(path: &Path, compiler: &Spec, clang_gcc: Option<&Spec>) -> Result<()> {
    edit_rc(path, compiler, clang_gcc)?.write()
}

/// Sets `key = "<value>"` in `table`; a value already there keeps its place and its comments.
fn set_string(table: &mut dyn TableLike, key: &str, value: &str) {
    let mut new = Value::from(value);
    match table.get_mut(key) {
        Some(Item::Value(old)) => {
            *new.decor_mut() = old.decor().clone();
            *old = new;
        }
        _ => {
            table.insert(key, Item::Value(new));
        }
    }
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
        debug!("{ENV_VAR}={value} decides the toolchain");
        return Ok(Resolution {
            spec,
            source: Source::Env,
        });
    }
    if let Some(rc) = find_rc(cwd) {
        let spec = ProjectRc::load(&rc)?.compiler;
        debug!("{} pins {spec}", rc.display());
        return Ok(Resolution {
            spec,
            source: Source::ProjectRc(rc),
        });
    }
    debug!("no {RC_FILE} in {} or its parents", cwd.display());
    if let Some(spec) = Config::load(config_path)?.compiler {
        debug!("{} sets the global default {spec}", config_path.display());
        return Ok(Resolution {
            spec,
            source: Source::Global(config_path.to_owned()),
        });
    }
    debug!("no global default in {}", config_path.display());
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
            write_rc(&rc, &spec(id), None).unwrap();
            assert_eq!(
                fs::read_to_string(&rc).unwrap(),
                format!("compiler = \"{id}\"\n")
            );
            assert_eq!(ProjectRc::load(&rc).unwrap().compiler, spec(id));
        }
    }

    #[test]
    fn a_plain_pin_has_no_pairing_and_only_the_gitignore_integration() {
        let f = fixture();
        let rc = f.pin("", "compiler = \"clang@18.1.8\"\n");
        let loaded = ProjectRc::load(&rc).unwrap();
        assert_eq!(loaded.clang_gcc, None);
        assert_eq!(
            loaded.integrations,
            Integrations {
                cmake_presets: false,
                vscode: false,
                gitignore: true
            }
        );
    }

    #[test]
    fn the_clang_and_integrations_tables_are_read() {
        let f = fixture();
        let rc = f.pin(
            "",
            "compiler = \"clang@18.1.8\"\n\n[clang]\ngcc = \"gcc@13\"\n\n\
             [integrations]\ncmake_presets = true\nvscode = true\ngitignore = false\n",
        );
        let loaded = ProjectRc::load(&rc).unwrap();
        assert_eq!(loaded.clang_gcc, Some(spec("gcc@13")));
        assert_eq!(
            loaded.integrations,
            Integrations {
                cmake_presets: true,
                vscode: true,
                gitignore: false
            }
        );
    }

    #[test]
    fn pairing_with_system_or_an_empty_table_is_no_pairing() {
        let f = fixture();
        for table in [
            "[clang]\ngcc = \"system\"\n",
            "[clang]\n",
            "[integrations]\n",
        ] {
            let rc = f.pin("", &format!("compiler = \"clang@18.1.8\"\n{table}"));
            let loaded = ProjectRc::load(&rc).unwrap();
            assert_eq!(loaded.clang_gcc, None, "{table:?}");
            assert_eq!(loaded.integrations, Integrations::default(), "{table:?}");
        }
    }

    #[test]
    fn invalid_tables_are_errors_naming_the_file_and_the_key() {
        let f = fixture();
        for (table, needle) in [
            ("[clang]\ngcc = \"clang@17\"\n", "must be a GCC"),
            ("[clang]\ngcc = \"gcc13\"\n", "`gcc13`"),
            ("[clang]\ngxx = \"gcc@13\"\n", "gxx"),
            ("[clang]\ngcc = 13\n", "gcc"),
            ("[integrations]\nvscode = \"yes\"\n", "vscode"),
            ("[integrations]\nvs_code = true\n", "vs_code"),
            ("[cmake]\npresets = true\n", "cmake"),
        ] {
            let rc = f.pin("", &format!("compiler = \"clang@18.1.8\"\n{table}"));
            let err = ProjectRc::load(&rc).unwrap_err().to_string();
            assert!(err.contains(&rc.display().to_string()), "{table:?}: {err}");
            assert!(err.contains(needle), "{table:?}: {err}");
            assert!(err.contains("hint:"), "{table:?}: {err}");
        }
    }

    #[test]
    fn pinning_keeps_the_rest_of_the_file_as_written() {
        let f = fixture();
        let rc = f.pin(
            "",
            "# Our compiler\ncompiler = \"gcc@13.3.0\"  # matches the cross toolchain\n\n\
             [integrations]\n# for the IDE\nvscode = true\n",
        );
        write_rc(&rc, &spec("clang@18.1.8"), None).unwrap();
        assert_eq!(
            fs::read_to_string(&rc).unwrap(),
            "# Our compiler\ncompiler = \"clang@18.1.8\"  # matches the cross toolchain\n\n\
             [integrations]\n# for the IDE\nvscode = true\n"
        );
    }

    #[test]
    fn pinning_with_a_pairing_adds_or_updates_the_clang_table() {
        let f = fixture();
        let rc = f.root.join(RC_FILE);
        write_rc(&rc, &spec("clang@18.1.8"), Some(&spec("gcc@13.3.0"))).unwrap();
        assert_eq!(
            fs::read_to_string(&rc).unwrap(),
            "compiler = \"clang@18.1.8\"\n\n[clang]\ngcc = \"gcc@13.3.0\"\n"
        );
        write_rc(&rc, &spec("clang@17.0.6"), Some(&Spec::System)).unwrap();
        assert_eq!(
            fs::read_to_string(&rc).unwrap(),
            "compiler = \"clang@17.0.6\"\n\n[clang]\ngcc = \"system\"\n"
        );
        // Without a pairing the table stays as it is.
        write_rc(&rc, &spec("clang@18.1.8"), Some(&spec("gcc@11.5.0"))).unwrap();
        write_rc(&rc, &spec("gcc@13.3.0"), None).unwrap();
        let loaded = ProjectRc::load(&rc).unwrap();
        assert_eq!(loaded.compiler, spec("gcc@13.3.0"));
        assert_eq!(loaded.clang_gcc, Some(spec("gcc@11.5.0")));
    }

    #[test]
    fn a_pin_is_added_before_existing_tables() {
        let f = fixture();
        let rc = f.pin("", "[integrations]\nvscode = true\n");
        write_rc(&rc, &spec("gcc@13.3.0"), None).unwrap();
        assert_eq!(
            fs::read_to_string(&rc).unwrap(),
            "compiler = \"gcc@13.3.0\"\n[integrations]\nvscode = true\n"
        );
    }

    #[test]
    fn pinning_repairs_an_invalid_compiler_but_nothing_else() {
        let f = fixture();
        let rc = f.pin("", "compiler = \"gcc13\"\n");
        write_rc(&rc, &spec("gcc@13.3.0"), None).unwrap();
        assert_eq!(ProjectRc::load(&rc).unwrap().compiler, spec("gcc@13.3.0"));

        for content in [
            "compiler = \"gcc@13.3.0\"\n[x",
            "compiler = \"gcc@13.3.0\"\ncolour = true\n",
            "compiler = \"gcc@13.3.0\"\nclang = \"gcc@13\"\n",
        ] {
            let rc = f.pin("", content);
            let err = write_rc(&rc, &spec("clang@18.1.8"), Some(&spec("gcc@13.3.0")))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains(&rc.display().to_string()),
                "{content:?}: {err}"
            );
            assert!(err.contains("hint:"), "{content:?}: {err}");
            assert_eq!(fs::read_to_string(&rc).unwrap(), content);
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
