//! Crate graph construction for Rust

use anyhow::{Context, Result};
use crate::driver::RustTarget;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    targets: HashMap<String, RustTarget>,
    output_dir: PathBuf,
}

impl CrateGraph {
    /// Create a new crate graph
    pub fn new(output_dir: PathBuf) -> Self {
        Self {
            crates: HashMap::new(),
            targets: HashMap::new(),
            output_dir,
        }
    }

    /// Add a crate to the graph
    pub fn add_crate(&mut self, crate_: Crate) -> Result<()> {
        self.crates.insert(crate_.name.clone(), crate_);
        Ok(())
    }

    /// Add a Rust target
    pub fn add_target(&mut self, target: RustTarget) -> Result<()> {
        self.targets.insert(target.name.clone(), target);
        Ok(())
    }

    /// Get a crate by name
    pub fn get_crate(&self, name: &str) -> Option<&Crate> {
        self.crates.get(name)
    }

    /// Get a target by name
    pub fn get_target(&self, name: &str) -> Option<&RustTarget> {
        self.targets.get(name)
    }

    /// Get dependencies for a target
    pub fn get_dependencies(&self, name: &str) -> Result<Vec<String>> {
        let target = self.targets.get(name)
            .context(format!("Target {} not found", name))?;
        Ok(target.dependencies.clone())
    }

    /// Get the output path for a target
    pub fn get_output_path(&self, name: &str) -> Result<PathBuf> {
        let target = self.targets.get(name)
            .context(format!("Target {} not found", name))?;

        let extension = match target.crate_type.as_str() {
            "bin" => "",
            "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro" => {
                match target.crate_type.as_str() {
                    "lib" | "rlib" => "rlib",
                    "dylib" => "so",
                    "cdylib" => "so",
                    "staticlib" => "a",
                    "proc-macro" => "so",
                    _ => "rlib",
                }
            }
            _ => "rlib",
        };

        if extension.is_empty() {
            Ok(self.output_dir.join(name))
        } else {
            Ok(self.output_dir.join(format!("lib{}.{}", name, extension)))
        }
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

    /// Get crates in compilation order (topological sort)
    pub fn compilation_order(&self) -> Result<Vec<String>> {
        let mut visited = std::collections::HashSet::new();
        let mut order = Vec::new();

        for name in self.targets.keys() {
            if !visited.contains(name) {
                self.visit(name, &mut visited, &mut order)?;
            }
        }

        Ok(order)
    }

    fn visit(
        &self,
        name: &str,
        visited: &mut std::collections::HashSet<String>,
        order: &mut Vec<String>,
    ) -> Result<()> {
        if visited.contains(name) {
            return Ok(());
        }

        visited.insert(name.to_string());

        if let Some(target) = self.targets.get(name) {
            for dep in &target.dependencies {
                if self.targets.contains_key(dep) {
                    self.visit(dep, visited, order)?;
                }
            }
        }

        order.push(name.to_string());
        Ok(())
    }

    /// Parse Rust source to extract dependencies
    pub fn parse_dependencies(&self, source: &Path) -> Result<Vec<String>> {
        // MVP: Simple parsing of use statements
        // Full implementation would use a proper Rust parser

        let content = std::fs::read_to_string(source)?;
        let mut deps = Vec::new();

        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("use ") {
                // Extract crate name from use statement
                let rest = &line[4..];
                if let Some(crate_name) = rest.split("::").next() {
                    let crate_name = crate_name.trim();
                    if !crate_name.is_empty() && crate_name != "std" && crate_name != "core" {
                        deps.push(crate_name.to_string());
                    }
                }
            }
        }

        Ok(deps)
    }
}

impl Default for CrateGraph {
    fn default() -> Self {
        Self::new(PathBuf::from("target"))
    }
}
