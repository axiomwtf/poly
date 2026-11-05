//! Poly C/C++ driver: direct gcc/clang/msvc invocation
//!
//! This crate provides the C/C++ compiler driver for Poly.
//! It calls gcc, clang, or msvc directly (not through CMake/Make) and handles:
//! - Compiler detection
//! - Build graph from source lists
//! - Header dependency scanning
//! - Cross-compilation support
//! - Static/dynamic linking

pub mod driver;
pub mod scanner;

pub use driver::CDriver;
pub use scanner::HeaderScanner;
