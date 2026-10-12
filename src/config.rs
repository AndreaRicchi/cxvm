//! Global `config.toml` in the store: default toolchain and settings.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::files;
use crate::toolchain::Spec;

/// Contents of `config.toml`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Config {
    /// Global default toolchain, used outside projects.
    pub compiler: Option<Spec>,
}

/// On-disk form; unknown keys are rejected so typos fail loudly.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    compiler: Option<String>,
}

impl Config {
    /// Reads `path`; a missing file is an empty config.
    pub fn load(path: &Path) -> Result<Self> {
        let raw: RawConfig = files::read_toml(path, FIX_HINT)?.unwrap_or_default();
        let compiler = raw
            .compiler
            .map(|id| parse_compiler(path, &id))
            .transpose()?;
        Ok(Self { compiler })
    }

    /// Writes `path` atomically, creating its directory if needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        let raw = RawConfig {
            compiler: self.compiler.as_ref().map(ToString::to_string),
        };
        let body = toml::to_string(&raw).expect("a struct of strings serializes to TOML");
        files::write_atomic(path, &format!("{HEADER}{body}"))
    }
}

const HEADER: &str = "# cxvm global settings. `cxvm global <id>` updates `compiler`.\n";

const FIX_HINT: &str = "fix the file, or delete it to reset the global settings; \
                        the only key is `compiler = \"<id>\"`";

/// Parses the `compiler` value of a cxvm TOML file, reporting errors against the file.
pub(crate) fn parse_compiler(path: &Path, id: &str) -> Result<Spec> {
    id.parse().map_err(|err| match err {
        Error::InvalidId { input, reason } => Error::InvalidFile {
            path: path.to_owned(),
            message: format!("invalid toolchain id `{input}` in `compiler`: {reason}"),
            hint: "use `gcc@<version>`, `clang@<version>` or `system`, e.g. `gcc@13.3.0`".into(),
        },
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn store() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn load_str(content: &str) -> (tempfile::TempDir, Result<Config>) {
        let dir = store();
        let path = dir.path().join("config.toml");
        fs::write(&path, content).unwrap();
        let config = Config::load(&path);
        (dir, config)
    }

    #[test]
    fn missing_file_is_an_empty_config() {
        let dir = store();
        let config = Config::load(&dir.path().join("config.toml")).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn empty_file_is_an_empty_config() {
        let (_dir, config) = load_str("");
        assert_eq!(config.unwrap(), Config::default());
    }

    #[test]
    fn reads_the_compiler() {
        let (_dir, config) = load_str("compiler = \"gcc@13\"\n");
        assert_eq!(config.unwrap().compiler, Some("gcc@13".parse().unwrap()));
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = store();
        let path = dir.path().join("config.toml");
        for compiler in [Some("gcc@13.3.0"), Some("system"), None] {
            let config = Config {
                compiler: compiler.map(|c| c.parse().unwrap()),
            };
            config.save(&path).unwrap();
            assert_eq!(Config::load(&path).unwrap(), config);
        }
    }

    #[test]
    fn save_writes_the_same_key_as_cxvmrc() {
        let dir = store();
        let path = dir.path().join("config.toml");
        let config = Config {
            compiler: Some("clang@18.1.8".parse().unwrap()),
        };
        config.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("compiler = \"clang@18.1.8\""), "{text}");
    }

    #[test]
    fn save_creates_the_store_directory_and_leaves_no_temp_files() {
        let dir = store();
        let path = dir.path().join("new/store/config.toml");
        Config::default().save(&path).unwrap();
        let entries: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, ["config.toml"]);
    }

    #[test]
    fn unknown_keys_are_rejected_with_the_file_and_key() {
        let (dir, config) = load_str("complier = \"gcc@13\"\n");
        let err = config.unwrap_err().to_string();
        assert!(err.contains(&dir.path().display().to_string()), "{err}");
        assert!(err.contains("complier"), "{err}");
        assert!(err.contains("hint:"), "{err}");
    }

    #[test]
    fn invalid_ids_are_rejected_with_the_file_and_id() {
        let (dir, config) = load_str("compiler = \"gcc13\"\n");
        let err = config.unwrap_err().to_string();
        assert!(err.contains(&dir.path().display().to_string()), "{err}");
        assert!(err.contains("`gcc13`"), "{err}");
        assert!(err.contains("hint:"), "{err}");
    }

    #[test]
    fn syntax_errors_name_the_file() {
        let (dir, config) = load_str("compiler = \n");
        let err = config.unwrap_err().to_string();
        assert!(err.contains(&dir.path().display().to_string()), "{err}");
        assert!(err.contains("hint:"), "{err}");
    }

    #[test]
    fn unreadable_path_is_an_error_naming_it() {
        let dir = store();
        let path = dir.path().join("config.toml");
        fs::create_dir(&path).unwrap();
        let err = Config::load(&path).unwrap_err().to_string();
        assert!(err.contains(&path.display().to_string()), "{err}");
    }
}
