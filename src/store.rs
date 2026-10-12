//! The shared toolchain store: installed toolchains, links, staging and locking (M1, M3).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::detect;
use crate::error::{Error, Result};
use crate::files;
use crate::output::{self, debug};
use crate::paths::Paths;
use crate::toolchain::{Kind, Spec, ToolchainId, Version};

/// A toolchain cxvm can use: installed in the store or registered with `cxvm link`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub id: ToolchainId,
    /// Its prefix: `toolchains/<kind>/<version>` in the store, or the linked directory.
    pub path: PathBuf,
    /// C driver, e.g. `<prefix>/bin/gcc`.
    pub cc: PathBuf,
    /// C++ driver, e.g. `<prefix>/bin/g++`.
    pub cxx: PathBuf,
    /// Registered with `cxvm link` rather than installed in the store.
    pub linked: bool,
}

/// A toolchain registered with `cxvm link`, as recorded in `links.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub id: ToolchainId,
    pub prefix: PathBuf,
    pub cc: PathBuf,
    pub cxx: PathBuf,
    /// First line of `cc --version` when it was linked.
    pub version: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawLinks {
    #[serde(default, rename = "toolchain", skip_serializing_if = "Vec::is_empty")]
    toolchains: Vec<RawLink>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawLink {
    id: String,
    prefix: PathBuf,
    cc: PathBuf,
    cxx: PathBuf,
    version: String,
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

    /// Every toolchain in the store and in `links.toml`, sorted by kind then version.
    ///
    /// Store entries that are not directories or whose names are not exact versions are
    /// skipped, and so is a link whose id a store entry already uses; `cxvm doctor` reports
    /// them (M7).
    pub fn installed(&self) -> Result<Vec<Installed>> {
        let mut installed = self.store_entries()?;
        for link in self.links()? {
            if installed.iter().any(|t| t.id == link.id) {
                continue;
            }
            installed.push(Installed {
                id: link.id,
                path: link.prefix,
                cc: link.cc,
                cxx: link.cxx,
                linked: true,
            });
        }
        installed.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(installed)
    }

    /// The toolchains registered in `links.toml`.
    pub fn links(&self) -> Result<Vec<Link>> {
        let path = self.paths.links_file();
        let raw: RawLinks = files::read_toml(&path, LINKS_HINT)?.unwrap_or_default();
        let invalid = |message: String| Error::InvalidFile {
            path: path.clone(),
            message,
            hint: LINKS_HINT.into(),
        };
        let mut links: Vec<Link> = Vec::with_capacity(raw.toolchains.len());
        for entry in raw.toolchains {
            let id: ToolchainId = entry.id.parse().map_err(|err| match err {
                Error::InvalidId { input, reason } => {
                    invalid(format!("invalid toolchain id `{input}`: {reason}"))
                }
                other => other,
            })?;
            if links.iter().any(|link| link.id == id) {
                return Err(invalid(format!("`{id}` is listed twice")));
            }
            links.push(Link {
                id,
                prefix: entry.prefix,
                cc: entry.cc,
                cxx: entry.cxx,
                version: entry.version,
            });
        }
        Ok(links)
    }

    /// Replaces `links.toml` with `links`, sorted by id.
    pub fn save_links(&self, links: &[Link]) -> Result<()> {
        let mut sorted: Vec<&Link> = links.iter().collect();
        sorted.sort_by(|a, b| a.id.cmp(&b.id));
        let toolchains: Vec<RawLink> = sorted
            .into_iter()
            .map(|link| RawLink {
                id: link.id.to_string(),
                prefix: link.prefix.clone(),
                cc: link.cc.clone(),
                cxx: link.cxx.clone(),
                version: link.version.clone(),
            })
            .collect();
        let path = self.paths.links_file();
        let body = toml::to_string(&RawLinks { toolchains }).map_err(|err| Error::InvalidFile {
            path: path.clone(),
            message: format!("cannot encode the links: {err}"),
            hint: "link only prefixes whose paths are valid UTF-8".into(),
        })?;
        files::write_atomic(&path, &format!("{LINKS_HEADER}{body}"))
    }

    /// Toolchains in `toolchains/<kind>/<version>/`.
    fn store_entries(&self) -> Result<Vec<Installed>> {
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
                    let (cc, cxx) = detect::compilers(&path, kind);
                    installed.push(Installed {
                        id,
                        path,
                        cc,
                        cxx,
                        linked: false,
                    });
                }
            }
        }
        Ok(installed)
    }

    /// Takes the store lock, waiting (with a message) while another cxvm process holds it,
    /// then deletes what interrupted installs and removals left in `staging/`. Every store
    /// mutation (install, remove, link, unlink) runs under the lock.
    pub fn lock(&self) -> Result<StoreLock> {
        let path = self.paths.lock_file();
        let io_error = |action, path: &Path| {
            let path = path.to_owned();
            move |source| Error::Io {
                action,
                path,
                source,
                hint: "check that the cxvm store is writable",
            }
        };
        fs::create_dir_all(self.paths.data_dir())
            .map_err(io_error("create", self.paths.data_dir()))?;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(io_error("open", &path))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => {
                eprintln!(
                    "waiting for another cxvm process (lock: {})",
                    path.display()
                );
                file.lock().map_err(io_error("lock", &path))?;
            }
            Err(fs::TryLockError::Error(source)) => return Err(io_error("lock", &path)(source)),
        }
        debug!("holding the store lock {}", path.display());
        let lock = StoreLock { _file: file };
        self.clean_staging(&lock)?;
        Ok(lock)
    }

    /// Deletes everything in `staging/`. Only safe under the lock: nothing else is in flight.
    fn clean_staging(&self, _lock: &StoreLock) -> Result<()> {
        let staging = self.paths.staging_dir();
        for entry in read_dir(&staging)? {
            let path = entry.path();
            output::note(format!(
                "deleting {}, left by an interrupted install or removal",
                path.display()
            ));
            remove_all(&path)?;
        }
        Ok(())
    }

    /// A fresh path in `staging/` for this process, named after `what`.
    pub fn staging_path(&self, what: &str) -> PathBuf {
        self.paths
            .staging_dir()
            .join(format!("{what}.{}", std::process::id()))
    }

    /// Moves the prefix prepared in `staged` into the store as `id`, atomically.
    pub fn commit(&self, _lock: &StoreLock, staged: &Path, id: &ToolchainId) -> Result<PathBuf> {
        let target = self.paths.toolchain_dir(id);
        if target.exists() {
            return Err(Error::Usage {
                message: format!("{id} is already installed in {}", target.display()),
                hint: format!("remove it first with `cxvm remove {id}`"),
            });
        }
        let parent = target.parent().expect("toolchain dirs have a parent");
        let io_error = |action, path: &Path| {
            let path = path.to_owned();
            move |source| Error::Io {
                action,
                path,
                source,
                hint: "check the permissions of the cxvm store",
            }
        };
        fs::create_dir_all(parent).map_err(io_error("create", parent))?;
        fs::rename(staged, &target).map_err(io_error("write", &target))?;
        Ok(target)
    }

    /// Deletes the installed toolchain `id`. It is first moved into `staging/`, so an
    /// interruption never leaves a half-deleted toolchain in `toolchains/`.
    pub fn remove(&self, _lock: &StoreLock, id: &ToolchainId) -> Result<()> {
        let path = self.paths.toolchain_dir(id);
        let trash = self.staging_path(&format!("removing-{}-{}", id.kind(), id.version()));
        let io_error = |action, path: &Path| {
            let path = path.to_owned();
            move |source| Error::Io {
                action,
                path,
                source,
                hint: "check the permissions of the cxvm store",
            }
        };
        let staging = self.paths.staging_dir();
        fs::create_dir_all(&staging).map_err(io_error("create", &staging))?;
        fs::rename(&path, &trash).map_err(io_error("move", &path))?;
        remove_all(&trash)
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

/// Name of the metadata file in each installed toolchain's prefix.
pub const META_FILE: &str = ".cxvm-meta.toml";

/// What `.cxvm-meta.toml` records about a toolchain cxvm installed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub id: String,
    /// Backend that installed it, e.g. `llvm-prebuilt`.
    pub backend: String,
    /// Archive it came from, and the archive's sha256.
    pub url: String,
    pub sha256: String,
    /// Install time, UTC, RFC 3339.
    pub installed: String,
    pub cc: PathBuf,
    pub cxx: PathBuf,
    /// First line of `cc --version`.
    pub version: String,
    /// GCC only: the directory holding its `libstdc++.so.6` (`lib64` or `lib`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub libstdcxx_dir: Option<PathBuf>,
    /// GCC only: its install dir, `lib/gcc/<triple>/<version>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gcc_install_dir: Option<PathBuf>,
    /// Built from source only: first line of `--version` of the compiler it was built with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_compiler: Option<String>,
    /// Built from source only: the arguments `configure` was run with.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub configure_args: Vec<String>,
}

