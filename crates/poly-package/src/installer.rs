//! Package installer for installing Poly packages

use anyhow::Result;
use poly_vfsx::VfsxVerifier;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Installation configuration
#[derive(Debug, Clone)]
pub struct InstallConfig {
    pub install_dir: PathBuf,
    pub verify_signatures: bool,
    pub trusted_keys: Vec<[u8; 32]>,
}

/// Package installer
pub struct Installer {
    config: InstallConfig,
}

impl Installer {
    /// Create a new installer
    pub fn new(config: InstallConfig) -> Self {
        Self { config }
    }

    /// Install a package from an archive
    pub async fn install(&self, archive_path: &Path) -> Result<()> {
        info!("Installing package: {}", archive_path.display());

        // Verify package if configured
        if self.config.verify_signatures {
            self.verify_package(archive_path).await?;
        }

        // Extract package
        let temp_dir = self.extract_package(archive_path).await?;

        // Install package
        self.install_package(&temp_dir).await?;

        // Cleanup
        fs::remove_dir_all(&temp_dir)?;

        info!("Package installed successfully");
        Ok(())
    }

    /// Verify package signatures
    async fn verify_package(&self, archive_path: &Path) -> Result<()> {
        debug!("Verifying package signatures");

        let verifier = VfsxVerifier::new(self.config.trusted_keys.clone());
        
        if !verifier.verify_archive(archive_path)? {
            anyhow::bail!("Package signature verification failed");
        }

        debug!("Package signature verified");
        Ok(())
    }

    /// Extract package archive
    async fn extract_package(&self, archive_path: &Path) -> Result<PathBuf> {
        debug!("Extracting package: {}", archive_path.display());

        let temp_dir = std::env::temp_dir()
            .join(format!("poly-install-{}", std::process::id()));
        
        fs::create_dir_all(&temp_dir)?;

        // Extract archive
        use tar::Archive;
        use xz2::read::XzDecoder;

        let file = fs::File::open(archive_path)?;
        let decoder = XzDecoder::new(file);
        let mut archive = Archive::new(decoder);

        archive.unpack(&temp_dir)?;

        Ok(temp_dir)
    }

    /// Install extracted package
    async fn install_package(&self, package_dir: &Path) -> Result<()> {
        debug!("Installing package from: {}", package_dir.display());

        // Find the package directory (should contain bin/, src/, etc.)
        let package_root = self.find_package_root(package_dir)?;

        // Install executables
        self.install_executables(&package_root)?;

        // Install source files if present
        self.install_sources(&package_root)?;

        // Install metadata
        self.install_metadata(&package_root)?;

        Ok(())
    }

    fn find_package_root(&self, extract_dir: &Path) -> Result<PathBuf> {
        // Look for the package directory (should contain vfsx.manifest.json)
        for entry in fs::read_dir(extract_dir)? {
            let entry = entry?;
            let path = entry.path();
            
            if path.is_dir() && path.join("vfsx.manifest.json").exists() {
                return Ok(path);
            }
        }

        // If no subdirectory found, assume extract_dir is the package root
        Ok(extract_dir.to_path_buf())
    }

    fn install_executables(&self, package_root: &Path) -> Result<()> {
        let bin_dir = package_root.join("bin");
        
        if !bin_dir.exists() {
            debug!("No bin directory found");
            return Ok(());
        }

        // Determine current platform
        let platform = self.get_current_platform()?;
        let platform_bin_dir = bin_dir.join(&platform);

        if !platform_bin_dir.exists() {
            debug!("No binaries for platform: {}", platform);
            return Ok(());
        }

        // Install executables to install_dir/bin
        let install_bin_dir = self.config.install_dir.join("bin");
        fs::create_dir_all(&install_bin_dir)?;

        for entry in fs::read_dir(&platform_bin_dir)? {
            let entry = entry?;
            let path = entry.path();
            
            if path.is_file() {
                let file_name = entry.file_name();
                let dest_path = install_bin_dir.join(&file_name);
                
                fs::copy(&path, &dest_path)?;
                
                // Make executable
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = fs::metadata(&dest_path)?.permissions();
                    perms.set_mode(0o755);
                    fs::set_permissions(&dest_path, perms)?;
                }
                
                info!("Installed executable: {}", file_name.to_string_lossy());
            }
        }

