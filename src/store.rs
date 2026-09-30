//! The shared toolchain store: installed toolchains, links, staging and locking (M1, M3).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::toolchain::{Kind, Spec, ToolchainId, Version};

/// A toolchain present in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub id: ToolchainId,
    /// Its prefix, `toolchains/<kind>/<version>`.
    pub path: PathBuf,
}

/// Read access to the toolchains in the store.
#[derive(Debug, Clone)]
pub struct Store {
    paths: Paths,
}

impl Store {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// Every toolchain in `toolchains/<kind>/<version>/`, sorted by kind then version.
    ///
    /// Entries that are not directories or whose names are not exact versions are skipped;
    /// `cxvm doctor` reports them (M7).
    pub fn installed(&self) -> Result<Vec<Installed>> {
        let mut installed = Vec::new();
        for kind in Kind::ALL {
            let kind_dir = self.paths.toolchains_dir().join(kind.as_str());
            for entry in read_dir(&kind_dir)? {
                let path = entry.path();
                let id = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<Version>().ok())
                    .and_then(|version| ToolchainId::new(kind, version));
                if let Some(id) = id
                    && path.is_dir()
                {
                    installed.push(Installed { id, path });
                }
            }
        }
        installed.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(installed)
    }

    /// The highest installed toolchain matching `spec`; `None` for `system` or no match.
    pub fn find(&self, spec: &Spec) -> Result<Option<Installed>> {
        let installed = self.installed()?;
        let Some(id) = spec.select(installed.iter().map(|t| &t.id)) else {
            return Ok(None);
        };
        Ok(installed.iter().find(|t| &t.id == id).cloned())
    }
}

/// The entries of `dir`; none if it is missing or not a directory.
fn read_dir(dir: &Path) -> Result<Vec<fs::DirEntry>> {
    let io_error = |source| Error::Io {
        action: "read",
        path: dir.to_owned(),
        source,
        hint: "check the permissions of the cxvm store",
    };
    match fs::read_dir(dir) {
        Ok(entries) => entries.collect::<io::Result<_>>().map_err(io_error),
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(Vec::new())
        }
        Err(err) => Err(io_error(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path) -> Store {
        let root = dir.to_owned();
        Store::new(
            Paths::from_env(|name| match name {
                "CXVM_HOME" => Some(root.join("store").into()),
                "XDG_CACHE_HOME" => Some(root.join("cache").into()),
                _ => None,
            })
            .unwrap(),
        )
    }

    fn add(store: &Store, rel: &str) -> PathBuf {
        let path = store.paths().toolchains_dir().join(rel);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn ids(installed: &[Installed]) -> Vec<String> {
        installed.iter().map(|t| t.id.to_string()).collect()
    }

    #[test]
    fn missing_store_has_no_toolchains() {
        let dir = tempfile::tempdir().unwrap();
        assert!(store(dir.path()).installed().unwrap().is_empty());
    }

    #[test]
    fn lists_toolchains_sorted_with_their_paths() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        add(&s, "clang/18.1.8");
        add(&s, "gcc/14.2.0");
        let gcc13 = add(&s, "gcc/13.3.0");
        add(&s, "gcc/11.4.0-local");
        let installed = s.installed().unwrap();
        assert_eq!(
            ids(&installed),
            [
                "gcc@11.4.0-local",
                "gcc@13.3.0",
                "gcc@14.2.0",
                "clang@18.1.8"
            ]
        );
        assert_eq!(installed[1].path, gcc13);
    }

    #[test]
    fn skips_entries_that_are_not_toolchains() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        add(&s, "gcc/13.3.0");
        add(&s, "gcc/13");
        add(&s, "gcc/latest");
        add(&s, "gcc/.13.3.1.tmp");
        add(&s, "msvc/19.0.0");
        fs::write(s.paths().toolchains_dir().join("gcc/14.2.0"), "").unwrap();
        fs::write(s.paths().toolchains_dir().join("clang"), "").unwrap();
        assert_eq!(ids(&s.installed().unwrap()), ["gcc@13.3.0"]);
    }

    #[test]
    fn find_selects_the_highest_match() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        add(&s, "gcc/13.2.0");
        let best = add(&s, "gcc/13.3.0");
        add(&s, "gcc/14.2.0");
        let found = s.find(&"gcc@13".parse().unwrap()).unwrap().unwrap();
        assert_eq!(found.id.to_string(), "gcc@13.3.0");
        assert_eq!(found.path, best);
        assert_eq!(s.find(&"gcc@15".parse().unwrap()).unwrap(), None);
        assert_eq!(s.find(&Spec::System).unwrap(), None);
    }
}
