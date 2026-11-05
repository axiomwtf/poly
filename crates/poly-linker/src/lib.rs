//! Poly linker: unified linker orchestration
//!
//! This crate provides unified linker orchestration for Poly.
//! It links objects from multiple languages (Rust, C/C++, Go) into a single executable.

pub mod linker;

pub use linker::{Linker, LinkTarget, LinkResult};
