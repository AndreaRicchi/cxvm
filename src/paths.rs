//! Store, cache and lock locations from `CXVM_HOME` and the XDG variables.
//!
//! Environment lookups go through a closure so tests never modify the process environment.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::toolchain::ToolchainId;

/// Where cxvm keeps its data (the store) and its download cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    data: PathBuf,
    cache: PathBuf,
}

impl Paths {
    /// Resolves the locations from `CXVM_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME` and `HOME`,
    /// read through `var`. Empty variables count as unset.
    pub fn from_env(var: impl Fn(&str) -> Option<OsString>) -> Result<Self> {
        let var = |name: &str| {
            var(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        // XDG base directories must be absolute; relative ones are ignored per the spec.
        let xdg = |name: &str| var(name).filter(|path| path.is_absolute());
        let home = || {
            var("HOME")
                .filter(|path| path.is_absolute())
                .ok_or(Error::NoHome)
        };

        let data = match var("CXVM_HOME") {
            Some(path) if path.is_absolute() => path,
            Some(path) => return Err(Error::RelativeCxvmHome { path }),
            None => match xdg("XDG_DATA_HOME") {
                Some(dir) => dir.join("cxvm"),
                None => home()?.join(".local/share/cxvm"),
            },
        };
        let cache = match xdg("XDG_CACHE_HOME") {
            Some(dir) => dir.join("cxvm"),
            None => home()?.join(".cache/cxvm"),
        };
        Ok(Self { data, cache })
    }

    /// The process environment version of [`Paths::from_env`].
    pub fn from_process_env() -> Result<Self> {
        Self::from_env(|name| std::env::var_os(name))
    }

    /// Store root: `$CXVM_HOME`, else `$XDG_DATA_HOME/cxvm`, else `~/.local/share/cxvm`.
    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    /// Cache root: `$XDG_CACHE_HOME/cxvm`, else `~/.cache/cxvm`.
    pub fn cache_dir(&self) -> &Path {
        &self.cache
    }

    pub fn toolchains_dir(&self) -> PathBuf {
        self.data.join("toolchains")
    }

    /// `toolchains/<kind>/<version>`.
    pub fn toolchain_dir(&self, id: &ToolchainId) -> PathBuf {
        self.toolchains_dir()
            .join(id.kind().as_str())
            .join(id.version().to_string())
    }

    pub fn config_file(&self) -> PathBuf {
        self.data.join("config.toml")
    }

    pub fn links_file(&self) -> PathBuf {
        self.data.join("links.toml")
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.data.join("staging")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.data.join("lock")
    }

    pub fn downloads_dir(&self) -> PathBuf {
        self.cache.join("downloads")
    }

    /// Logs of builds from source.
    pub fn logs_dir(&self) -> PathBuf {
        self.cache.join("logs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(vars: &[(&str, &str)]) -> Result<Paths> {
        let vars: Vec<(String, OsString)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        Paths::from_env(|name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
    }

    #[test]
    fn defaults_to_home_locations() {
        let p = paths(&[("HOME", "/home/me")]).unwrap();
        assert_eq!(p.data_dir(), Path::new("/home/me/.local/share/cxvm"));
        assert_eq!(p.cache_dir(), Path::new("/home/me/.cache/cxvm"));
    }

    #[test]
    fn xdg_variables_override_home() {
        let p = paths(&[
            ("HOME", "/home/me"),
            ("XDG_DATA_HOME", "/data"),
            ("XDG_CACHE_HOME", "/cache"),
        ])
        .unwrap();
        assert_eq!(p.data_dir(), Path::new("/data/cxvm"));
        assert_eq!(p.cache_dir(), Path::new("/cache/cxvm"));
    }

    #[test]
    fn cxvm_home_overrides_the_data_dir_only() {
        let p = paths(&[
            ("HOME", "/home/me"),
            ("XDG_DATA_HOME", "/data"),
            ("CXVM_HOME", "/opt/cxvm"),
        ])
        .unwrap();
        assert_eq!(p.data_dir(), Path::new("/opt/cxvm"));
        assert_eq!(p.cache_dir(), Path::new("/home/me/.cache/cxvm"));
    }

    #[test]
    fn relative_xdg_variables_are_ignored() {
        let p = paths(&[
            ("HOME", "/home/me"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "./cache"),
        ])
        .unwrap();
        assert_eq!(p.data_dir(), Path::new("/home/me/.local/share/cxvm"));
        assert_eq!(p.cache_dir(), Path::new("/home/me/.cache/cxvm"));
    }

    #[test]
    fn empty_variables_count_as_unset() {
        let p = paths(&[
            ("HOME", "/home/me"),
            ("CXVM_HOME", ""),
            ("XDG_DATA_HOME", ""),
            ("XDG_CACHE_HOME", ""),
        ])
        .unwrap();
        assert_eq!(p.data_dir(), Path::new("/home/me/.local/share/cxvm"));
        assert_eq!(p.cache_dir(), Path::new("/home/me/.cache/cxvm"));
    }

    #[test]
    fn relative_cxvm_home_is_an_error() {
        let err = paths(&[("HOME", "/home/me"), ("CXVM_HOME", "store")])
            .unwrap_err()
            .to_string();
        assert!(err.contains("CXVM_HOME"), "{err}");
        assert!(err.contains("`store`"), "{err}");
        assert!(err.contains("absolute"), "{err}");
        assert!(err.contains("hint:"), "{err}");
    }

    #[test]
    fn missing_home_is_an_error_naming_the_variables_to_set() {
        let err = paths(&[]).unwrap_err().to_string();
        assert!(err.contains("HOME"), "{err}");
        assert!(err.contains("hint:"), "{err}");
    }

    #[test]
    fn relative_home_is_an_error() {
        assert!(paths(&[("HOME", "me")]).is_err());
    }

    #[test]
    fn full_overrides_need_no_home() {
        let p = paths(&[("CXVM_HOME", "/s"), ("XDG_CACHE_HOME", "/c")]).unwrap();
        assert_eq!(p.data_dir(), Path::new("/s"));
        assert_eq!(p.cache_dir(), Path::new("/c/cxvm"));
    }

    #[test]
    fn store_layout_follows_plan() {
        let p = paths(&[("CXVM_HOME", "/s"), ("XDG_CACHE_HOME", "/c")]).unwrap();
        let id: ToolchainId = "gcc@13.3.0".parse().unwrap();
        assert_eq!(p.toolchains_dir(), Path::new("/s/toolchains"));
        assert_eq!(p.toolchain_dir(&id), Path::new("/s/toolchains/gcc/13.3.0"));
        assert_eq!(p.config_file(), Path::new("/s/config.toml"));
        assert_eq!(p.links_file(), Path::new("/s/links.toml"));
        assert_eq!(p.staging_dir(), Path::new("/s/staging"));
        assert_eq!(p.lock_file(), Path::new("/s/lock"));
        assert_eq!(p.downloads_dir(), Path::new("/c/cxvm/downloads"));
    }
}
