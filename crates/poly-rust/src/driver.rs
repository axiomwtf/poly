//! Rust compiler driver (direct rustc invocation)

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::debug;

/// Rust compiler driver
pub struct RustDriver {
    rustc_path: PathBuf,
}

impl RustDriver {
    /// Create a new Rust driver
    pub fn new() -> Result<Self> {
        let rustc_path = Self::find_rustc()?;
        Ok(Self { rustc_path })
    }

    /// Find rustc in PATH
    fn find_rustc() -> Result<PathBuf> {
        let output = Command::new("which")
            .arg("rustc")
            .output()
            .context("Failed to find rustc")?;

        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(PathBuf::from(path))
        } else {
            anyhow::bail!("rustc not found in PATH")
        }
    }

    /// Compile a Rust source file
    pub fn compile(
        &self,
        input: &Path,
        output: &Path,
        edition: Option<&str>,
        features: &[String],
        flags: &[String],
        crate_type: Option<&str>,
    ) -> Result<()> {
        debug!("Compiling Rust file: {:?}", input);

        let mut cmd = Command::new(&self.rustc_path);

        // Crate type
        if let Some(ct) = crate_type {
            cmd.arg("--crate-type");
            cmd.arg(ct);
        } else {
            cmd.arg("--crate-type");
            cmd.arg("lib");
        }

        // Suppress non_snake_case warnings for camelCase compatibility
        cmd.arg("-A");
        cmd.arg("non_snake_case");

        // Edition
        if let Some(ed) = edition {
            cmd.arg("--edition");
            cmd.arg(ed);
        }

        // Features (for MVP, we'll pass them as cfg flags)
        for feature in features {
            cmd.arg("--cfg");
            cmd.arg(&format!("feature=\"{}\"", feature));
        }

        // Additional flags
        for flag in flags {
            cmd.arg(flag);
        }

        // Input and output
        cmd.arg("-o");
        cmd.arg(output);
        cmd.arg(input);

        debug!("Running: {:?}", cmd);

        let output = cmd.output().context("Failed to execute rustc")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("rustc compilation failed: {}", stderr);
        }

        debug!("Compilation successful");
        Ok(())
    }

    /// Get rustc version
    pub fn version(&self) -> Result<String> {
        let output = Command::new(&self.rustc_path)
            .arg("--version")
            .output()
            .context("Failed to get rustc version")?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            anyhow::bail!("Failed to get rustc version")
        }
    }
}

impl Default for RustDriver {
    fn default() -> Self {
        Self::new().expect("Failed to create RustDriver")
    }
}
