//! Unified linker orchestration

use anyhow::{Context, Result};
use poly_c::CDriver;
use poly_go::GoDriver;
use poly_rust::RustDriver;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info};

/// Unified linker for all languages
pub struct Linker {
    // Drivers are kept for future use when we need more sophisticated linking
    #[allow(dead_code)]
    rust_driver: RustDriver,
    #[allow(dead_code)]
    c_driver: CDriver,
    #[allow(dead_code)]
    go_driver: GoDriver,
}

#[derive(Debug, Clone)]
pub struct LinkTarget {
    pub name: String,
    pub language: String,
    pub objects: Vec<PathBuf>,
    pub libraries: Vec<String>,
    pub library_dirs: Vec<PathBuf>,
    pub include_dirs: Vec<PathBuf>,
    pub flags: Vec<String>,
    pub target_type: String, // "bin", "lib", "static-lib", "shared-lib"
}

#[derive(Debug, Clone)]
pub struct LinkResult {
    pub output: PathBuf,
    pub objects: Vec<PathBuf>,
    pub linked_libraries: Vec<String>,
}

impl Linker {
    /// Create a new linker with all language drivers
    pub fn new() -> Result<Self> {
        Ok(Self {
            rust_driver: RustDriver::new()?,
            c_driver: CDriver::new()?,
            go_driver: GoDriver::new()?,
        })
    }

    /// Link a target with objects from multiple languages
    pub fn link(&self, target: &LinkTarget, output: &Path) -> Result<LinkResult> {
        debug!("Linking target: {}", target.name);
        debug!("Language: {}", target.language);
        debug!("Objects: {:?}", target.objects);

        // Determine primary language for linking
        let primary_lang = self.determine_primary_language(target)?;

        match primary_lang.as_str() {
            "rust" => self.link_with_rust(target, output),
            "c" | "cpp" => self.link_with_c(target, output),
            "go" => self.link_with_go(target, output),
            _ => anyhow::bail!("Unsupported primary language: {}", primary_lang),
        }
    }

    /// Determine which language should handle linking
    fn determine_primary_language(&self, target: &LinkTarget) -> Result<String> {
        // Rust can link C objects, so prefer Rust if present
        if target.language == "rust" {
            return Ok("rust".to_string());
        }

        // C/C++ can link Go objects via CGO
        if target.language == "c" || target.language == "cpp" {
            return Ok(target.language.clone());
        }

        // Go can link C objects via CGO
        if target.language == "go" {
            return Ok("go".to_string());
        }

        // Default to C linker
        Ok("c".to_string())
    }

    /// Link using Rust as primary linker
    fn link_with_rust(&self, target: &LinkTarget, output: &Path) -> Result<LinkResult> {
        debug!("Linking with Rust as primary linker");

        // Rust can link C objects directly
        // For Go objects, we need to use CGO or external linking

        let mut cmd = Command::new("rustc");

        // Crate type
        cmd.arg("--crate-type");
        cmd.arg(match target.target_type.as_str() {
            "bin" => "bin",
            "lib" => "rlib",
            "static-lib" => "staticlib",
            "shared-lib" => "cdylib",
            _ => "bin",
        });

        // Output
        cmd.arg("-o");
        cmd.arg(output);

        // Add all objects
        for obj in &target.objects {
            cmd.arg(obj);
        }

        // Add library paths
        for lib_dir in &target.library_dirs {
            cmd.arg("-L");
            cmd.arg(lib_dir);
        }

        // Add libraries
        for lib in &target.libraries {
            cmd.arg("-l");
            cmd.arg(lib);
        }

        // Add flags
        for flag in &target.flags {
            cmd.arg(flag);
        }

        debug!("Running: {:?}", cmd);

        let output_result = cmd.output().context("Failed to execute rustc linker")?;

        if !output_result.status.success() {
            let stderr = String::from_utf8_lossy(&output_result.stderr);
            anyhow::bail!("Rust linking failed: {}", stderr);
        }

        info!("Linked with Rust: {:?}", output);
        Ok(LinkResult {
            output: output.to_path_buf(),
            objects: target.objects.clone(),
            linked_libraries: target.libraries.clone(),
        })
    }

