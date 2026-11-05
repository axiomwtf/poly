//! Go compiler driver (direct go tool invocation)

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::debug;

/// Go compiler driver
pub struct GoDriver {
    go_path: PathBuf,
}

impl GoDriver {
    /// Create a new Go driver
    pub fn new() -> Result<Self> {
        let go_path = Self::find_go()?;
        Ok(Self { go_path })
    }

    /// Find go tool in PATH
    fn find_go() -> Result<PathBuf> {
        let output = Command::new("which")
            .arg("go")
            .output()
            .context("Failed to find go")?;

        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(PathBuf::from(path))
        } else {
            anyhow::bail!("go not found in PATH")
        }
    }

    /// Compile Go source files
    pub fn compile(
        &self,
        inputs: &[PathBuf],
        output: &Path,
        module: Option<&str>,
        cgo: bool,
        flags: &[String],
    ) -> Result<()> {
        debug!("Compiling Go files: {:?}", inputs);

        let mut cmd = Command::new(&self.go_path);
        cmd.arg("tool");
        cmd.arg("compile");

        // CGO support
        if cgo {
            unsafe { std::env::set_var("CGO_ENABLED", "1") };
        } else {
            unsafe { std::env::set_var("CGO_ENABLED", "0") };
        }

        // Module path
        if let Some(mod_path) = module {
            cmd.arg("-p");
            cmd.arg(mod_path);
        }

        // Additional flags
        for flag in flags {
            cmd.arg(flag);
        }

        // Output
        cmd.arg("-o");
        cmd.arg(output);

        // Input files
        for input in inputs {
            cmd.arg(input);
        }

        debug!("Running: {:?}", cmd);

        let output = cmd.output().context("Failed to execute go tool compile")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Go compilation failed: {}", stderr);
        }

        debug!("Compilation successful");
        Ok(())
    }

    /// Link Go object files
    pub fn link(
        &self,
        inputs: &[PathBuf],
        output: &Path,
        module: Option<&str>,
        flags: &[String],
    ) -> Result<()> {
        debug!("Linking Go files to: {:?}", output);

        let mut cmd = Command::new(&self.go_path);
        cmd.arg("tool");
        cmd.arg("link");

        // Module path
        if let Some(mod_path) = module {
            cmd.arg("-p");
            cmd.arg(mod_path);
        }

        // Additional flags
        for flag in flags {
            cmd.arg(flag);
        }

        // Output
        cmd.arg("-o");
        cmd.arg(output);

        // Input files
        for input in inputs {
            cmd.arg(input);
        }

        debug!("Running: {:?}", cmd);

        let output = cmd.output().context("Failed to execute go tool link")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Go linking failed: {}", stderr);
        }

        debug!("Linking successful");
        Ok(())
    }

    /// Get go version
    pub fn version(&self) -> Result<String> {
        let output = Command::new(&self.go_path)
            .arg("version")
            .output()
            .context("Failed to get go version")?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            anyhow::bail!("Failed to get go version")
        }
    }
}

impl Default for GoDriver {
    fn default() -> Self {
        Self::new().expect("Failed to create GoDriver")
    }
}
