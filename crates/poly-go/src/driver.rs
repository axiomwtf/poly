//! Go compiler driver (direct go tool invocation)

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info};

/// Go compiler driver
pub struct GoDriver {
    go_path: PathBuf,
    gopath: PathBuf,
    gomodcache: PathBuf,
}

#[derive(Debug, Clone)]
pub struct GoTarget {
    pub name: String,
    pub module: String,
    pub package: String,
    pub sources: Vec<PathBuf>,
    pub dependencies: Vec<String>,
    pub cgo: bool,
    pub build_tags: Vec<String>,
    pub flags: Vec<String>,
    pub target_type: String, // "bin", "lib", "plugin"
}

#[derive(Debug, Clone)]
pub struct GoModule {
    pub path: String,
    pub version: String,
    pub dependencies: HashMap<String, String>,
}

impl GoDriver {
    /// Create a new Go driver
    pub fn new() -> Result<Self> {
        let go_path = Self::find_go()?;
        let gopath = Self::find_gopath()?;
        let gomodcache = Self::find_gomodcache(&gopath)?;
        Ok(Self { go_path, gopath, gomodcache })
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

    /// Find GOPATH
    fn find_gopath() -> Result<PathBuf> {
        if let Ok(path) = std::env::var("GOPATH") {
            return Ok(PathBuf::from(path));
        }

        if let Ok(home) = std::env::var("HOME") {
            return Ok(PathBuf::from(home).join("go"));
        }

        anyhow::bail!("Could not determine GOPATH")
    }

    /// Find Go module cache
    fn find_gomodcache(gopath: &Path) -> Result<PathBuf> {
        Ok(gopath.join("pkg").join("mod"))
    }

    /// Get the module cache path
    pub fn module_cache(&self) -> &Path {
        &self.gomodcache
    }

    /// Compile Go source files with full module support
    pub fn compile(
        &self,
        target: &GoTarget,
        output: &Path,
        work_dir: &Path,
    ) -> Result<()> {
        debug!("Compiling Go target: {}", target.name);
        debug!("Module: {}", target.module);
        debug!("Package: {}", target.package);

        // Set up environment
        let mut env = std::collections::HashMap::new();
        env.insert("GOPATH".to_string(), self.gopath.display().to_string());
        env.insert("GO111MODULE".to_string(), "on".to_string());
        env.insert("CGO_ENABLED".to_string(), if target.cgo { "1" } else { "0" }.to_string());

        // Create temporary go.mod if needed
        let go_mod_path = work_dir.join("go.mod");
        self.create_go_mod(&go_mod_path, target)?;

        // Download dependencies
        self.download_dependencies(work_dir, target)?;

        // Compile the package
        let mut cmd = Command::new(&self.go_path);
        cmd.arg("build");
        cmd.arg("-o");
        cmd.arg(output);

        // Build tags
        for tag in &target.build_tags {
            cmd.arg("-tags");
            cmd.arg(tag);
        }

        // Additional flags
        for flag in &target.flags {
            cmd.arg(flag);
        }

        // Package path
        cmd.arg(&target.package);

        cmd.current_dir(work_dir);

        // Set environment variables
        for (key, value) in &env {
            cmd.env(key, value);
        }

        debug!("Running: {:?}", cmd);

        let output_result = cmd.output().context("Failed to execute go build")?;

        if !output_result.status.success() {
            let stderr = String::from_utf8_lossy(&output_result.stderr);
            anyhow::bail!("Go compilation failed: {}", stderr);
        }

        info!("Successfully compiled Go target: {}", target.name);
        Ok(())
    }

    /// Create go.mod file for the target
    fn create_go_mod(&self, path: &Path, target: &GoTarget) -> Result<()> {
        let mut content = String::new();
        content.push_str(&format!("module {}\n\n", target.module));
        content.push_str("go 1.21\n\n");

        if !target.dependencies.is_empty() {
            content.push_str("require (\n");
            for dep in &target.dependencies {
                // Parse dependency specification
                let parts: Vec<&str> = dep.split(':').collect();
                if parts.len() == 2 {
                    let name = parts[0];
                    let version = parts[1];
                    content.push_str(&format!("\t{} {}\n", name, version));
                }
            }
            content.push_str(")\n");
        }

        std::fs::write(path, content)?;
        debug!("Created go.mod at {:?}", path);
        Ok(())
    }

    /// Download Go dependencies
    fn download_dependencies(&self, work_dir: &Path, target: &GoTarget) -> Result<()> {
        if target.dependencies.is_empty() {
            return Ok(());
        }

        debug!("Downloading Go dependencies");

        let mut cmd = Command::new(&self.go_path);
        cmd.arg("mod");
        cmd.arg("download");
        cmd.current_dir(work_dir);

        // Set environment
        cmd.env("GOPATH", &self.gopath);
        cmd.env("GO111MODULE", "on");

        debug!("Running: {:?}", cmd);

        let output = cmd.output().context("Failed to download dependencies")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to download Go dependencies: {}", stderr);
        }

        info!("Downloaded Go dependencies");
        Ok(())
    }

    /// Compile with CGO support
    pub fn compile_with_cgo(
        &self,
        target: &GoTarget,
        output: &Path,
        work_dir: &Path,
        cgo_flags: &[String],
    ) -> Result<()> {
        if !target.cgo {
            return self.compile(target, output, work_dir);
        }

        debug!("Compiling with CGO support");

        // Set CGO environment variables
        let mut cmd = Command::new(&self.go_path);
        cmd.arg("build");
        cmd.arg("-o");
        cmd.arg(output);

        // CGO flags
        for flag in cgo_flags {
            cmd.arg("-ldflags");
            cmd.arg(flag);
        }

        // Build tags
        for tag in &target.build_tags {
            cmd.arg("-tags");
            cmd.arg(tag);
        }

        cmd.arg(&target.package);
        cmd.current_dir(work_dir);

        // Set environment
        cmd.env("GOPATH", &self.gopath);
        cmd.env("GO111MODULE", "on");
        cmd.env("CGO_ENABLED", "1");

        debug!("Running: {:?}", cmd);

        let output_result = cmd.output().context("Failed to execute go build with CGO")?;

        if !output_result.status.success() {
            let stderr = String::from_utf8_lossy(&output_result.stderr);
            anyhow::bail!("Go CGO compilation failed: {}", stderr);
        }

        info!("Successfully compiled Go target with CGO: {}", target.name);
        Ok(())
    }

    /// Get Go version
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

    /// Get Go environment
    pub fn env(&self) -> Result<HashMap<String, String>> {
        let output = Command::new(&self.go_path)
            .arg("env")
            .output()
            .context("Failed to get go env")?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut env = HashMap::new();

            for line in stdout.lines() {
                if let Some((key, value)) = line.split_once('=') {
                    env.insert(key.to_string(), value.to_string());
                }
            }

            Ok(env)
        } else {
            anyhow::bail!("Failed to get go env")
        }
    }

    /// List available Go versions
    pub fn versions(&self) -> Result<Vec<String>> {
        // This would require network access to fetch from go.dev
        // For now, return a placeholder
        Ok(vec!["1.21".to_string(), "1.22".to_string()])
    }
}

impl Default for GoDriver {
    fn default() -> Self {
        Self::new().expect("Failed to create GoDriver")
    }
}
