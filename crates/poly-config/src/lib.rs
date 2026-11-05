//! Poly configuration: poly.ini parser
//!
//! This crate provides the INI-based configuration parser for Poly.
//! It handles the poly.ini manifest format with support for:
//! - Hierarchical sections (dots make hierarchy)
//! - Arrays (comma-separated)
//! - Metadata in values (pipe-separated)
//! - camelCase key validation

pub mod parser;
pub mod model;

pub use model::{Config, Project, Target, Dependencies, BuildConfig};
pub use parser::parse_poly_ini;
