//! C/C++ compiler driver (direct gcc/clang/msvc invocation)

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info};

/// C/C++ compiler driver
#[derive(Clone)]
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

#[derive(Debug, Clone)]
pub struct CTarget {
    pub name: String,
    pub language: String, // "c" or "cpp"
    pub target_type: String, // "bin", "lib", "static-lib", "shared-lib"
    pub sources: Vec<PathBuf>,
    pub headers: Vec<PathBuf>,
    pub include_dirs: Vec<PathBuf>,
    pub library_dirs: Vec<PathBuf>,
    pub libraries: Vec<String>,
    pub flags: Vec<String>,
    pub defines: HashMap<String, String>,
    pub dependencies: Vec<String>,
    /// Rust-style target triple (e.g. x86_64-pc-windows-gnu)
    pub target_triple: String,
}

#[derive(Debug, Clone)]
pub struct LinkResult {
    pub output: PathBuf,
    pub objects: Vec<PathBuf>,
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

    /// Return a driver configured for the given target triple, selecting a
    /// cross-compiler when the triple doesn't match the host.
    pub fn for_target(&self, target_triple: &str) -> Result<Self> {
        let prefix = Self::toolchain_prefix(target_triple);
        let mut driver = self.clone();

        if let Some(prefix) = prefix {
            // Try well-known install locations, then PATH.
            let candidates = [
                format!("{}gcc", prefix),
                format!("{}cc", prefix),
                format!("{}clang", prefix),
            ];
            for name in candidates {
                if let Ok(path) = Self::find_tool(&name) {
                    driver.compiler_path = path;
                    driver.compiler_type = CompilerType::Gcc;
                    break;
                }
            }
        }
        Ok(driver)
    }

    /// Map a target triple to a toolchain prefix like `x86_64-w64-mingw32-`.
    /// Returns None when the triple matches the host (no cross needed).
    fn toolchain_prefix(target_triple: &str) -> Option<String> {
        let host_is_windows = cfg!(windows);
        let host_is_linux = cfg!(target_os = "linux");
        let host_is_macos = cfg!(target_os = "macos");

        if target_triple.contains("windows") && !host_is_windows {
            let arch = if target_triple.starts_with("i686") || target_triple.starts_with("i586") {
                "i686"
            } else {
                "x86_64"
            };
            return Some(format!("{}-w64-mingw32-", arch));
        }
        if target_triple.contains("linux") && !host_is_linux {
            let arch = target_triple.split('-').next().unwrap_or("x86_64");
            return Some(format!("{}-linux-gnu-", arch));
        }
        if (target_triple.contains("apple") || target_triple.contains("darwin"))
            && !host_is_macos
        {
            let arch = target_triple.split('-').next().unwrap_or("aarch64");
            return Some(format!("{}-apple-darwin", arch));
        }
        None
    }

    /// Find a build tool, checking ~/.local/mingw/usr/bin before PATH.
    fn find_tool(name: &str) -> Result<PathBuf> {
        let local_mingw = PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join(".local/mingw/usr/bin")
            .join(name);
        if local_mingw.exists() {
            return Ok(local_mingw);
        }
        Self::find_compiler(name)
    }

