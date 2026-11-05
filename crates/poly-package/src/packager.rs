//! Package packager for creating distributable archives
//!
//! Per spec §8.1 a package is ONE VFSX archive named
//! `{name}-{version}.vfsx.tar.zst` containing:
//!   bin/<platform-slug>/<executables>   (one dir per platform)
//!   src/                                (when include_docs = true)
//!   poly.lock
//!   package.json                        (metadata)

use anyhow::Result;
use poly_vfsx::Vfsx;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

/// Package configuration
#[derive(Debug, Clone)]
pub struct PackageConfig {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub authors: Vec<String>,
    pub license: String,
    /// Platform slugs to include (e.g. `x86_64-linux`). Empty = all found.
    pub platforms: Vec<String>,
    pub include_docs: bool,
    pub compression: String,
    /// Sibling package dependencies (crate names) recorded in package.json.
    pub dependencies: Vec<String>,
}

/// Package packager
pub struct Packager {
    config: PackageConfig,
    /// Project root (contains poly.ini, poly.lock, sources)
    project_root: PathBuf,
    /// dist/ directory containing {profile}/{os}/{triple}/ outputs
    dist_dir: PathBuf,
    /// Build profile to package ("debug" or "release")
    profile: String,
}

impl Packager {
    /// Create a new packager
    pub fn new(
        config: PackageConfig,
        project_root: PathBuf,
        dist_dir: PathBuf,
        profile: String,
    ) -> Self {
        Self {
            config,
            project_root,
            dist_dir,
            profile,
        }
    }

    /// Create the package archive. Returns the archive path.
    pub async fn create_package(&self) -> Result<PathBuf> {
        info!(
            "Creating package {}-{} ({})",
            self.config.name, self.config.version, self.profile
        );

        let profile_dir = self.dist_dir.join(&self.profile);
        if !profile_dir.exists() {
            anyhow::bail!(
                "No build output at {} — run `poly build` first",
                profile_dir.display()
            );
        }

        // Staging directory for the package contents
        let staging = self
            .dist_dir
            .join(format!(".package_{}", std::process::id()));
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        fs::create_dir_all(staging.join("bin"))?;

        // Collect executables per platform slug: bin/<arch>-<os>/<file>
        let mut packaged = 0usize;
        for os_entry in fs::read_dir(&profile_dir)? {
            let os_entry = os_entry?;
            let os_dir = os_entry.path();
            if !os_dir.is_dir() {
                continue;
            }
            let os_name = os_entry.file_name().to_string_lossy().to_string();
            if os_name == "deps" {
                continue;
            }

            // Each os dir contains per-triple subdirectories
            for triple_entry in fs::read_dir(&os_dir)? {
                let triple_entry = triple_entry?;
                let triple_dir = triple_entry.path();
                if !triple_dir.is_dir() {
                    continue;
                }
                let triple = triple_entry.file_name().to_string_lossy().to_string();
                let slug = platform_slug(&triple).unwrap_or_else(|| {
                    format!("{}-{}", os_name, triple)
                });
                if !self.config.platforms.is_empty()
                    && !self.config.platforms.contains(&slug)
                {
                    continue;
                }

                let dest_dir = staging.join("bin").join(&slug);
                fs::create_dir_all(&dest_dir)?;
                // poly-linker writes the single unified executable under
                // <triple>/bin/. Only that file is packaged — per-target
                // artifacts stay in dist for debugging but are not shipped.
                let unified_bin = triple_dir.join("bin");
                let scan_dir = if unified_bin.is_dir() {
                    unified_bin
                } else {
                    triple_dir.clone()
                };
                for file in Self::executables_in(&scan_dir)? {
                    fs::copy(&file, dest_dir.join(file.file_name().unwrap()))?;
                    packaged += 1;
                }
            }
        }

        if packaged == 0 {
            let _ = fs::remove_dir_all(&staging);
            anyhow::bail!("No executables found under {}", profile_dir.display());
        }

        // Lock file from the project root
        let lock_src = self.project_root.join("poly.lock");
        if lock_src.exists() {
            fs::copy(&lock_src, staging.join("poly.lock"))?;
        }

        // Sources when include_docs = true
        if self.config.include_docs {
            self.copy_sources(&staging)?;
        }

        // Metadata
        let metadata = self.create_metadata()?;
        let metadata_json = serde_json::to_string_pretty(&metadata)?;
        fs::write(staging.join("package.json"), metadata_json)?;

        // Single VFSX archive for all platforms (spec §8.1)
        let archive_path = self.dist_dir.join(format!(
            "{}-{}.vfsx.tar.zst",
            self.config.name, self.config.version
        ));

        let vfsx = Vfsx::new(false);
        vfsx.create(&staging, &archive_path, &self.config.compression)?;

        fs::remove_dir_all(&staging)?;

        info!("Created package: {}", archive_path.display());
        Ok(archive_path)
    }

    /// Recursively find executable files under a directory.
    fn executables_in(dir: &Path) -> Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        for entry in walkdir::WalkDir::new(dir) {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && Self::is_executable(path) {
                out.push(path.to_path_buf());
            }
        }
        Ok(out)
    }

    fn copy_sources(&self, package_dir: &Path) -> Result<()> {
        let src_dir = package_dir.join("src");
        fs::create_dir_all(&src_dir)?;

        for entry in walkdir::WalkDir::new(&self.project_root) {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() || !Self::is_source_file(path) {
                continue;
            }
            // Skip build artifacts and VCS dirs
            let rel = path.strip_prefix(&self.project_root)?;
            let first = rel.iter().next().and_then(|c| c.to_str()).unwrap_or("");
            if matches!(first, "dist" | "target" | ".git" | "node_modules") {
                continue;
            }
            let dest = src_dir.join(rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(path, dest)?;
        }
        Ok(())
    }

    fn is_executable(path: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = fs::metadata(path) {
                return metadata.permissions().mode() & 0o111 != 0;
            }
        }
        // Windows .exe files are always "executable"
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("exe"))
            .unwrap_or(false)
    }

    fn is_source_file(path: &Path) -> bool {
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        matches!(
            ext,
            "rs" | "c" | "cpp" | "h" | "hpp" | "go" | "js" | "ts" | "py" | "java" | "kt"
        )
    }

    fn create_metadata(&self) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "name": self.config.name,
            "version": self.config.version,
            "description": self.config.description,
            "authors": self.config.authors,
            "license": self.config.license,
            "dependencies": self.config.dependencies,
            "created": chrono::Utc::now().to_rfc3339(),
        }))
    }
}

/// Map a Rust target triple to a spec platform slug like `x86_64-linux`.
fn platform_slug(triple: &str) -> Option<String> {
    let arch = triple.split('-').next()?;
    let os = if triple.contains("windows") {
        "windows"
    } else if triple.contains("linux") {
        "linux"
    } else if triple.contains("apple") || triple.contains("darwin") {
        "macos"
    } else {
        return None;
    };
    Some(format!("{}-{}", arch, os))
}

impl Default for Packager {
    fn default() -> Self {
        Self::new(
            PackageConfig {
                name: "package".to_string(),
                version: "0.1.0".to_string(),
                description: None,
                authors: Vec::new(),
                license: "MPL-2.0".to_string(),
                platforms: Vec::new(),
                include_docs: false,
                compression: "zstd".to_string(),
                dependencies: Vec::new(),
            },
            PathBuf::from("."),
            PathBuf::from("dist"),
            "release".to_string(),
        )
    }
}
