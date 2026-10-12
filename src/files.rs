//! Small file helpers shared by the config, project and store modules.

use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};

use serde::de::DeserializeOwned;

use crate::error::{Error, Result};
use crate::output::{self, debug};

/// `path` made absolute against `cwd`, with `.` and `..` resolved lexically. Symlinks are
/// kept as given, unlike `canonicalize`.
pub(crate) fn absolute(cwd: &Path, path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in cwd.join(path).components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            other => result.push(other),
        }
    }
    result
}

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

/// Runs `command` to the end with its output captured, retrying like [`retry_busy`]. With
/// `-v`, says what ran and how it ended.
pub(crate) fn run_captured(command: &mut Command) -> io::Result<Output> {
    let result = retry_busy(|| command.output());
    debug!(
        "ran {}: {}",
        output::command_line(command),
        match &result {
            Ok(output) => output.status.to_string(),
            Err(err) => err.to_string(),
        }
    );
    result
}

/// Whether `name` is one ordinary, visible path component: not empty, no `/`, no leading `.`.
pub(crate) fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\0'])
}

/// Whether `path` is a regular file (after following symlinks) with an execute bit set.
pub(crate) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Reads a text file; `Ok(None)` if it does not exist.
pub(crate) fn read_text(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::Io {
            action: "read",
            path: path.to_owned(),
            source,
            hint: "make sure it is a readable file, or remove it",
        }),
    }
}

/// Parses `text`, the content of the TOML file at `path`. `hint` is attached to parse errors
/// and should say how to fix the file.
pub(crate) fn parse_toml<T: DeserializeOwned>(path: &Path, text: &str, hint: &str) -> Result<T> {
    toml::from_str(text).map_err(|err| Error::InvalidFile {
        path: path.to_owned(),
        message: err.to_string().trim_end().to_owned(),
        hint: hint.to_owned(),
    })
}

/// Reads and parses a TOML file; `Ok(None)` if it does not exist. `hint` is as for
/// [`parse_toml`].
pub(crate) fn read_toml<T: DeserializeOwned>(path: &Path, hint: &str) -> Result<Option<T>> {
    read_text(path)?
        .map(|text| parse_toml(path, &text, hint))
        .transpose()
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
    result.map_err(io_error("write", path))?;
    debug!("wrote {}", path.display());
    Ok(())
}
