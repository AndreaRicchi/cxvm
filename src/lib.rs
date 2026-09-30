//! cxvm: per-project C/C++ compiler versions for x86_64 Linux.
//!
//! This crate holds all of the logic; `src/main.rs` only parses arguments and maps errors to an
//! exit code. See `PLAN.md` for the specification and milestone plan.

pub mod backends;
pub mod cli;
pub mod config;
pub mod doctor;
pub mod error;
pub mod integrations;
pub mod paths;
pub mod project;
pub mod store;
pub mod toolchain;

pub use error::{Error, Result};
