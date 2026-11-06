//! Go compiler driver.
//!
//! Delegation boundary (polyspec §7.3): pure-Go packages build through
//! `go tool compile`/`go tool link` directly (`compile_native`). The `go`
//! command is invoked only where no tool-level equivalent exists — `go list`
//! for dependency export data, `go build` for cgo/module-mode builds,
//! `go test`/`go vet` for test and lint.

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
    /// Target GOOS (e.g. "linux", "windows"); None = host
    pub goos: Option<String>,
    /// Target GOARCH (e.g. "amd64"); None = host
    pub goarch: Option<String>,
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
            return Ok(PathBuf::from(path));
        }

        // Fallback: user-local Go installation
        if let Ok(home) = std::env::var("HOME") {
            let local = PathBuf::from(home).join(".local/go/bin/go");
            if local.exists() {
                return Ok(local);
            }
        }

        anyhow::bail!("go not found in PATH")
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
        if let Some(goos) = &target.goos {
            env.insert("GOOS".to_string(), goos.clone());
        }
        if let Some(goarch) = &target.goarch {
            env.insert("GOARCH".to_string(), goarch.clone());
        }

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

        self.apply_windows_toolchain(&mut cmd, target);

        debug!("Running: {:?}", cmd);

        let output_result = cmd.output().context("Failed to execute go build")?;

        if !output_result.status.success() {
            let stderr = String::from_utf8_lossy(&output_result.stderr);
            anyhow::bail!("Go compilation failed: {}", stderr);
        }

        info!("Successfully compiled Go target: {}", target.name);
        Ok(())
    }

    /// For windows targets, cgo needs the mingw cross toolchain: point CC at
    /// x86_64-w64-mingw32-gcc and put the locally extracted mingw bin dir on
    /// PATH (same toolchain the Rust driver uses).
    fn apply_windows_toolchain(&self, cmd: &mut Command, target: &GoTarget) {
        if target.goos.as_deref() != Some("windows") {
            return;
        }
        cmd.env("CC", "x86_64-w64-mingw32-gcc");
        if let Ok(home) = std::env::var("HOME") {
            let mingw_bin = std::path::PathBuf::from(home).join(".local/mingw/usr/bin");
            if mingw_bin.exists() {
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

    /// Spec §7.3: compile with `go tool compile` + `go tool link` directly.
    /// No go.mod — sources are staged into GOPATH layout and importcfg is
    /// generated from `go list -export`. Only for pure-Go packages; files
    /// importing "C" need the cgo pipeline (`go build` path).
    pub fn compile_native(
        &self,
        target: &GoTarget,
        output: &Path,
        work_dir: &Path,
    ) -> Result<()> {
        debug!("Native Go toolchain build: {}", target.name);

        let import_path = target.module.trim_start_matches("./").to_string();
        // Stage into a per-build GOPATH so we never pollute the user's.
        // Must be absolute: `go list` rejects relative GOPATH entries.
        let gopath = std::path::absolute(work_dir)?.join(".gopath");
        let pkg_dir = gopath.join("src").join(&import_path);
        if pkg_dir.exists() {
            std::fs::remove_dir_all(&pkg_dir)?;
        }
        std::fs::create_dir_all(&pkg_dir)?;

        let mut go_files = Vec::new();
        for src in &target.sources {
            if let Some(name) = src.file_name() {
                let dest = pkg_dir.join(name);
                std::fs::copy(src, &dest)?;
                go_files.push(dest);
            }
        }
        if go_files.is_empty() {
            anyhow::bail!("No Go source files for target {}", target.name);
        }

        let mut env = |cmd: &mut Command| {
            cmd.env("GO111MODULE", "off")
                .env("GOPATH", &gopath)
                .env("CGO_ENABLED", "0");
            if let Some(goos) = &target.goos {
                cmd.env("GOOS", goos);
            }
            if let Some(goarch) = &target.goarch {
                cmd.env("GOARCH", goarch);
            }
        };

        // importcfg: map every transitive dependency to its export .a
        let mut list = Command::new(&self.go_path);
        list.arg("list")
            .arg("-export")
            .arg("-deps")
            .arg("-f")
            .arg("{{.ImportPath}}|{{.Export}}")
            .arg(&import_path)
            .current_dir(&pkg_dir);
        env(&mut list);
        self.apply_windows_toolchain(&mut list, target);
        let list_out = list.output().context("go list failed")?;
        if !list_out.status.success() {
            anyhow::bail!(
                "go list failed: {}",
                String::from_utf8_lossy(&list_out.stderr)
            );
        }
        let mut importcfg = String::new();
        for line in String::from_utf8_lossy(&list_out.stdout).lines() {
            if let Some((pkg, export)) = line.split_once('|') {
                if !export.is_empty() && pkg != import_path {
                    importcfg.push_str(&format!("packagefile {}={}\n", pkg, export));
                }
            }
        }
        let importcfg_path = work_dir.join("importcfg.txt");
        std::fs::write(&importcfg_path, &importcfg)?;

        // `package main` must compile with -p main or the linker can't find
        // the entrypoint symbol.
        let compile_pkg = if target.target_type == "bin" {
            "main".to_string()
        } else {
            import_path.clone()
        };

        // go tool compile -> package archive
        let obj = work_dir.join(format!("{}.a", target.name.replace('-', "_")));
        let mut compile = Command::new(&self.go_path);
        compile
            .arg("tool")
            .arg("compile")
            .arg("-o")
            .arg(&obj)
            .arg("-p")
            .arg(&compile_pkg)
            .arg("-importcfg")
            .arg(&importcfg_path);
        for f in &go_files {
            compile.arg(f);
        }
        env(&mut compile);
        self.apply_windows_toolchain(&mut compile, target);
        let out = compile.output().context("go tool compile failed")?;
        if !out.status.success() {
            anyhow::bail!(
                "go tool compile failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        // go tool link -> executable (or c-archive/shared for lib targets)
        if target.target_type == "bin" {
            let mut link = Command::new(&self.go_path);
            link.arg("tool")
                .arg("link")
                .arg("-o")
                .arg(output)
                .arg("-importcfg")
                .arg(&importcfg_path);
            if let Some(goos) = &target.goos {
                link.arg("-H").arg(match goos.as_str() {
                    "windows" => "windows",
                    _ => "linux",
                });
            }
            link.arg(&obj);
            env(&mut link);
            self.apply_windows_toolchain(&mut link, target);
            let out = link.output().context("go tool link failed")?;
            if !out.status.success() {
                anyhow::bail!(
                    "go tool link failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
        } else {
            // Library targets: the compiled .a is the artifact.
            std::fs::copy(&obj, output)?;
        }

        info!("Native Go build complete: {}", target.name);
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

        let go_mod_path = work_dir.join("go.mod");
        self.create_go_mod(&go_mod_path, target)?;
        self.download_dependencies(work_dir, target)?;

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
        if let Some(goos) = &target.goos {
            cmd.env("GOOS", goos);
        }
        if let Some(goarch) = &target.goarch {
            cmd.env("GOARCH", goarch);
        }

        self.apply_windows_toolchain(&mut cmd, target);

        debug!("Running: {:?}", cmd);

        let output_result = cmd.output().context("Failed to execute go build with CGO")?;

        if !output_result.status.success() {
            let stderr = String::from_utf8_lossy(&output_result.stderr);
            anyhow::bail!("Go CGO compilation failed: {}", stderr);
        }

        info!("Successfully compiled Go target with CGO: {}", target.name);
        Ok(())
    }

    /// Build the package as a C archive (`-buildmode=c-archive`) so its
    /// code can be linked into an executable produced by another language.
    /// Produces a static `.a` plus a `.h` export header next to `output`.
    pub fn build_c_archive(
        &self,
        target: &GoTarget,
        output: &Path,
        work_dir: &Path,
    ) -> Result<()> {
        debug!("Building Go target as C archive: {}", target.name);

        let mut env = std::collections::HashMap::new();
        env.insert("GOPATH".to_string(), self.gopath.display().to_string());
        env.insert("GO111MODULE".to_string(), "on".to_string());
        env.insert("CGO_ENABLED".to_string(), "1".to_string());
        if let Some(goos) = &target.goos {
            env.insert("GOOS".to_string(), goos.clone());
        }
        if let Some(goarch) = &target.goarch {
            env.insert("GOARCH".to_string(), goarch.clone());
        }

        let go_mod_path = work_dir.join("go.mod");
        self.create_go_mod(&go_mod_path, target)?;
        self.download_dependencies(work_dir, target)?;

        let mut cmd = Command::new(&self.go_path);
        cmd.arg("build");
        cmd.arg("-buildmode=c-archive");
        cmd.arg("-o");
        cmd.arg(output);

        for tag in &target.build_tags {
            cmd.arg("-tags");
            cmd.arg(tag);
        }
        for flag in &target.flags {
            cmd.arg(flag);
        }
        cmd.arg(&target.package);
        cmd.current_dir(work_dir);

        for (key, value) in &env {
            cmd.env(key, value);
        }

        self.apply_windows_toolchain(&mut cmd, target);

        debug!("Running: {:?}", cmd);
        let output_result = cmd
            .output()
            .context("Failed to execute go build -buildmode=c-archive")?;

        if !output_result.status.success() {
            let stderr = String::from_utf8_lossy(&output_result.stderr);
            anyhow::bail!("Go c-archive build failed: {}", stderr);
        }

        info!("Built Go C archive: {}", target.name);
        Ok(())
    }

    /// Run `go test` on a prepared module work dir. Returns pass/fail.
    pub fn test_package(&self, work_dir: &Path, package: &str, goos: Option<&str>) -> Result<bool> {
        let mut cmd = Command::new(&self.go_path);
        cmd.arg("test").arg(package);
        cmd.current_dir(work_dir);
        cmd.env("GOPATH", &self.gopath);
        cmd.env("GO111MODULE", "on");
        cmd.env("CGO_ENABLED", "0");
        if let Some(goos) = goos {
            cmd.env("GOOS", goos);
        }
        let out = cmd.output().context("Failed to run go test")?;
        print!("{}", String::from_utf8_lossy(&out.stdout));
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
        Ok(out.status.success())
    }

    /// Run `go vet` on a prepared module work dir.
    pub fn vet_package(&self, work_dir: &Path, package: &str) -> Result<std::process::Output> {
        let mut cmd = Command::new(&self.go_path);
        cmd.arg("vet").arg(package);
        cmd.current_dir(work_dir);
        cmd.env("GOPATH", &self.gopath);
        cmd.env("GO111MODULE", "on");
        cmd.output().context("Failed to run go vet")
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
