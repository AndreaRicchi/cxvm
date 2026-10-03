//! Toolchain installers: prebuilt Clang (M3), prebuilt GCC (M4), GCC from source (M5).
//!
//! Releases come from the embedded [`catalog`](crate::catalog). [`install`] runs the steps every
//! backend shares: download and verify the archive, let the [`Backend`] turn it into a prefix
//! in `staging/`, check that the compiler runs, record `.cxvm-meta.toml` and move the prefix
//! into the store in one rename.

pub mod download;
pub mod extract;
pub mod smoke;

use std::path::{Path, PathBuf};

use crate::catalog::Release;
use crate::detect;
use crate::error::{Error, Result};
use crate::store::{self, Installed, Meta, Store, StoreLock};
use crate::toolchain::Kind;

/// Turns a verified release archive into an install prefix.
pub trait Backend {
    /// Name recorded in `.cxvm-meta.toml`.
    fn name(&self) -> &'static str;

    /// Builds the prefix for `release` in `prefix`, a new directory in `staging/`, from its
    /// verified `archive`. The error is a reason for the user.
    fn prepare(&self, release: &Release, archive: &Path, prefix: &Path) -> Result<(), String>;
}

/// Official LLVM release archives (see docs/backends.md).
pub struct LlvmPrebuilt;

impl Backend for LlvmPrebuilt {
    fn name(&self) -> &'static str {
        "llvm-prebuilt"
    }

    fn prepare(&self, _release: &Release, archive: &Path, prefix: &Path) -> Result<(), String> {
        extract::unpack_tar_xz(archive, prefix)
    }
}

/// The backend that installs `kind`, if one exists yet.
pub fn backend_for(kind: Kind) -> Option<&'static dyn Backend> {
    match kind {
        Kind::Clang => Some(&LlvmPrebuilt),
        Kind::Gcc => None,
    }
}

/// A finished install.
#[derive(Debug)]
pub struct Report {
    pub toolchain: Installed,
    /// The cached archive it came from.
    pub archive: PathBuf,
    /// A problem that does not make the toolchain unusable, e.g. no C++ headers on the host.
    pub warning: Option<String>,
}

/// Installs `release` into the store. On failure nothing is left in `toolchains/`, and the
/// verified archive stays cached for the next attempt.
pub fn install(store: &Store, lock: &StoreLock, release: &Release) -> Result<Report> {
    let id = &release.id;
    let backend = backend_for(id.kind()).ok_or(Error::NotImplemented {
        command: "install gcc@…",
        milestone: "M4",
    })?;
    let paths = store.paths();
    let archive = download::fetch(release, &paths.downloads_dir())?;
    let staged = store.staging_path(&format!("{}-{}", id.kind(), id.version()));
    let fail = |reason: String, hint: String| Error::Install {
        id: id.to_string(),
        reason,
        hint,
    };

    let result = (|| {
        backend
            .prepare(release, &archive, &staged)
            .map_err(|reason| {
                fail(
                    reason,
                    format!(
                        "the archive may be damaged: delete {} and run the same command again",
                        archive.display()
                    ),
                )
            })?;

        let (cc, cxx) = detect::compilers(&staged, id.kind());
        let mut missing: Vec<String> = Vec::new();
        for driver in [&cc, &cxx] {
            for library in smoke::missing_libraries(driver) {
                if !missing.contains(&library) {
                    missing.push(library);
                }
            }
        }
        if !missing.is_empty() {
            return Err(fail(
                format!(
                    "its compiler cannot run on this system: missing shared libraries {}",
                    missing.join(", ")
                ),
                smoke::missing_libraries_hint(&missing),
            ));
        }
        let detected = detect::probe(&staged, id.kind()).map_err(|err| {
            fail(
                format!("its compiler does not run: {err}"),
                "check `cxvm list --available` for another release of the same major".into(),
            )
        })?;
        if &detected.version != id.version() {
            return Err(fail(
                format!("the archive holds {}@{}", id.kind(), detected.version),
                "report it at the cxvm issue tracker: the catalog entry is wrong".into(),
            ));
        }
        let warning = smoke::compile_and_run(&detected.cxx, &store.staging_path("smoke"));

        let target = paths.toolchain_dir(id);
        let (final_cc, final_cxx) = detect::compilers(&target, id.kind());
        Meta {
            id: id.to_string(),
            backend: backend.name().into(),
            url: release.url.clone(),
            sha256: release.sha256.clone(),
            installed: store::utc_timestamp(std::time::SystemTime::now()),
            cc: final_cc.clone(),
            cxx: final_cxx.clone(),
            version: detected.version_line,
        }
        .write(&staged)?;
        let path = store.commit(lock, &staged, id)?;
        Ok(Report {
            toolchain: Installed {
                id: id.clone(),
                path,
                cc: final_cc,
                cxx: final_cxx,
                linked: false,
            },
            archive: archive.clone(),
            warning,
        })
    })();
    if result.is_err() {
        let _ = store::remove_all(&staged);
    }
    result
}
