//! Header dependency scanning for C/C++

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::debug;

/// Header dependency scanner
pub struct HeaderScanner {
    compiler_path: PathBuf,
}

impl HeaderScanner {
    /// Create a new header scanner
    pub fn new(compiler_path: PathBuf) -> Self {
        Self { compiler_path }
    }

    /// Scan a C/C++ source file for header dependencies
    pub fn scan(&self, source: &Path) -> Result<Vec<PathBuf>> {
        debug!("Scanning headers for: {:?}", source);

        // Use gcc/clang -M or -MM to generate dependencies
        let output = Command::new(&self.compiler_path)
            .arg("-MM")  // Only user headers, not system headers
            .arg(source)
            .output()
            .context("Failed to scan headers")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Header scan failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let headers = self.parse_makefile_deps(&stdout);

        debug!("Found {} header dependencies", headers.len());
        Ok(headers)
    }

    /// Parse Makefile-style dependency output
    fn parse_makefile_deps(&self, output: &str) -> Vec<PathBuf> {
        let mut headers = Vec::new();

        // Output format: "source.o: header1.h header2.h ..."
        for line in output.lines() {
            if let Some(colon_pos) = line.find(':') {
                let deps_part = &line[colon_pos + 1..];
                for dep in deps_part.split_whitespace() {
                    if !dep.is_empty() {
                        headers.push(PathBuf::from(dep));
                    }
                }
            }
        }

        headers
    }
}
