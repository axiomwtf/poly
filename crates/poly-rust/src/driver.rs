//! Rust compiler driver (direct rustc invocation)

use anyhow::{Context, Result};
use crate::graph::CrateGraph;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info};

/// Rust compiler driver
pub struct RustDriver {
    rustc_path: PathBuf,
    cargo_home: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RustTarget {
    pub name: String,
    pub crate_type: String,
    pub edition: String,
    pub features: Vec<String>,
    pub dependencies: Vec<String>,
    pub source: PathBuf,
    pub flags: Vec<String>,
    pub target_triple: Option<String>,
    /// Extra environment variables for the rustc process
    /// (CARGO_PKG_* manifest metadata, etc.)
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct BuildScript {
    pub name: String,
    pub path: PathBuf,
    pub dependencies: Vec<String>,
}

impl RustDriver {
    /// Create a new Rust driver
    pub fn new() -> Result<Self> {
        let rustc_path = Self::find_rustc()?;
        let cargo_home = Self::find_cargo_home()?;
        Ok(Self { rustc_path, cargo_home })
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

    /// Find cargo home directory
    fn find_cargo_home() -> Result<PathBuf> {
        if let Ok(path) = std::env::var("CARGO_HOME") {
            return Ok(PathBuf::from(path));
        }

        if let Ok(home) = std::env::var("HOME") {
            return Ok(PathBuf::from(home).join(".cargo"));
        }

        anyhow::bail!("Could not determine CARGO_HOME")
    }

    /// Get the registry cache path
    pub fn registry_path(&self) -> PathBuf {
        self.cargo_home.join("registry")
    }

    /// Get the git dependencies path
    pub fn git_path(&self) -> PathBuf {
        self.cargo_home.join("git")
    }

    /// Compile a Rust source file with full feature support
    pub fn compile(
        &self,
        input: &Path,
        output: &Path,
        target: &RustTarget,
        dependencies: &[PathBuf],
        build_script_output: Option<&BuildScriptOutput>,
    ) -> Result<()> {
        debug!("Compiling Rust target: {}", target.name);
        debug!("Source: {:?}", input);
        debug!("Output: {:?}", output);

        let mut cmd = Command::new(&self.rustc_path);

        // Crate type
        cmd.arg("--crate-type");
        cmd.arg(&target.crate_type);

        // Crate name (sanitized for Rust identifier rules)
        let sanitized_name = target.name.replace('-', "_").replace('.', "_");
        cmd.arg("--crate-name");
        cmd.arg(&sanitized_name);

        // Suppress non_snake_case warnings for camelCase compatibility
        cmd.arg("-A");
        cmd.arg("non_snake_case");

        // Edition
        cmd.arg("--edition");
        cmd.arg(&target.edition);

        // Target triple for cross-compilation
        if let Some(target_triple) = &target.target_triple {
            cmd.arg("--target");
            cmd.arg(target_triple);
        }

        // Features (as cfg flags)
        for feature in &target.features {
            cmd.arg("--cfg");
            cmd.arg(&format!("feature=\"{}\"", feature));
        }

        // External dependencies - use proper crate names from flags
        // The flags already contain --extern crate_name=path
        for dep in dependencies {
            cmd.arg("--extern");
            let dep_name = dep.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .trim_start_matches("lib");
            cmd.arg(&format!("{}={}", dep_name, dep.display()));
        }

        // Manifest metadata env (CARGO_PKG_*, CARGO_MANIFEST_DIR, ...)
        for (key, value) in &target.env {
            cmd.env(key, value);
        }

        // Build script outputs (env vars and cfg flags)
        if let Some(bs_output) = build_script_output {
            for (key, value) in &bs_output.env_vars {
                cmd.env(key, value);
            }
            for cfg in &bs_output.cfg_flags {
                cmd.arg("--cfg");
                cmd.arg(cfg);
            }
        }

        // Additional flags from target (includes --extern for workspace deps)
        for flag in &target.flags {
            cmd.arg(flag);
        }

        // Input and output
        cmd.arg("-o");
        cmd.arg(output);
        cmd.arg(input);

        // Windows-gnu targets need a mingw cross toolchain; rustc invokes
        // `x86_64-w64-mingw32-gcc` by name, so make sure it is on PATH.
        if target
            .target_triple
            .as_deref()
            .map(|t| t.contains("windows"))
            .unwrap_or(false)
        {
            if let Ok(home) = std::env::var("HOME") {
                let mingw_bin = PathBuf::from(home).join(".local/mingw/usr/bin");
                if mingw_bin.join("x86_64-w64-mingw32-gcc").exists() {
                    let mut paths = vec![mingw_bin];
                    if let Ok(path) = std::env::var("PATH") {
                        paths.extend(std::env::split_paths(&path));
                    }
                    if let Ok(joined) = std::env::join_paths(paths) {
                        cmd.env("PATH", joined);
                    }
                }
            }
        }

        debug!("Running: {:?}", cmd);

        let output = cmd.output().context("Failed to execute rustc")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("rustc compilation failed for {}: {}", target.name, stderr);
        }

        info!("Successfully compiled Rust target: {}", target.name);
        Ok(())
    }

    /// Compile with dependencies resolved
    pub fn compile_with_deps(
        &self,
        input: &Path,
        output: &Path,
        target: &RustTarget,
        dep_graph: &CrateGraph,
    ) -> Result<()> {
        debug!("Compiling with dependency graph");

        // Get dependencies in compilation order
        let deps = dep_graph.get_dependencies(&target.name)?;
        let dep_paths: Vec<PathBuf> = deps.iter()
            .map(|name| dep_graph.get_output_path(name))
            .collect::<Result<Vec<_>>>()?;

        // Compile dependencies first
        for dep_name in &deps {
            if let Some(dep_target) = dep_graph.get_target(dep_name) {
                let dep_output = dep_graph.get_output_path(dep_name)?;
                let dep_input = dep_target.source.clone();
                self.compile(&dep_input, &dep_output, dep_target, &[], None)?;
            }
        }

        // Compile the target
        self.compile(input, output, target, &dep_paths, None)
    }

    /// Run a build script
    pub fn run_build_script(
        &self,
        script_path: &Path,
        target_dir: &Path,
    ) -> Result<BuildScriptOutput> {
        debug!("Running build script: {:?}", script_path);

        let mut cmd = Command::new(&self.rustc_path);
        cmd.arg("--run");
        cmd.arg(script_path);
        cmd.current_dir(target_dir);

        // Set environment variables for build script
        cmd.env("OUT_DIR", target_dir.join("build"));
        cmd.env("TARGET", std::env::consts::ARCH);
        cmd.env("HOST", std::env::consts::ARCH);
        cmd.env("PROFILE", "debug"); // TODO: Get from build profile

        debug!("Running: {:?}", cmd);

        let output = cmd.output().context("Failed to run build script")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Build script failed: {}", stderr);
        }

        // Parse build script output
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut env_vars = HashMap::new();
        let mut cfg_flags = Vec::new();

        for line in stdout.lines() {
            if let Some(rest) = line.strip_prefix("cargo:") {
                if let Some((key, value)) = rest.split_once('=') {
                    if key == "rustc-cfg" {
                        cfg_flags.push(value.to_string());
                    } else if key == "rustc-env" {
                        let parts: Vec<&str> = value.splitn(2, '=').collect();
                        if parts.len() == 2 {
                            env_vars.insert(parts[0].to_string(), parts[1].to_string());
                        }
                    }
                }
            }
        }

        Ok(BuildScriptOutput {
            env_vars,
            cfg_flags,
        })
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

    /// Get target list
    pub fn targets(&self) -> Result<Vec<String>> {
        let output = Command::new(&self.rustc_path)
            .arg("--print")
            .arg("target-list")
            .output()
            .context("Failed to get target list")?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(stdout.lines().map(|s| s.to_string()).collect())
        } else {
            anyhow::bail!("Failed to get target list")
        }
    }
}

/// Output from a build script
#[derive(Debug, Clone)]
pub struct BuildScriptOutput {
    pub env_vars: HashMap<String, String>,
    pub cfg_flags: Vec<String>,
}

impl Default for RustDriver {
    fn default() -> Self {
        Self::new().expect("Failed to create RustDriver")
    }
}
