//! Poly package: single-binary packaging and distribution
//!
//! This crate provides single-binary packaging for Poly projects.
//! It creates VFSX-signed archives containing one executable per platform.

pub mod packager;
pub mod installer;

pub use packager::{Packager, PackageConfig};
pub use installer::{Installer, InstallConfig};
