//! cxvm: per-project C/C++ compiler versions for x86_64 Linux.
//!
//! This crate holds all of the logic; `src/main.rs` only parses arguments and maps errors to an
//! exit code.

pub mod backends;
pub mod catalog;
pub mod cli;
pub mod commands;
pub mod config;
pub mod detect;
pub mod doctor;
pub mod error;
mod files;
pub mod integrations;
pub mod output;
pub mod paths;
pub mod project;
pub mod store;
pub mod toolchain;

pub use error::{Error, Result};
