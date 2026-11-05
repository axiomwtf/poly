//! Poly dependency resolver: SAT-based dependency resolution
//!
//! This crate provides SAT-based dependency resolution for Poly.
//! It handles resolving dependencies across all three languages (Rust, C/C++, Go)
//! and generates the poly.lock file.

pub mod resolver;
pub mod lockfile;

pub use resolver::{DependencyResolver, ResolutionResult};
pub use lockfile::{LockFile, LockEntry};
