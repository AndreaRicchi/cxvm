//! Toolchain installers: prebuilt Clang (M3), prebuilt GCC (M4), GCC from source (M5).
//!
//! Releases come from the embedded [`catalog`](crate::catalog); archives are downloaded into
//! the cache and verified by [`download`].

pub mod download;
