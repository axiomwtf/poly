//! Poly registry client for crates.io
//!
//! This crate provides a client for interacting with crates.io registry.
//! It handles dependency resolution, package downloading, and verification.

pub mod client;
pub mod resolver;

pub use client::{CrateDependency, RegistryClient};
pub use resolver::{RegistryResolver, ResolvedDependency};