impl Meta {
    /// Writes `prefix/.cxvm-meta.toml`.
    pub fn write(&self, prefix: &Path) -> Result<()> {
        let path = prefix.join(META_FILE);
        let body = toml::to_string(self).map_err(|err| Error::InvalidFile {
            path: path.clone(),
            message: format!("cannot encode the metadata: {err}"),
            hint: "install into a store whose path is valid UTF-8".into(),
        })?;
        files::write_atomic(
            &path,
            &format!("# Written by cxvm when it installed this toolchain.\n{body}"),
        )
    }

    /// Reads `prefix/.cxvm-meta.toml`; `None` if there is none.
    pub fn load(prefix: &Path) -> Result<Option<Self>> {
        files::read_toml(
            &prefix.join(META_FILE),
            "reinstall the toolchain with `cxvm remove` and `cxvm install`",
        )
    }
}

/// `time` as an RFC 3339 UTC timestamp, e.g. `2026-10-03T14:05:09Z`.
pub fn utc_timestamp(time: std::time::SystemTime) -> String {
    let secs = time
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Exclusive hold on the store, released when dropped.
#[derive(Debug)]
pub struct StoreLock {
    _file: fs::File,
}

/// Deletes `path`, a directory tree or a file; a missing path is fine.
pub(crate) fn remove_all(path: &Path) -> Result<()> {
    let result = match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(err) => Err(err),
    };
    match result {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(Error::Io {
            action: "delete",
            path: path.to_owned(),
            source: err,
            hint: "check the permissions of the cxvm store, or delete it by hand",
        }),
        _ => Ok(()),
    }
}

