//! Dependency compiler for building dependencies in correct order
//!
//! This module handles compiling dependencies before the main project,
//! ensuring proper build order and --extern flag passing.

use anyhow::{Context, Result};
use crate::dependency_downloader::{DependencyDownloader, DownloadedDependency};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, info};

/// Dependency compiler
pub struct DependencyCompiler {
    downloader: DependencyDownloader,
    build_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CompiledDependency {
    pub name: String,
    pub version: String,
    pub rlib_path: PathBuf,
    pub dependencies: Vec<String>,
}

impl DependencyCompiler {
    /// Create a new dependency compiler
    pub fn new(build_dir: PathBuf) -> Result<Self> {
        Ok(Self {
            downloader: DependencyDownloader::new()?,
            build_dir,
        })
    }

    /// Compile all dependencies for a project
    pub async fn compile_dependencies(
        &self,
        rust_deps: &HashMap<String, String>,
    ) -> Result<HashMap<String, CompiledDependency>> {
        info!("Compiling {} Rust dependencies", rust_deps.len());

        // Download all dependencies
        let downloaded = self.downloader.download_all(rust_deps).await?;

        // Resolve transitive dependencies
        let all_deps = self.resolve_transitive_dependencies(&downloaded).await?;

        // Build dependency graph
        let dep_graph = self.build_dependency_graph(&all_deps).await?;

        // Compile in topological order
        let compiled = self.compile_in_order(&dep_graph, &all_deps).await?;

        info!("Compiled {} dependencies", compiled.len());
        Ok(compiled)
    }

    /// Resolve transitive dependencies
    async fn resolve_transitive_dependencies(
        &self,
        direct_deps: &HashMap<String, DownloadedDependency>,
    ) -> Result<HashMap<String, DownloadedDependency>> {
        let all_deps = direct_deps.clone();
        let mut to_process: Vec<String> = direct_deps.keys().cloned().collect();
        let mut processed = HashSet::new();

        while let Some(dep_name) = to_process.pop() {
            if processed.contains(&dep_name) {
                continue;
            }
            processed.insert(dep_name.clone());

            if !all_deps.contains_key(&dep_name) {
                continue;
            }

            // For now, we'll assume dependencies are pre-resolved
            // In a full implementation, we'd parse the dependency's poly.ini
            // or use a registry API to get transitive dependencies
            debug!("Processing dependency: {}", dep_name);
        }

        Ok(all_deps)
    }

    /// Build dependency graph
    async fn build_dependency_graph(
        &self,
        deps: &HashMap<String, DownloadedDependency>,
    ) -> Result<HashMap<String, Vec<String>>> {
        // For now, return empty graph - dependencies are assumed to be independent
        // In a full implementation, we'd parse poly.ini files from dependencies
        let mut graph = HashMap::new();

        for name in deps.keys() {
            graph.insert(name.clone(), Vec::new());
        }

        Ok(graph)
    }

    /// Compile dependencies in topological order
    async fn compile_in_order(
        &self,
        graph: &HashMap<String, Vec<String>>,
        deps: &HashMap<String, DownloadedDependency>,
    ) -> Result<HashMap<String, CompiledDependency>> {
        let mut compiled = HashMap::new();
        let mut visited = HashSet::new();
        let mut visiting = HashSet::new();

        // Topological sort
        let sorted = self.topological_sort(graph, &mut visited, &mut visiting)?;

        // Compile in order
        for dep_name in sorted {
            let dep = deps.get(&dep_name)
                .context(format!("Dependency not found: {}", dep_name))?;

            let compiled_dep = self.compile_dependency(dep, &compiled).await?;
            compiled.insert(dep_name, compiled_dep);
        }

        Ok(compiled)
    }

    /// Topological sort of dependency graph
    fn topological_sort(
        &self,
        graph: &HashMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        visiting: &mut HashSet<String>,
    ) -> Result<Vec<String>> {
        let mut result = Vec::new();

        for node in graph.keys() {
            if !visited.contains(node) {
                self.visit_node(node, graph, visited, visiting, &mut result)?;
            }
        }

        Ok(result)
    }