    /// Derive a sibling tool name from the compiler path
    /// (`x86_64-w64-mingw32-gcc` -> `x86_64-w64-mingw32-ar`).
    fn sibling_tool(&self, tool: &str) -> PathBuf {
        let file = self
            .compiler_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("gcc");
        let prefix = file
            .strip_suffix("gcc")
            .or_else(|| file.strip_suffix("clang"))
            .or_else(|| file.strip_suffix("cc"))
            .unwrap_or("");
        let candidate = format!("{}{}", prefix, tool);
        if let Ok(path) = Self::find_tool(&candidate) {
            return path;
        }
        PathBuf::from(tool)
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

    /// Compile a C/C++ source file
    pub fn compile(
        &self,
        input: &Path,
        output: &Path,
        target: &CTarget,
    ) -> Result<()> {
        debug!("Compiling {} file: {:?}", target.language, input);

        let mut cmd = Command::new(&self.compiler_path);

        // Language flag
        let is_cpp = target.language == "cpp";
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

        // Include directories
        for include_dir in &target.include_dirs {
            match self.compiler_type {
                CompilerType::Gcc | CompilerType::Clang => {
                    cmd.arg("-I");
                    cmd.arg(include_dir);
                }
                CompilerType::Msvc => {
                    cmd.arg(&format!("/I{}", include_dir.display()));
                }
            }
        }

        // Preprocessor defines
        for (key, value) in &target.defines {
            match self.compiler_type {
                CompilerType::Gcc | CompilerType::Clang => {
                    if value.is_empty() {
                        cmd.arg(&format!("-D{}", key));
                    } else {
                        cmd.arg(&format!("-D{}={}", key, value));
                    }
                }
                CompilerType::Msvc => {
                    if value.is_empty() {
                        cmd.arg(&format!("/D{}", key));
                    } else {
                        cmd.arg(&format!("/D{}={}", key, value));
                    }
                }
            }
        }

        // Target flags
        for flag in &target.flags {
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

        let output_result = cmd.output().context("Failed to execute compiler")?;

        if !output_result.status.success() {
            let stderr = String::from_utf8_lossy(&output_result.stderr);
            anyhow::bail!("Compilation failed: {}", stderr);
        }

        debug!("Compilation successful");
        Ok(())
    }

    /// Compile all sources in a target
    pub fn compile_target(&self, target: &CTarget, output_dir: &Path) -> Result<Vec<PathBuf>> {
        debug!("Compiling target: {}", target.name);

        let mut objects = Vec::new();

        for source in &target.sources {
            let object_name = format!("{}.{}", source.file_stem().unwrap().to_str().unwrap(), "o");
            let object_path = output_dir.join(object_name);

            self.compile(source, &object_path, target)?;
            objects.push(object_path);
        }

        info!("Compiled {} objects for target {}", objects.len(), target.name);
        Ok(objects)
    }

    /// Link objects into final binary/library
    pub fn link(&self, target: &CTarget, objects: &[PathBuf], output: &Path) -> Result<LinkResult> {
        debug!("Linking target: {}", target.name);

        // Link through the compiler driver (gcc/clang handle crt objects,
        // library paths and the correct linker for the target).
        let mut cmd = Command::new(&self.compiler_path);

        // Target type determines linking mode
        match target.target_type.as_str() {
            "bin" => {
                // Executable
            }
            "lib" | "shared-lib" => {
                // Shared library
                match self.compiler_type {
                    CompilerType::Gcc | CompilerType::Clang => {
                        cmd.arg("-shared");
                    }
                    CompilerType::Msvc => {
                        cmd.arg("/DLL");
                    }
                }
            }
            "static-lib" => {
                // Static library - use ar
                return self.create_static_lib(objects, output);
            }
            _ => {}
        }

        // Library search paths
        for lib_dir in &target.library_dirs {
            match self.compiler_type {
                CompilerType::Gcc | CompilerType::Clang => {
                    cmd.arg("-L");
                    cmd.arg(lib_dir);
                }
                CompilerType::Msvc => {
                    cmd.arg(&format!("/LIBPATH:{}", lib_dir.display()));
                }
            }
        }

        // Libraries
        for lib in &target.libraries {
            match self.compiler_type {
                CompilerType::Gcc | CompilerType::Clang => {
                    cmd.arg(&format!("-l{}", lib));
                }
                CompilerType::Msvc => {
                    cmd.arg(&format!("{}.lib", lib));
                }
            }
        }

        // Output
        match self.compiler_type {
            CompilerType::Gcc | CompilerType::Clang => {
                cmd.arg("-o");
                cmd.arg(output);
            }
            CompilerType::Msvc => {
                cmd.arg(&format!("/OUT:{}", output.display()));
            }
        }

        // Object files
        for obj in objects {
            cmd.arg(obj);
        }

        debug!("Running: {:?}", cmd);

        let output_result = cmd.output().context("Failed to execute linker")?;

        if !output_result.status.success() {
            let stderr = String::from_utf8_lossy(&output_result.stderr);
            anyhow::bail!("Linking failed: {}", stderr);
        }

        info!("Linked target: {} -> {:?}", target.name, output);
        Ok(LinkResult {
            output: output.to_path_buf(),
            objects: objects.to_vec(),
        })
    }

    /// Create a static library from objects
    fn create_static_lib(&self, objects: &[PathBuf], output: &Path) -> Result<LinkResult> {
        debug!("Creating static library: {:?}", output);

        let mut cmd = Command::new(self.sibling_tool("ar"));
        cmd.arg("rcs");
        cmd.arg(output);

        for obj in objects {
            cmd.arg(obj);
        }

        debug!("Running: {:?}", cmd);

        let output_result = cmd.output().context("Failed to execute ar")?;

        if !output_result.status.success() {
            let stderr = String::from_utf8_lossy(&output_result.stderr);
            anyhow::bail!("Static library creation failed: {}", stderr);
        }

        info!("Created static library: {:?}", output);
        Ok(LinkResult {
            output: output.to_path_buf(),
            objects: objects.to_vec(),
        })
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

    /// Get supported targets
    pub fn targets(&self) -> Result<Vec<String>> {
        match self.compiler_type {
            CompilerType::Gcc | CompilerType::Clang => {
                let output = Command::new(&self.compiler_path)
                    .arg("-print-targets")
                    .output()
                    .context("Failed to get targets")?;

                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    Ok(stdout.lines()
                        .filter(|line| line.contains("known to gcc"))
                        .map(|line| line.split_whitespace().next().unwrap_or("").to_string())
                        .collect())
                } else {
                    anyhow::bail!("Failed to get targets")
                }
            }
            CompilerType::Msvc => {
                // MSVC doesn't have a simple way to list targets
                Ok(Vec::new())
            }
        }
    }
}

impl Default for CDriver {
    fn default() -> Self {
        Self::new().expect("Failed to create CDriver")
    }
}
