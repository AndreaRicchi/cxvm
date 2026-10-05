//! Downloading release archives into the cache: resumable, with progress, and verified against
//! the catalog's size and sha256 before they are used.

use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};

use crate::catalog::Download;
use crate::error::{Error, Result};

/// Returns the verified file `release` describes, in `dir`, downloading what is missing.
///
/// The download goes to `<asset>.part`, which a later run resumes, and is renamed to `<asset>`
/// only once its size and sha256 match. A cached archive is checked again before it is reused;
/// one that does not match is deleted.
pub fn fetch(release: Download<'_>, dir: &Path) -> Result<PathBuf> {
    let archive = dir.join(release.asset);
    let part = dir.join(format!("{}.part", release.asset));
    let io_error = |action, path: &Path| {
        let path = path.to_owned();
        move |source| Error::Io {
            action,
            path,
            source,
            hint: "check that the download cache is writable and the disk is not full",
        }
    };
    fs::create_dir_all(dir).map_err(io_error("create", dir))?;

    if archive.exists() {
        eprintln!("Checking the cached {}", archive.display());
        let (hasher, len) = hash_file(&archive).map_err(io_error("read", &archive))?;
        if len == release.size && hex(hasher) == release.sha256 {
            return Ok(archive);
        }
        eprintln!("warning: the cached archive does not match the catalog; downloading it again");
        fs::remove_file(&archive).map_err(io_error("delete", &archive))?;
    }

    // Resume from what an interrupted run left, unless it is already too long.
    let (mut hasher, mut len) = match hash_file(&part) {
        Ok((hasher, len)) if len <= release.size => (hasher, len),
        Ok(_) => {
            fs::remove_file(&part).map_err(io_error("delete", &part))?;
            (Sha256::new(), 0)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => (Sha256::new(), 0),
        Err(err) => return Err(io_error("read", &part)(err)),
    };

    if len < release.size {
        let download_error = |reason: String| Error::Download {
            url: release.url.to_owned(),
            reason,
            hint: "check your network connection and run the same command again; \
                   the download resumes where it stopped"
                .into(),
        };
        let mut request = agent().get(release.url);
        if len > 0 {
            request = request.header("Range", format!("bytes={len}-"));
        }
        let response = request.call().map_err(|e| download_error(e.to_string()))?;
        let resumed = len > 0 && response.status().as_u16() == 206;
        if len > 0 && !resumed {
            // The server sent the whole file: start over.
            hasher = Sha256::new();
            len = 0;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .open(&part)
            .map_err(io_error("write", &part))?;

        let progress = progress_bar(release, len);
        let mut body = response.into_body().into_reader();
        let mut buf = vec![0; 256 * 1024];
        loop {
            let n = match body.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    progress.abandon();
                    return Err(download_error(err.to_string()));
                }
            };
            if len + n as u64 > release.size {
                progress.abandon();
                drop(file);
                let _ = fs::remove_file(&part);
                return Err(Error::Download {
                    url: release.url.to_owned(),
                    reason: format!(
                        "the server sent more than the expected {} bytes",
                        release.size
                    ),
                    hint: "the file upstream changed; report it at the cxvm issue tracker".into(),
                });
            }
            file.write_all(&buf[..n])
                .map_err(io_error("write", &part))?;
            hasher.update(&buf[..n]);
            len += n as u64;
            progress.set_position(len);
        }
        file.sync_all().map_err(io_error("write", &part))?;
        progress.finish_and_clear();
        if len < release.size {
            return Err(download_error(format!(
                "the connection closed after {len} of {} bytes",
                release.size
            )));
        }
    }

    let actual = hex(hasher);
    if actual != release.sha256 {
        let _ = fs::remove_file(&part);
        return Err(Error::Download {
            url: release.url.to_owned(),
            reason: format!(
                "sha256 mismatch: expected {}, got {actual}; the file was deleted",
                release.sha256
            ),
            hint: "run the same command again; if it keeps failing, the file upstream changed \
                   or something altered it on the way, so report it at the cxvm issue tracker"
                .into(),
        });
    }
    fs::rename(&part, &archive).map_err(io_error("write", &archive))?;
    Ok(archive)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(concat!("cxvm/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Some(Duration::from_secs(30)))
        .build()
        .into()
}

/// A progress bar on stderr when it is a terminal; otherwise one line saying what is downloaded.
fn progress_bar(release: Download<'_>, start: u64) -> ProgressBar {
    let what = if start > 0 {
        format!("Resuming the download of {}", release.asset)
    } else {
        format!("Downloading {}", release.asset)
    };
    if !io::stderr().is_terminal() {
        eprintln!("{what} ({})", release.url);
        return ProgressBar::hidden();
    }
    eprintln!("{what}");
    let bar = ProgressBar::new(release.size).with_style(
        ProgressStyle::with_template(
            "  [{bar:30}] {bytes}/{total_bytes} {bytes_per_sec}, {eta} left",
        )
        .expect("valid template")
        .progress_chars("=> "),
    );
    bar.set_position(start);
    bar
}

/// The sha256 state and length of the file at `path`.
fn hash_file(path: &Path) -> io::Result<(Sha256, u64)> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 1 << 20];
    let mut len = 0;
    loop {
        match file.read(&mut buf) {
            Ok(0) => return Ok((hasher, len)),
            Ok(n) => {
                hasher.update(&buf[..n]);
                len += n as u64;
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
}

fn hex(hasher: Sha256) -> String {
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Release;

    /// A release whose URL refuses connections, so any network access fails the test.
    fn release(content: &[u8]) -> Release {
        let mut hasher = Sha256::new();
        hasher.update(content);
        Release {
            id: "clang@19.1.0".parse().unwrap(),
            asset: "LLVM-19.1.0-Linux-X64.tar.xz".into(),
            url: "http://127.0.0.1:1/LLVM-19.1.0-Linux-X64.tar.xz".into(),
            size: content.len() as u64,
            sha256: hex(hasher),
            source: "test".into(),
        }
    }

    #[test]
    fn reuses_a_verified_cached_archive() {
        let dir = tempfile::tempdir().unwrap();
        let release = release(b"archive");
        fs::write(dir.path().join(&release.asset), b"archive").unwrap();
        assert_eq!(
            fetch(release.download(), dir.path()).unwrap(),
            dir.path().join(&release.asset)
        );
    }

    #[test]
    fn completes_a_full_part_file_without_the_network() {
        let dir = tempfile::tempdir().unwrap();
        let release = release(b"archive");
        fs::write(
            dir.path().join(format!("{}.part", release.asset)),
            b"archive",
        )
        .unwrap();
        let archive = fetch(release.download(), dir.path()).unwrap();
        assert_eq!(fs::read(archive).unwrap(), b"archive");
        assert!(!dir.path().join(format!("{}.part", release.asset)).exists());
    }

    #[test]
    fn deletes_a_bad_cached_archive_and_a_bad_part() {
        let dir = tempfile::tempdir().unwrap();
        let release = release(b"archive");
        let cached = dir.path().join(&release.asset);
        let part = dir.path().join(format!("{}.part", release.asset));
        fs::write(&cached, b"tampered").unwrap();
        fs::write(&part, b"ARCHIVE").unwrap();
        let err = fetch(release.download(), dir.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("sha256 mismatch"), "{err}");
        assert!(!cached.exists() && !part.exists());
    }

    #[test]
    fn network_errors_say_the_download_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let err = fetch(release(b"archive").download(), dir.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("127.0.0.1:1"), "{err}");
        assert!(err.contains("resumes"), "{err}");
    }

    #[test]
    fn human_readable_hex() {
        let mut hasher = Sha256::new();
        hasher.update(b"abc");
        assert_eq!(
            hex(hasher),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
