//! Small file helpers shared by the config, project and store modules.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use serde::de::DeserializeOwned;

use crate::error::{Error, Result};

/// Runs `spawn` again while it fails with `ExecutableFileBusy` (ETXTBSY), for up to about a
/// second. Executing a file fails that way while any process still holds it open for writing,
/// e.g. a child forked by another thread just after the file was written, before its exec.
pub(crate) fn retry_busy<T>(mut spawn: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut delay = std::time::Duration::from_millis(5);
    for _ in 0..8 {
        match spawn() {
            Err(err) if err.kind() == io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(delay);
                delay *= 2;
            }
            result => return result,
        }
    }
    spawn()
}

/// Whether `path` is a regular file (after following symlinks) with an execute bit set.
pub(crate) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Reads and parses a TOML file; `Ok(None)` if it does not exist. `hint` is attached to parse
/// errors and should say how to fix the file.
pub(crate) fn read_toml<T: DeserializeOwned>(path: &Path, hint: &str) -> Result<Option<T>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::Io {
                action: "read",
                path: path.to_owned(),
                source,
                hint: "make sure it is a readable file, or remove it",
            });
        }
    };
    toml::from_str(&text)
        .map(Some)
        .map_err(|err| Error::InvalidFile {
            path: path.to_owned(),
            message: err.to_string().trim_end().to_owned(),
            hint: hint.to_owned(),
        })
}

/// Replaces `path` with `contents` atomically: a temp file in the same directory is written,
/// synced and renamed over it. Missing parent directories are created.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let io_error = |action, path: &Path| {
        let path = path.to_owned();
        move |source| Error::Io {
            action,
            path,
            source,
            hint: "check that the directory is writable",
        }
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir).map_err(io_error("create", dir))?;

    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dir.join(format!(".{name}.tmp.{}", std::process::id()));
    let result = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(io_error("write", path))
}
