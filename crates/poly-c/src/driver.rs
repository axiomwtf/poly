//! C/C++ compiler driver (direct gcc/clang/msvc invocation)

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::debug;

/// C/C++ compiler driver
pub struct CDriver {
    compiler_path: PathBuf,
    compiler_type: CompilerType,
}

#[derive(Debug, Clone)]
pub enum CompilerType {
    Gcc,
    Clang,
    Msvc,
}

impl CDriver {
    /// Create a new C driver, auto-detecting the compiler
    pub fn new() -> Result<Self> {
        let (compiler_path, compiler_type) = Self::detect_compiler()?;
        Ok(Self {
            compiler_path,
            compiler_type,
        })
    }

    /// Detect available C compiler
    fn detect_compiler() -> Result<(PathBuf, CompilerType)> {
        // Try clang first
        if let Ok(path) = Self::find_compiler("clang") {
            debug!("Detected clang compiler");
            return Ok((path, CompilerType::Clang));
        }

        // Try gcc
        if let Ok(path) = Self::find_compiler("gcc") {
            debug!("Detected gcc compiler");
            return Ok((path, CompilerType::Gcc));
        }

        // Try cl (MSVC on Windows)
        if let Ok(path) = Self::find_compiler("cl") {
            debug!("Detected MSVC compiler");
            return Ok((path, CompilerType::Msvc));
        }

        anyhow::bail!("No C compiler found (tried clang, gcc, cl)")
    }

    fn find_compiler(name: &str) -> Result<PathBuf> {
        let output = Command::new("which")
            .arg(name)
            .output()
            .context(format!("Failed to find {}", name))?;

        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(PathBuf::from(path))
        } else {
            anyhow::bail!("{} not found in PATH", name)
        }
    }

    /// Compile a C source file
    pub fn compile(
        &self,
        input: &Path,
        output: &Path,
        flags: &[String],
        is_cpp: bool,
    ) -> Result<()> {
        debug!("Compiling {} file: {:?}", if is_cpp { "C++" } else { "C" }, input);

        let mut cmd = Command::new(&self.compiler_path);

        // Language flag
        if is_cpp {
            match self.compiler_type {
                CompilerType::Gcc | CompilerType::Clang => {
                    cmd.arg("-x");
                    cmd.arg("c++");
                }
                CompilerType::Msvc => {
                    cmd.arg("/TP");
                }
            }
        }

        // Optimization and debug flags
        for flag in flags {
            cmd.arg(flag);
        }

        // Output
        match self.compiler_type {
            CompilerType::Gcc | CompilerType::Clang => {
                cmd.arg("-c");
                cmd.arg("-o");
                cmd.arg(output);
            }
            CompilerType::Msvc => {
                cmd.arg("/c");
                cmd.arg(&format!("/Fo:{}", output.display()));
            }
        }

        // Input
        cmd.arg(input);

        debug!("Running: {:?}", cmd);

        let output = cmd.output().context("Failed to execute compiler")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Compilation failed: {}", stderr);
        }

        debug!("Compilation successful");
        Ok(())
    }

    /// Get compiler version
    pub fn version(&self) -> Result<String> {
        let output = Command::new(&self.compiler_path)
            .arg("--version")
            .output()
            .context("Failed to get compiler version")?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            anyhow::bail!("Failed to get compiler version")
        }
    }
}

impl Default for CDriver {
    fn default() -> Self {
        Self::new().expect("Failed to create CDriver")
    }
}
