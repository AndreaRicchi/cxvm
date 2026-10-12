//! The library's error type. Every message says what went wrong and what to do next.

use std::io;
use std::path::PathBuf;

/// Errors surfaced to the user by `cxvm` commands.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A string that is not a valid toolchain id.
    #[error(
        "invalid toolchain id `{input}`: {reason}\n  \
         hint: use `gcc@<version>`, `clang@<version>` or `system`, e.g. `gcc@13.3.0` or `clang@18`"
    )]
    InvalidId {
        /// The rejected input.
        input: String,
        /// What is wrong with it.
        reason: String,
    },

    /// `CXVM_COMPILER` is not a valid toolchain id.
    #[error(
        "CXVM_COMPILER is set to `{input}`, which is not a valid toolchain id: {reason}\n  \
         hint: set it to e.g. `gcc@13.3.0`, `clang@18` or `system`, or unset it"
    )]
    InvalidEnvCompiler { input: String, reason: String },

    /// The active toolchain is not in the store.
    #[error("{spec} is not installed{context}\n  hint: {hint}")]
    NotInstalled {
        spec: String,
        /// Where the pin came from, e.g. ` (set by /p/.cxvmrc)`, or empty.
        context: String,
        hint: String,
    },

    /// `CXVM_HOME` is set to a relative path.
    #[error(
        "CXVM_HOME is set to `{}`, which is not an absolute path\n  \
         hint: set CXVM_HOME to an absolute path, or unset it to use ~/.local/share/cxvm",
        path.display()
    )]
    RelativeCxvmHome { path: PathBuf },

    /// No usable `HOME` to derive default locations from.
    #[error(
        "cannot find your home directory: HOME is unset or not an absolute path\n  \
         hint: set HOME, or set CXVM_HOME and XDG_CACHE_HOME to absolute paths"
    )]
    NoHome,

    /// A filesystem operation failed.
    #[error("cannot {action} {}: {source}\n  hint: {hint}", path.display())]
    Io {
        /// What cxvm was doing, e.g. "read".
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
        /// What the user can do about it.
        hint: &'static str,
    },

    /// A request cxvm refuses, with what to do instead.
    #[error("{message}\n  hint: {hint}")]
    Usage { message: String, hint: String },

    /// A prefix whose compilers cannot be identified.
    #[error("cannot detect the toolchain in {}: {reason}\n  hint: {hint}", prefix.display())]
    Detect {
        prefix: PathBuf,
        reason: String,
        hint: String,
    },

    /// A download that failed or whose content did not verify.
    #[error("cannot download {url}: {reason}\n  hint: {hint}")]
    Download {
        url: String,
        reason: String,
        hint: String,
    },

    /// An install that failed after the download.
    #[error("cannot install {id}: {reason}\n  hint: {hint}")]
    Install {
        id: String,
        reason: String,
        hint: String,
    },

    /// A build from source whose `configure` or `make` failed.
    #[error(
        "cannot build {id}: {failure}\n{detail}  the full log is {}\n  hint: {hint}",
        log.display()
    )]
    Build {
        id: String,
        /// The step that failed and how, e.g. `` `make` exited with status 2 ``.
        failure: String,
        log: PathBuf,
        /// Indented lines quoted from the log, or empty.
        detail: String,
        hint: String,
    },

    /// The user interrupted a long operation, which cleaned up after itself.
    #[error(
        "{what} was interrupted; nothing was installed\n  \
         hint: run the same command again to start over; what was downloaded stays in the cache"
    )]
    Cancelled {
        /// What was running, e.g. `the build of gcc@10.5.0`.
        what: String,
    },

    /// `cxvm doctor` found errors, which it has printed with their hints.
    #[error(
        "cxvm doctor found {summary}\n  \
         hint: the hint under each error says what to do; run `cxvm doctor` again afterwards"
    )]
    Diagnosed {
        /// What it found, e.g. `2 errors and 1 warning`.
        summary: String,
    },

    /// A cxvm file (`config.toml`, `.cxvmrc`) has invalid content.
    #[error("{} is invalid: {message}\n  hint: {hint}", path.display())]
    InvalidFile {
        path: PathBuf,
        message: String,
        hint: String,
    },
}

impl Error {
    /// The exit status `cxvm` ends with for this error: 130 after an interrupt, as a shell
    /// reports a command stopped by Ctrl-C, else 1.
    pub fn exit_code(&self) -> u8 {
        match self {
            Error::Cancelled { .. } => 130,
            _ => 1,
        }
    }
}

/// Shorthand for results carrying [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
