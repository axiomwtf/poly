//! Poly Rust driver: direct rustc invocation
//!
//! This crate provides the Rust compiler driver for Poly.
//! It calls rustc directly (not through Cargo) and handles:
//! - Crate graph construction
//! - Feature resolution
//! - cfg evaluation
//! - Incremental compilation
//! - Build script execution

pub mod driver;
pub mod graph;

pub use driver::RustDriver;
pub use graph::CrateGraph;
