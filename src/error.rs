//! The library's error type. Every message says what went wrong and what to do next.

use std::io;
use std::path::PathBuf;

/// Errors surfaced to the user by `cxvm` commands.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A command whose milestone has not been reached yet.
    #[error(
        "`cxvm {command}` is not implemented yet: it is planned for milestone {milestone} (see PLAN.md)"
    )]
    NotImplemented {
        /// Subcommand name as typed by the user.
        command: &'static str,
        /// Milestone from PLAN.md §8 that implements it.
        milestone: &'static str,
    },

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
    #[error("{spec} is not installed ({reason})\n  hint: {hint}")]
    NotInstalled {
        spec: String,
        /// Where the pin came from, e.g. `set by /p/.cxvmrc`.
        reason: String,
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

    /// A cxvm file (`config.toml`, `.cxvmrc`) has invalid content.
    #[error("{} is invalid: {message}\n  hint: {hint}", path.display())]
    InvalidFile {
        path: PathBuf,
        message: String,
        hint: String,
    },
}

/// Shorthand for results carrying [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
