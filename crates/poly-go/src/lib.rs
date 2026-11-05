//! Poly Go driver: direct go tool invocation
//!
//! This crate provides the Go compiler driver for Poly.
//! It calls go tool compile and go tool link directly (not through go mod) and handles:
//! - Module information from poly.ini
//! - Vendoring
//! - CGO support
//! - Build tags and constraints

pub mod driver;

pub use driver::GoDriver;
