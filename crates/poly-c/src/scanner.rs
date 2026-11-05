//! Header dependency scanning for C/C++

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::debug;

/// Header dependency scanner
pub struct HeaderScanner {
    compiler_path: PathBuf,
    compiler_type: crate::driver::CompilerType,
}

impl HeaderScanner {
    /// Create a new header scanner
    pub fn new(compiler_path: PathBuf, compiler_type: crate::driver::CompilerType) -> Self {
        Self { compiler_path, compiler_type }
    }

    /// Scan a C/C++ source file for header dependencies
    pub fn scan(&self, source: &Path, include_dirs: &[PathBuf]) -> Result<Vec<PathBuf>> {
        debug!("Scanning headers for: {:?}", source);

        let mut cmd = Command::new(&self.compiler_path);

        // Add include directories
        for include_dir in include_dirs {
            match self.compiler_type {
                crate::driver::CompilerType::Gcc | crate::driver::CompilerType::Clang => {
                    cmd.arg("-I");
                    cmd.arg(include_dir);
                }
                crate::driver::CompilerType::Msvc => {
                    cmd.arg(&format!("/I{}", include_dir.display()));
                }
            }
        }

        // Generate dependencies
        match self.compiler_type {
            crate::driver::CompilerType::Gcc | crate::driver::CompilerType::Clang => {
                cmd.arg("-MM");  // Only user headers, not system headers
            }
            crate::driver::CompilerType::Msvc => {
                cmd.arg("/showIncludes");
            }
        }

        cmd.arg(source);

        let output = cmd.output().context("Failed to scan headers")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Header scan failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let headers = match self.compiler_type {
            crate::driver::CompilerType::Gcc | crate::driver::CompilerType::Clang => {
                self.parse_makefile_deps(&stdout)
            }
            crate::driver::CompilerType::Msvc => {
                self.parse_msvc_includes(&stdout)
            }
        };

        debug!("Found {} header dependencies", headers.len());
        Ok(headers)
    }

    /// Scan multiple source files
    pub fn scan_multiple(&self, sources: &[PathBuf], include_dirs: &[PathBuf]) -> Result<HashSet<PathBuf>> {
        let mut all_headers = HashSet::new();

        for source in sources {
            let headers = self.scan(source, include_dirs)?;
            for header in headers {
                all_headers.insert(header);
            }
        }

        Ok(all_headers)
    }

    /// Parse Makefile-style dependency output (gcc/clang)
    fn parse_makefile_deps(&self, output: &str) -> Vec<PathBuf> {
        let mut headers = Vec::new();

        // Output format: "source.o: header1.h header2.h ..."
        for line in output.lines() {
            if let Some(colon_pos) = line.find(':') {
                let deps_part = &line[colon_pos + 1..];
                for dep in deps_part.split_whitespace() {
                    if !dep.is_empty() && !dep.ends_with(":") {
                        headers.push(PathBuf::from(dep));
                    }
                }
            }
        }

        headers
    }

    /// Parse MSVC /showIncludes output
    fn parse_msvc_includes(&self, output: &str) -> Vec<PathBuf> {
        let mut headers = Vec::new();

        for line in output.lines() {
            // MSVC format: "Note: including file: C:\path\to\header.h"
            if let Some(pos) = line.find("Note: including file:") {
                let path_str = line[pos + 22..].trim();
                if !path_str.is_empty() {
                    headers.push(PathBuf::from(path_str));
                }
            }
        }

        headers
    }

    /// Check if a header has changed (for incremental builds)
    pub fn has_header_changed(
        &self,
        source: &Path,
        include_dirs: &[PathBuf],
        previous_headers: &HashSet<PathBuf>,
    ) -> Result<bool> {
        let current_headers = self.scan(source, include_dirs)?;
        let current_set: HashSet<PathBuf> = current_headers.into_iter().collect();

        // Check if any header is new or missing
        if current_set != *previous_headers {
            return Ok(true);
        }

        // Check if any header file has been modified
        for header in &current_set {
            if let Ok(metadata) = std::fs::metadata(header) {
                if let Ok(_modified) = metadata.modified() {
                    // For MVP, we just check if the file exists and is readable
                    // Full implementation would track modification times
                    if !header.exists() {
                        return Ok(true);
                    }
                }
            }
        }

        Ok(false)
    }
}
