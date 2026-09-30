//! The library's error type. Every message says what went wrong and what to do next.

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
}

/// Shorthand for results carrying [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
