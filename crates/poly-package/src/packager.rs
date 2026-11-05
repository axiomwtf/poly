//! Package packager for creating distributable archives

use anyhow::{Context, Result};
use poly_vfsx::{VfsxManifest, VfsxSigner, PackageInfo};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Package configuration
#[derive(Debug, Clone)]
pub struct PackageConfig {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub authors: Vec<String>,
    pub license: String,
    pub platforms: Vec<String>,
    pub include_docs: bool,
    pub sign: bool,
    pub keyfile: Option<PathBuf>,
}

/// Package packager
pub struct Packager {
    config: PackageConfig,
    build_dir: PathBuf,
    output_dir: PathBuf,
}

impl Packager {
    /// Create a new packager
    pub fn new(config: PackageConfig, build_dir: PathBuf, output_dir: PathBuf) -> Self {
        Self {
            config,
            build_dir,
            output_dir,
        }
    }

    /// Create a package for all platforms
    pub async fn create_package(&self) -> Result<Vec<PathBuf>> {
        info!("Creating package for {}", self.config.name);

        let mut packages = Vec::new();

        for platform in &self.config.platforms {
            let package_path = self.create_platform_package(platform).await?;
            packages.push(package_path);
        }

        info!("Created {} packages", packages.len());
        Ok(packages)
    }

    /// Create a package for a specific platform
    async fn create_platform_package(&self, platform: &str) -> Result<PathBuf> {
        debug!("Creating package for platform: {}", platform);

        // Create temporary package directory
        let package_dir = self.output_dir.join(format!("{}-{}-{}", 
            self.config.name, self.config.version, platform));
        fs::create_dir_all(&package_dir)?;

        // Create bin directory
        let bin_dir = package_dir.join("bin").join(platform);
        fs::create_dir_all(&bin_dir)?;

        // Copy executables for this platform
        self.copy_executables(platform, &bin_dir)?;

        // Copy source files if needed
        if self.config.include_docs {
            self.copy_sources(&package_dir)?;
        }

        // Copy lock file
        let lock_file = self.build_dir.join("poly.lock");
        if lock_file.exists() {
            fs::copy(&lock_file, package_dir.join("poly.lock"))?;
        }

        // Create manifest
        let manifest = self.create_manifest(platform)?;
        let manifest_path = package_dir.join("vfsx.manifest.json");
        let manifest_json = serde_json::to_string_pretty(&manifest)?;
        fs::write(&manifest_path, manifest_json)?;

        // Sign package if configured
        if self.config.sign {
            self.sign_package(&package_dir, &manifest).await?;
        }

        // Create archive
        let archive_path = self.output_dir.join(format!(
            "{}-{}.vfsx.tar.zst",
            self.config.name, self.config.version
        ));

        self.create_archive(&package_dir, &archive_path)?;

        // Cleanup
        fs::remove_dir_all(&package_dir)?;

        debug!("Created package: {}", archive_path.display());
        Ok(archive_path)
    }

    fn copy_executables(&self, platform: &str, bin_dir: &Path) -> Result<()> {
        // Look for executables in build directory
        let platform_build_dir = self.build_dir.join(platform);
        
        if platform_build_dir.exists() {
            for entry in fs::read_dir(&platform_build_dir)? {
                let entry = entry?;
                let path = entry.path();
                
                if path.is_file() && self.is_executable(&path) {
                    let file_name = entry.file_name();
                    fs::copy(&path, bin_dir.join(&file_name))?;
                    debug!("Copied executable: {}", file_name.to_string_lossy());
                }
            }
        }

        Ok(())
    }

    fn copy_sources(&self, package_dir: &Path) -> Result<()> {
        // Copy source files
        let src_dir = package_dir.join("src");
        fs::create_dir_all(&src_dir)?;

        // Copy all source files from project
        let project_root = self.build_dir.parent()
            .unwrap_or(&self.build_dir);

        for entry in walkdir::WalkDir::new(project_root) {
            let entry = entry?;
            let path = entry.path();
            
            if path.is_file() && self.is_source_file(path) {
                let relative_path = path.strip_prefix(project_root)?;
                let dest_path = src_dir.join(relative_path);
                
                if let Some(parent) = dest_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                
                fs::copy(path, dest_path)?;
            }
        }

        Ok(())
    }

    fn is_executable(&self, path: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = fs::metadata(path) {
                let permissions = metadata.permissions();
                return permissions.mode() & 0o111 != 0;
            }
        }
        false
    }

    fn is_source_file(&self, path: &Path) -> bool {
        let ext = path.extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        
        matches!(ext, "rs" | "c" | "cpp" | "h" | "hpp" | "go" | "js" | "ts" | "py" | "java" | "kt")
    }

    fn create_manifest(&self, platform: &str) -> Result<VfsxManifest> {
        let package_info = PackageInfo {
            name: self.config.name.clone(),
            version: self.config.version.clone(),
            description: self.config.description.clone(),
            authors: self.config.authors.clone(),
            license: self.config.license.clone(),
            repository: None, // TODO: Get from config
            keywords: Vec::new(), // TODO: Get from config
            platforms: vec![platform.to_string()],
        };

        let mut manifest = VfsxManifest::new(package_info);

        // Add file entries
        let package_dir = self.output_dir.join(format!("{}-{}-{}", 
            self.config.name, self.config.version, platform));
        
        for entry in walkdir::WalkDir::new(&package_dir) {
            let entry = entry?;
            let path = entry.path();
            
            if path.is_file() {
                let relative_path = path.strip_prefix(&package_dir)?;
                let metadata = fs::metadata(path)?;
                let checksum = self.calculate_checksum(path)?;
                
                manifest.add_file(
                    relative_path.to_string_lossy().to_string(),
                    metadata.len(),
                    checksum,
                );
            }
        }

        Ok(manifest)
    }

    fn calculate_checksum(&self, path: &Path) -> Result<String> {
        use sha2::{Digest, Sha256};
        
        let content = fs::read(path)?;
        let mut hasher = Sha256::new();
        hasher.update(&content);
        Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
    }

    async fn sign_package(&self, package_dir: &Path, manifest: &VfsxManifest) -> Result<()> {
        let keyfile = self.config.keyfile.as_ref()
            .context("No keyfile specified for signing")?;
        
        let signer = VfsxSigner::from_keyfile(keyfile)?;
        let mut signed_manifest = manifest.clone();
        
        signer.sign_package(package_dir, &mut signed_manifest)?;
        
        Ok(())
    }

    fn create_archive(&self, source_dir: &Path, output_path: &Path) -> Result<()> {
        use tar::Builder;
        use xz2::write::XzEncoder;

        let file = fs::File::create(output_path)?;
        let encoder = XzEncoder::new(file, 9);
        let mut tar_builder = Builder::new(encoder);

        for entry in walkdir::WalkDir::new(source_dir) {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() {
                let relative_path = path.strip_prefix(source_dir)?;
                let mut file = fs::File::open(path)?;
                tar_builder.append_file(relative_path, &mut file)?;
            }
        }

        let encoder = tar_builder.into_inner()?;
        encoder.finish()?;

        debug!("Created archive: {}", output_path.display());
        Ok(())
    }
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
                sign: false,
                keyfile: None,
            },
            PathBuf::from("target"),
            PathBuf::from("dist"),
        )
    }
}
