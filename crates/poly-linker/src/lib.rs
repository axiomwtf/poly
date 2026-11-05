//! Poly linker: unified linker orchestration
//!
//! This crate provides unified linker orchestration for Poly.
//! It links objects from multiple languages (Rust, C/C++, Go) into a single executable.

pub mod linker;
pub mod platform;

pub use linker::{Linker, LinkTarget, LinkResult};
pub use platform::{
    link_platform_executable, CBuild, GoBuild, LinkUnit, RustBuild,
};