    /// Link using C/C++ as primary linker
    fn link_with_c(&self, target: &LinkTarget, output: &Path) -> Result<LinkResult> {
        debug!("Linking with C/C++ as primary linker");

        let mut cmd = Command::new("gcc"); // Default to gcc, could be clang

        // Target type
        match target.target_type.as_str() {
            "bin" => {
                // Default executable
            }
            "shared-lib" => {
                cmd.arg("-shared");
            }
            "static-lib" => {
                // Use ar for static libraries
                return self.create_static_lib(target, output);
            }
            _ => {}
        }

        // Output
        cmd.arg("-o");
        cmd.arg(output);

        // Add all objects
        for obj in &target.objects {
            cmd.arg(obj);
        }

        // Add library paths
        for lib_dir in &target.library_dirs {
            cmd.arg("-L");
            cmd.arg(lib_dir);
        }

        // Add libraries
        for lib in &target.libraries {
            cmd.arg("-l");
            cmd.arg(lib);
        }

        // Add flags
        for flag in &target.flags {
            cmd.arg(flag);
        }

        debug!("Running: {:?}", cmd);

        let output_result = cmd.output().context("Failed to execute C linker")?;

        if !output_result.status.success() {
            let stderr = String::from_utf8_lossy(&output_result.stderr);
            anyhow::bail!("C linking failed: {}", stderr);
        }

        info!("Linked with C/C++: {:?}", output);
        Ok(LinkResult {
            output: output.to_path_buf(),
            objects: target.objects.clone(),
            linked_libraries: target.libraries.clone(),
        })
    }

    /// Link using Go as primary linker
    fn link_with_go(&self, target: &LinkTarget, output: &Path) -> Result<LinkResult> {
        debug!("Linking with Go as primary linker");

        // Go uses its own linker, but can link C objects via CGO
        // For simplicity, we'll use the go tool link command

        let mut cmd = Command::new("go");
        cmd.arg("tool");
        cmd.arg("link");

        // Output
        cmd.arg("-o");
        cmd.arg(output);

        // Add all objects
        for obj in &target.objects {
            cmd.arg(obj);
        }

        // Add flags
        for flag in &target.flags {
            cmd.arg(flag);
        }

        debug!("Running: {:?}", cmd);

        let output_result = cmd.output().context("Failed to execute Go linker")?;

        if !output_result.status.success() {
            let stderr = String::from_utf8_lossy(&output_result.stderr);
            anyhow::bail!("Go linking failed: {}", stderr);
        }

        info!("Linked with Go: {:?}", output);
        Ok(LinkResult {
            output: output.to_path_buf(),
            objects: target.objects.clone(),
            linked_libraries: target.libraries.clone(),
        })
    }

    /// Create a static library
    fn create_static_lib(&self, target: &LinkTarget, output: &Path) -> Result<LinkResult> {
        debug!("Creating static library: {:?}", output);

        let mut cmd = Command::new("ar");
        cmd.arg("rcs");
        cmd.arg(output);

        for obj in &target.objects {
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
            objects: target.objects.clone(),
            linked_libraries: target.libraries.clone(),
        })
    }

    /// Link multiple targets together
    pub fn link_multiple(&self, targets: &[LinkTarget], output: &Path) -> Result<LinkResult> {
        debug!("Linking {} targets into single output", targets.len());

        // Collect all objects from all targets
        let mut all_objects = Vec::new();
        let mut all_libraries = Vec::new();
        let mut all_library_dirs = Vec::new();
        let mut all_flags = Vec::new();

        for target in targets {
            all_objects.extend(target.objects.clone());
            all_libraries.extend(target.libraries.clone());
            all_library_dirs.extend(target.library_dirs.clone());
            all_flags.extend(target.flags.clone());
        }

        // Determine primary language (prefer Rust > C > Go)
        let primary_lang = if targets.iter().any(|t| t.language == "rust") {
            "rust"
        } else if targets.iter().any(|t| t.language == "c" || t.language == "cpp") {
            "c"
        } else {
            "go"
        };

        // Create a combined target
        let combined_target = LinkTarget {
            name: "combined".to_string(),
            language: primary_lang.to_string(),
            objects: all_objects,
            libraries: all_libraries,
            library_dirs: all_library_dirs,
            include_dirs: Vec::new(),
            flags: all_flags,
            target_type: "bin".to_string(),
        };

        self.link(&combined_target, output)
    }
}

impl Default for Linker {
    fn default() -> Self {
        Self::new().expect("Failed to create Linker")
    }
}