    fn visit_node(
        &self,
        node: &str,
        graph: &HashMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        visiting: &mut HashSet<String>,
        result: &mut Vec<String>,
    ) -> Result<()> {
        if visiting.contains(node) {
            anyhow::bail!("Circular dependency detected: {}", node);
        }

        if visited.contains(node) {
            return Ok(());
        }

        visiting.insert(node.to_string());

        if let Some(deps) = graph.get(node) {
            for dep in deps {
                self.visit_node(dep, graph, visited, visiting, result)?;
            }
        }

        visiting.remove(node);
        visited.insert(node.to_string());
        result.push(node.to_string());

        Ok(())
    }

    /// Compile a single dependency
    async fn compile_dependency(
        &self,
        dep: &DownloadedDependency,
        compiled_deps: &HashMap<String, CompiledDependency>,
    ) -> Result<CompiledDependency> {
        debug!("Compiling dependency: {} {}", dep.name, dep.version);

        // Find the library target in the dependency
        let src_dir = dep.path.join("src");
        let lib_path = src_dir.join("lib.rs");
        
        if !lib_path.exists() {
            debug!("No lib.rs found for dependency: {}", dep.name);
            return Ok(CompiledDependency {
                name: dep.name.clone(),
                version: dep.version.clone(),
                rlib_path: dep.path.clone(),
                dependencies: Vec::new(),
            });
        }

        // Compile the dependency
        let output_dir = self.build_dir.join("deps").join(&dep.name);
        fs::create_dir_all(&output_dir)?;

        let crate_name = dep.name.replace('-', "_");
        let rlib_path = output_dir.join(format!("lib{}.rlib", crate_name));

        // Build rustc command
        let mut cmd = Command::new("rustc");
        cmd.arg("--crate-name")
            .arg(&crate_name)
            .arg("--crate-type")
            .arg("lib")
            .arg("--edition")
            .arg("2021") // Default edition
            .arg(&lib_path)
            .arg("-o")
            .arg(&rlib_path)
            .arg("-L")
            .arg(&output_dir);

        // Add --extern flags for compiled dependencies
        for (dep_name, compiled_dep) in compiled_deps {
            cmd.arg("--extern")
                .arg(format!("{}={}", dep_name.replace('-', "_"), compiled_dep.rlib_path.display()));
        }

        // Add dependency search paths
        for compiled_dep in compiled_deps.values() {
            if let Some(parent) = compiled_dep.rlib_path.parent() {
                cmd.arg("-L")
                    .arg(parent);
            }
        }

        // Run rustc
        let output = cmd.output()?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to compile dependency {}: {}", dep.name, stderr);
        }

        info!("Compiled dependency: {} {}", dep.name, dep.version);
        
        Ok(CompiledDependency {
            name: dep.name.clone(),
            version: dep.version.clone(),
            rlib_path,
            dependencies: compiled_deps.keys().cloned().collect(),
        })
    }

    /// Get --extern flags for a target
    pub fn get_extern_flags(&self, compiled_deps: &HashMap<String, CompiledDependency>) -> Vec<String> {
        Self::get_extern_flags_static(compiled_deps)
    }

    /// Get --extern flags for a target (static version)
    pub fn get_extern_flags_static(compiled_deps: &HashMap<String, CompiledDependency>) -> Vec<String> {
        let mut flags = Vec::new();
        
        for (name, dep) in compiled_deps {
            flags.push("--extern".to_string());
            flags.push(format!("{}={}", name.replace('-', "_"), dep.rlib_path.display()));
        }
        
        flags
    }

    /// Get library search paths
    pub fn get_library_paths(&self, compiled_deps: &HashMap<String, CompiledDependency>) -> Vec<String> {
        let mut paths = Vec::new();
        
        for dep in compiled_deps.values() {
            if let Some(parent) = dep.rlib_path.parent() {
                paths.push(format!("-L").to_string());
                paths.push(parent.display().to_string());
            }
        }
        
        paths
    }
}
