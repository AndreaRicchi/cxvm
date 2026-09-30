//! The library's error type. Every message says what went wrong and what to do next.

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
}

/// Shorthand for results carrying [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
