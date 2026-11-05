//! Source code dependency analyzer
//!
//! This module scans Rust source code to find external crate dependencies
//! by parsing `use` statements and `extern crate` declarations.

use anyhow::Result;
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use tracing::debug;

/// Dependency analyzer for Rust source code
pub struct DependencyAnalyzer {
    use_regex: Regex,
    extern_regex: Regex,
    path_regex: Regex,
}

impl DependencyAnalyzer {
    /// Create a new dependency analyzer
    pub fn new() -> Result<Self> {
        Ok(Self {
            // Match `use crate_name::...`, `pub use crate_name::...`, etc.
            use_regex: Regex::new(r"(?m)^\s*(?:pub(?:\s*\([^)]*\))?\s+)?use\s+([a-zA-Z_][a-zA-Z0-9_]*)")?,
            // Match `extern crate crate_name;`
            extern_regex: Regex::new(r"^\s*extern\s+crate\s+([a-zA-Z_][a-zA-Z0-9_]*)")?,
            // Match qualified paths like `tokio::main`, `dirs::cache_dir`, `tracing_subscriber::fmt`
            path_regex: Regex::new(r"\b([a-zA-Z_][a-zA-Z0-9_]*)\s*::")?,
        })
    }

    /// Analyze a Rust source file for external dependencies
    pub fn analyze_file(&self, path: &Path) -> Result<HashSet<String>> {
        let content = fs::read_to_string(path)?;
        self.analyze_content(&content)
    }

    /// Analyze Rust source code content for external dependencies
    pub fn analyze_content(&self, content: &str) -> Result<HashSet<String>> {
        let mut deps = HashSet::new();

        for line in content.lines() {
            let line = line.trim();
            
            // Skip comments and empty lines
            if line.is_empty() || line.starts_with("//") || line.starts_with("/*") {
                continue;
            }

            // Check for `use` statements - only extract the first path segment
            if let Some(captures) = self.use_regex.captures(line) {
                if let Some(crate_name) = captures.get(1) {
                    let name = crate_name.as_str();
                    // Skip local modules and std/core/alloc
                    if !self.is_local_or_std(name) {
                        // Only take the first segment of the path
                        // e.g., "use tokio::sync::mpsc" -> "tokio"
                        // e.g., "use crate::module::item" -> skip (crate is local)
                        deps.insert(name.replace('-', "_"));
                    }
                }
            }

            // Check for `extern crate` declarations
            if let Some(captures) = self.extern_regex.captures(line) {
                if let Some(crate_name) = captures.get(1) {
                    let name = crate_name.as_str();
                    if !self.is_local_or_std(name) {
                        deps.insert(name.replace('-', "_"));
                    }
                }
            }

            // Check for qualified paths and macro attributes (e.g. `tokio::main`, `dirs::cache_dir`)
            for captures in self.path_regex.captures_iter(line) {
                if let Some(crate_name) = captures.get(1) {
                    let name = crate_name.as_str();
                    if !self.is_local_or_std(name) {
                        deps.insert(name.replace('-', "_"));
                    }
                }
            }
        }

        Ok(deps)
    }

    /// Check if a crate name is local or from std/core/alloc
    fn is_local_or_std(&self, name: &str) -> bool {
        matches!(name, 
            "std" | "core" | "alloc" | "proc_macro" | "test" |
            "self" | "super" | "crate" |
            // Common false positives from internal modules
            "mocks" | "tests" | "benches" | "examples" |
            "internal" | "private" | "sealed" | "driver" | "tree" |
            "task" | "config" | "scheduler" | "registration_set" |
            "orphan" | "counters" | "scoped" | "chan" | "metrics" |
            "entry" | "trace" | "format" | "escape" | "directive" |
            "connect" | "header" | "parser" | "symbol" | "defer" |
            "reap" | "uring" | "io_uring" | "slab" | "backtrace" |
            "doc_comment" | "ValueMatch" | "ParseState" | "TryCurrentErrorKind"
        )
    }

    /// Analyze all Rust files in a directory recursively
    pub fn analyze_directory(&self, dir: &Path) -> Result<HashSet<String>> {
        let mut all_deps = HashSet::new();
        self.scan_directory(dir, &mut all_deps)?;
        Ok(all_deps)
    }

    /// Recursively scan a directory for Rust files
    fn scan_directory(&self, dir: &Path, deps: &mut HashSet<String>) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                // Skip target/debug directories and test directories
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if matches!(name, "target" | "dist" | "tests" | "benches" | "examples" | ".git") {
                        continue;
                    }
                }
                self.scan_directory(&path, deps)?;
            } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
                // Skip test files
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    if file_name.ends_with("_test.rs") || file_name.ends_with("_tests.rs") {
                        continue;
                    }
                }
                match self.analyze_file(&path) {
                    Ok(file_deps) => {
                        deps.extend(file_deps);
                    }
                    Err(e) => {
                        debug!("Failed to analyze {:?}: {}", path, e);
                    }
                }
            }
        }

        Ok(())
    }
}

impl Default for DependencyAnalyzer {
    fn default() -> Self {
        Self::new().expect("Failed to create DependencyAnalyzer")
    }
}
