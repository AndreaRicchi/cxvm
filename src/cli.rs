//! Command-line surface: argument parsing with clap and dispatch to the library.
//!
//! The command set mirrors PLAN.md §5. Until a milestone implements a command, it returns
//! [`Error::NotImplemented`] so that scripts never mistake a stub for success.

use std::io;
use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use crate::commands::{self, Context};
use crate::error::{Error, Result};

/// Per-project C/C++ compiler versions for x86_64 Linux.
#[derive(Debug, Parser)]
#[command(name = "cxvm", version, about, long_about = None, propagate_version = true)]
pub struct Cli {
    /// Print more detail about what cxvm is doing
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// One entry per row of the PLAN.md §5 table.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Install a toolchain; with no id, install what `.cxvmrc` needs and regenerate `.cxvm/`
    Install {
        /// Toolchain id, e.g. `gcc@13.3.0`, `gcc@13` or `clang@18`
        id: Option<String>,
        /// Build GCC from source instead of using a prebuilt archive
        #[arg(long)]
        from_source: bool,
    },
    /// Remove a toolchain from the store
    Remove {
        /// Exact toolchain id, e.g. `gcc@13.3.0`
        id: String,
        /// Remove even if the toolchain is the global default
        #[arg(long)]
        force: bool,
    },
    /// List installed toolchains, marking the active one and the global default
    List {
        /// List installable versions instead of installed ones
        #[arg(long)]
        available: bool,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Pin a toolchain in the current project: write `.cxvmrc` and (re)generate `.cxvm/`
    Use {
        /// Toolchain id; partial versions resolve to the highest installed match
        id: String,
    },
    /// Set the default toolchain used outside projects
    Global {
        /// Toolchain id; partial versions resolve to the highest installed match
        id: String,
    },
    /// Show the active toolchain and why it is active
    Current {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Run a command with the active toolchain's environment (`PATH`, `CC`, `CXX`)
    Exec {
        /// Command and arguments, after `--`
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            required = true,
            value_name = "CMD"
        )]
        cmd: Vec<String>,
    },
    /// Print shell exports for `eval "$(cxvm env)"` or direnv
    Env {
        /// Shell syntax to emit; default: detected from `$SHELL`
        #[arg(long, value_enum)]
        shell: Option<EnvShell>,
    },
    /// Register an existing toolchain install, e.g. `/opt/gcc-11`
    Link {
        /// Install prefix containing `bin/gcc` + `bin/g++` or `bin/clang` + `bin/clang++`
        prefix: PathBuf,
        /// Id to register under instead of the detected `<kind>@<version>-local`
        #[arg(long = "as", value_name = "ID")]
        as_id: Option<String>,
    },
    /// Unregister a linked toolchain (files on disk are left alone)
    Unlink {
        /// Id of the linked toolchain
        id: String,
    },
    /// Diagnose problems with the store and the current project
    Doctor,
    /// Print a shell completion script to stdout
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
}

/// Shells `cxvm env` can emit exports for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EnvShell {
    Bash,
    Zsh,
    Fish,
}

/// Run the parsed command line.
pub fn run(cli: Cli) -> Result<()> {
    let (command, milestone) = match cli.command {
        Command::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "cxvm", &mut io::stdout());
            return Ok(());
        }
        Command::List {
            available: true, ..
        } => ("list --available", "M3"),
        Command::List { json, .. } => return commands::list(&Context::from_process_env()?, json),
        Command::Current { json } => return commands::current(&Context::from_process_env()?, json),
        Command::Install {
            id: None,
            from_source: false,
        } => return commands::install_pinned(&Context::from_process_env()?),
        Command::Install { .. } => ("install", "M3"),
        Command::Remove { .. } => ("remove", "M3"),
        Command::Use { id } => return commands::use_(&Context::from_process_env()?, &id),
        Command::Global { id } => return commands::global(&Context::from_process_env()?, &id),
        Command::Exec { .. } => ("exec", "M2"),
        Command::Env { .. } => ("env", "M2"),
        Command::Link { prefix, as_id } => {
            return commands::link(&Context::from_process_env()?, &prefix, as_id.as_deref());
        }
        Command::Unlink { id } => return commands::unlink(&Context::from_process_env()?, &id),
        Command::Doctor => ("doctor", "M7"),
    };
    Err(Error::NotImplemented { command, milestone })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clap_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn exec_keeps_hyphen_arguments_after_double_dash() {
        let cli = Cli::parse_from(["cxvm", "exec", "--", "make", "-j4", "--dry-run"]);
        match cli.command {
            Command::Exec { cmd } => assert_eq!(cmd, ["make", "-j4", "--dry-run"]),
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn link_as_flag_is_named_as() {
        let cli = Cli::parse_from(["cxvm", "link", "/opt/gcc-11", "--as", "gcc@11.4.0-opt"]);
        match cli.command {
            Command::Link { prefix, as_id } => {
                assert_eq!(prefix, PathBuf::from("/opt/gcc-11"));
                assert_eq!(as_id.as_deref(), Some("gcc@11.4.0-opt"));
            }
            other => panic!("unexpected command {other:?}"),
        }
    }
}
