//! Crate graph construction for Rust

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::debug;

/// Represents a Rust crate
#[derive(Debug, Clone)]
pub struct Crate {
    pub name: String,
    pub root: PathBuf,
    pub dependencies: Vec<String>,
    pub edition: String,
}

/// Crate graph for Rust project
pub struct CrateGraph {
    crates: HashMap<String, Crate>,
}

impl CrateGraph {
    /// Create a new crate graph
    pub fn new() -> Self {
        Self {
            crates: HashMap::new(),
        }
    }

    /// Add a crate to the graph
    pub fn add_crate(&mut self, crate_: Crate) -> Result<()> {
        self.crates.insert(crate_.name.clone(), crate_);
        Ok(())
    }

    /// Get a crate by name
    pub fn get_crate(&self, name: &str) -> Option<&Crate> {
        self.crates.get(name)
    }

    /// Build the crate graph by parsing source files
    /// For MVP, this is a simple implementation
    pub fn build_from_sources(&mut self, sources: &[PathBuf]) -> Result<()> {
        debug!("Building crate graph from {} source files", sources.len());

        // MVP: Simple single-crate support
        // Full implementation would parse mod declarations, use statements, etc.

        if sources.is_empty() {
            anyhow::bail!("No sources provided");
        }

        // Assume first source is the lib or bin root
        let root = sources[0].clone();
        let name = root
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let crate_ = Crate {
            name: name.clone(),
            root,
            dependencies: Vec::new(),
            edition: "2024".to_string(),
        };

        self.add_crate(crate_)?;

        debug!("Built crate graph with 1 crate");
        Ok(())
    }

    /// Get crates in compilation order
    pub fn compilation_order(&self) -> Vec<String> {
        // MVP: Return all crates in any order
        // Full implementation would do topological sort
        self.crates.keys().cloned().collect()
    }
}

impl Default for CrateGraph {
    fn default() -> Self {
        Self::new()
    }
}