const LINKS_HEADER: &str =
    "# Toolchains registered with `cxvm link`. Change them with `cxvm link` / `cxvm unlink`.\n";

const LINKS_HINT: &str = "fix the entry, or delete the file and register the toolchains again \
                          with `cxvm link <prefix>`";

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

    fn link(id: &str, prefix: &str) -> Link {
        Link {
            id: id.parse().unwrap(),
            prefix: prefix.into(),
            cc: format!("{prefix}/bin/cc").into(),
            cxx: format!("{prefix}/bin/c++").into(),
            version: format!("{id} (fake)"),
        }
    }

    #[test]
    fn no_links_file_means_no_links() {
        let dir = tempfile::tempdir().unwrap();
        assert!(store(dir.path()).links().unwrap().is_empty());
    }

    #[test]
    fn links_round_trip_sorted_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let clang = link("clang@18.1.8-local", "/opt/llvm-18");
        let gcc = link("gcc@11.4.0-local", "/opt/gcc-11");
        s.save_links(&[clang.clone(), gcc.clone()]).unwrap();
        assert_eq!(s.links().unwrap(), [gcc, clang]);
        let text = fs::read_to_string(s.paths().links_file()).unwrap();
        assert!(text.contains("[[toolchain]]"), "{text}");
        assert!(text.contains("prefix = \"/opt/gcc-11\""), "{text}");
    }

    #[test]
    fn saving_no_links_leaves_a_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        s.save_links(&[link("gcc@11.4.0-local", "/opt/gcc-11")])
            .unwrap();
        s.save_links(&[]).unwrap();
        assert!(s.links().unwrap().is_empty());
    }

    #[test]
    fn invalid_links_file_is_an_error_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let path = s.paths().links_file();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for content in [
            "[[toolchain]]\nid = \"gcc@11\"\nprefix = \"/x\"\ncc = \"/x/gcc\"\ncxx = \"/x/g++\"\nversion = \"\"\n",
            "[[toolchain]]\nid = \"gcc@11.4.0-local\"\n",
            "toolchains = 1\n",
        ] {
            fs::write(&path, content).unwrap();
            let err = s.links().unwrap_err().to_string();
            assert!(
                err.contains(&path.display().to_string()),
                "{content}: {err}"
            );
            assert!(err.contains("hint:"), "{content}: {err}");
        }
    }

    #[test]
    fn installed_merges_store_and_links() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let gcc13 = add(&s, "gcc/13.3.0");
        add(&s, "clang/18.1.8");
        let linked = link("gcc@11.4.0-local", "/opt/gcc-11");
        s.save_links(std::slice::from_ref(&linked)).unwrap();
        let installed = s.installed().unwrap();
        assert_eq!(
            ids(&installed),
            ["gcc@11.4.0-local", "gcc@13.3.0", "clang@18.1.8"]
        );
        assert_eq!(
            installed[0],
            Installed {
                id: linked.id,
                path: linked.prefix,
                cc: linked.cc,
                cxx: linked.cxx,
                linked: true,
            }
        );
        assert_eq!(
            installed[1],
            Installed {
                id: "gcc@13.3.0".parse().unwrap(),
                cc: gcc13.join("bin/gcc"),
                cxx: gcc13.join("bin/g++"),
                path: gcc13,
                linked: false,
            }
        );
        assert_eq!(installed[2].cxx, installed[2].path.join("bin/clang++"));
    }

    #[test]
    fn store_entry_wins_over_a_link_with_the_same_id() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let in_store = add(&s, "gcc/11.4.0-local");
        s.save_links(&[link("gcc@11.4.0-local", "/opt/gcc-11")])
            .unwrap();
        let installed = s.installed().unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].path, in_store);
        assert!(!installed[0].linked);
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
    fn lock_waits_for_the_holder() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let held = s.lock().unwrap();
        let acquired = Arc::new(AtomicBool::new(false));
        let waiter = {
            let (s, acquired) = (s.clone(), Arc::clone(&acquired));
            std::thread::spawn(move || {
                let _lock = s.lock().unwrap();
                acquired.store(true, Ordering::SeqCst);
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!acquired.load(Ordering::SeqCst), "lock taken twice");
        drop(held);
        waiter.join().unwrap();
        assert!(acquired.load(Ordering::SeqCst));
    }

    #[test]
    fn lock_cleans_staging() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let leftover = s.paths().staging_dir().join("clang@18.1.8.123/bin");
        fs::create_dir_all(&leftover).unwrap();
        fs::write(leftover.join("clang"), "").unwrap();
        fs::write(s.paths().staging_dir().join("stray"), "").unwrap();
        let _lock = s.lock().unwrap();
        assert_eq!(fs::read_dir(s.paths().staging_dir()).unwrap().count(), 0);
        assert!(s.paths().lock_file().is_file());
    }

    #[test]
    fn meta_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Meta::load(dir.path()).unwrap(), None);
        let meta = Meta {
            id: "clang@19.1.0".into(),
            backend: "llvm-prebuilt".into(),
            url: "https://example.invalid/a.tar.xz".into(),
            sha256: "0".repeat(64),
            installed: "2026-10-03T14:05:09Z".into(),
            cc: "/s/toolchains/clang/19.1.0/bin/clang".into(),
            cxx: "/s/toolchains/clang/19.1.0/bin/clang++".into(),
            version: "clang version 19.1.0".into(),
            libstdcxx_dir: None,
            gcc_install_dir: None,
            host_compiler: None,
            configure_args: Vec::new(),
        };
        meta.write(dir.path()).unwrap();
        let text = fs::read_to_string(dir.path().join(META_FILE)).unwrap();
        assert!(!text.contains("libstdcxx_dir"), "{text}");
        assert_eq!(Meta::load(dir.path()).unwrap(), Some(meta.clone()));

        let gcc = Meta {
            id: "gcc@13.3.0".into(),
            libstdcxx_dir: Some("/s/toolchains/gcc/13.3.0/lib64".into()),
            gcc_install_dir: Some(
                "/s/toolchains/gcc/13.3.0/lib/gcc/x86_64-pc-linux-gnu/13.3.0".into(),
            ),
            ..meta
        };
        gcc.write(dir.path()).unwrap();
        assert_eq!(Meta::load(dir.path()).unwrap(), Some(gcc.clone()));
        let text = fs::read_to_string(dir.path().join(META_FILE)).unwrap();
        assert!(!text.contains("configure_args"), "{text}");

        let built = Meta {
            backend: "gnu-source".into(),
            host_compiler: Some("gcc (Ubuntu 15.2.0-16ubuntu1) 15.2.0".into()),
            configure_args: vec!["--prefix=/s/toolchains/gcc/13.3.0".into()],
            ..gcc
        };
        built.write(dir.path()).unwrap();
        assert_eq!(Meta::load(dir.path()).unwrap(), Some(built));
    }

    #[test]
    fn timestamps_are_rfc3339_utc() {
        let at = |secs| utc_timestamp(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(1_791_036_309), "2026-10-03T14:05:09Z");
        assert_eq!(at(4_107_542_399), "2100-02-28T23:59:59Z");
    }

    #[test]
    fn commit_moves_a_staged_prefix_into_the_store_once() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let lock = s.lock().unwrap();
        let id: ToolchainId = "clang@19.1.0".parse().unwrap();
        let staged = s.staging_path("clang-19.1.0");
        fs::create_dir_all(staged.join("bin")).unwrap();
        let target = s.commit(&lock, &staged, &id).unwrap();
        assert_eq!(target, s.paths().toolchain_dir(&id));
        assert!(target.join("bin").is_dir() && !staged.exists());

        fs::create_dir_all(&staged).unwrap();
        let err = s.commit(&lock, &staged, &id).unwrap_err().to_string();
        assert!(err.contains("already installed"), "{err}");
    }

    #[test]
    fn remove_deletes_the_toolchain_through_staging() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        let path = add(&s, "clang/19.1.0/bin");
        fs::write(path.join("clang"), "").unwrap();
        let lock = s.lock().unwrap();
        s.remove(&lock, &"clang@19.1.0".parse().unwrap()).unwrap();
        assert!(!s.paths().toolchains_dir().join("clang/19.1.0").exists());
        assert_eq!(fs::read_dir(s.paths().staging_dir()).unwrap().count(), 0);
        let err = s
            .remove(&lock, &"clang@19.1.0".parse().unwrap())
            .unwrap_err();
        assert!(err.to_string().contains("cannot move"), "{err}");
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