        Ok(())
    }

    fn install_sources(&self, package_root: &Path) -> Result<()> {
        let src_dir = package_root.join("src");
        
        if !src_dir.exists() {
            debug!("No src directory found");
            return Ok(());
        }

        let install_src_dir = self.config.install_dir.join("src");
        fs::create_dir_all(&install_src_dir)?;

        // Copy source files
        for entry in walkdir::WalkDir::new(&src_dir) {
            let entry = entry?;
            let path = entry.path();
            
            if path.is_file() {
                let relative_path = path.strip_prefix(&src_dir)?;
                let dest_path = install_src_dir.join(relative_path);
                
                if let Some(parent) = dest_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                
                fs::copy(path, dest_path)?;
            }
        }

        debug!("Installed source files");
        Ok(())
    }

    fn install_metadata(&self, package_root: &Path) -> Result<()> {
        // Install metadata files
        let metadata_dir = self.config.install_dir.join("metadata");
        fs::create_dir_all(&metadata_dir)?;

        // Copy manifest
        let manifest_path = package_root.join("vfsx.manifest.json");
        if manifest_path.exists() {
            fs::copy(&manifest_path, metadata_dir.join("vfsx.manifest.json"))?;
        }

        // Copy lock file
        let lock_path = package_root.join("poly.lock");
        if lock_path.exists() {
            fs::copy(&lock_path, metadata_dir.join("poly.lock"))?;
        }

        debug!("Installed metadata");
        Ok(())
    }

    fn get_current_platform(&self) -> Result<String> {
        // Determine current platform
        let arch = std::env::consts::ARCH;
        let os = std::env::consts::OS;

        let platform = match (arch, os) {
            ("x86_64", "linux") => "x86_64-linux",
            ("aarch64", "linux") => "aarch64-linux",
            ("x86_64", "macos") => "x86_64-macos",
            ("aarch64", "macos") => "aarch64-macos",
            ("x86_64", "windows") => "x86_64-windows",
            ("aarch64", "windows") => "aarch64-windows",
            _ => return Err(anyhow::anyhow!("Unsupported platform: {}-{}", arch, os)),
        };

        Ok(platform.to_string())
    }

    /// Uninstall a package
    pub async fn uninstall(&self, package_name: &str) -> Result<()> {
        info!("Uninstalling package: {}", package_name);

        // Remove executables
        self.remove_executables(package_name)?;

        // Remove source files
        self.remove_sources(package_name)?;

        // Remove metadata
        self.remove_metadata(package_name)?;

        info!("Package uninstalled successfully");
        Ok(())
    }

    fn remove_executables(&self, _package_name: &str) -> Result<()> {
        // TODO: Implement proper package tracking
        // For now, just remove the bin directory
        let bin_dir = self.config.install_dir.join("bin");
        if bin_dir.exists() {
            fs::remove_dir_all(&bin_dir)?;
        }
        Ok(())
    }

    fn remove_sources(&self, _package_name: &str) -> Result<()> {
        let src_dir = self.config.install_dir.join("src");
        if src_dir.exists() {
            fs::remove_dir_all(&src_dir)?;
        }
        Ok(())
    }

    fn remove_metadata(&self, _package_name: &str) -> Result<()> {
        let metadata_dir = self.config.install_dir.join("metadata");
        if metadata_dir.exists() {
            fs::remove_dir_all(&metadata_dir)?;
        }
        Ok(())
    }
}

impl Default for Installer {
    fn default() -> Self {
        Self::new(InstallConfig {
            install_dir: PathBuf::from(".poly"),
            verify_signatures: true,
            trusted_keys: Vec::new(),
        })
    }
}
