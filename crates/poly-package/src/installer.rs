//! Package installer for installing Poly packages
//!
//! Layout under `install_dir`:
//!   bin/                  <- executables for the current platform
//!   packages/<name>/      <- extracted package contents
//!   manifest.json         <- package name -> installed files

use anyhow::{Context, Result};
use poly_vfsx::Vfsx;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Installation configuration
#[derive(Debug, Clone)]
pub struct InstallConfig {
    pub install_dir: PathBuf,
    /// Where executables are placed. Defaults to `<install_dir>/bin`.
    pub bin_dir: Option<PathBuf>,
    pub verify_signatures: bool,
    pub public_key_path: Option<PathBuf>,
}

/// Per-package install record persisted in manifest.json
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PackageRecord {
    version: String,
    files: Vec<PathBuf>,
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

        if self.config.verify_signatures {
            if let Some(key_path) = &self.config.public_key_path {
                if key_path.exists() {
                    self.verify_package(archive_path, key_path).await?;
                } else {
                    debug!(
                        "No public key at {}; skipping signature verification",
                        key_path.display()
                    );
                }
            }
        }

        let temp_dir = self.extract_package(archive_path).await?;
        let result = self.install_package(&temp_dir).await;
        let _ = fs::remove_dir_all(&temp_dir);
        result?;

        info!("Package installed successfully");
        Ok(())
    }

    /// Verify package signatures using external vfsx command
    async fn verify_package(&self, archive_path: &Path, key_path: &Path) -> Result<()> {
        debug!("Verifying package signatures");

        let vfsx = Vfsx::new(false);
        if !vfsx.verify(archive_path, key_path)? {
            anyhow::bail!("Package signature verification failed");
        }

        debug!("Package signature verified");
        Ok(())
    }

    /// Extract package archive using external vfsx command
    async fn extract_package(&self, archive_path: &Path) -> Result<PathBuf> {
        debug!("Extracting package: {}", archive_path.display());

        let temp_dir = std::env::temp_dir()
            .join(format!("poly-install-{}", std::process::id()));

        fs::create_dir_all(&temp_dir)?;

        let vfsx = Vfsx::new(false);
        vfsx.extract(archive_path, &temp_dir)?;

        Ok(temp_dir)
    }

    /// Install extracted package
    async fn install_package(&self, package_dir: &Path) -> Result<()> {
        debug!("Installing package from: {}", package_dir.display());

        let package_root = self.find_package_root(package_dir)?;
        let metadata = self.read_metadata(&package_root)?;
        let name = metadata
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let version = metadata
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("0.0.0")
            .to_string();

        // Keep the extracted package under packages/<name>/
        let pkg_dir = self.config.install_dir.join("packages").join(&name);
        if pkg_dir.exists() {
            fs::remove_dir_all(&pkg_dir)?;
        }
        copy_dir_all(&package_root, &pkg_dir)?;

        // Install current-platform executables into bin/
        let mut installed_files = Vec::new();
        let platform = self.get_current_platform()?;
        let platform_bin = package_root.join("bin").join(&platform);
        if platform_bin.is_dir() {
            let install_bin = self
                .config
                .bin_dir
                .clone()
                .unwrap_or_else(|| self.config.install_dir.join("bin"));
            fs::create_dir_all(&install_bin)?;
            for entry in fs::read_dir(&platform_bin)? {
                let entry = entry?;
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let file_name = entry.file_name();
                let dest = install_bin.join(&file_name);
                fs::copy(&path, &dest)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = fs::metadata(&dest)?.permissions();
                    perms.set_mode(0o755);
                    fs::set_permissions(&dest, perms)?;
                }
                installed_files.push(dest.clone());
                info!("Installed {}", dest.display());
            }
        } else {
            info!("No binaries for platform {} in package", platform);
        }

        // Record in manifest
        let mut manifest = self.load_manifest()?;
        manifest.insert(
            name.clone(),
            PackageRecord {
                version,
                files: installed_files,
            },
        );
        self.save_manifest(&manifest)?;

        Ok(())
    }

    fn read_metadata(&self, package_root: &Path) -> Result<serde_json::Value> {
        let path = package_root.join("package.json");
        if !path.exists() {
            return Ok(serde_json::json!({}));
        }
        let contents = fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&contents)?)
    }

    fn manifest_path(&self) -> PathBuf {
        self.config.install_dir.join("manifest.json")
    }

    fn load_manifest(&self) -> Result<HashMap<String, PackageRecord>> {
        let path = self.manifest_path();
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let contents = fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&contents)?)
    }

    fn save_manifest(&self, manifest: &HashMap<String, PackageRecord>) -> Result<()> {
        fs::create_dir_all(&self.config.install_dir)?;
        let json = serde_json::to_string_pretty(manifest)?;
        fs::write(self.manifest_path(), json)?;
        Ok(())
    }

    fn find_package_root(&self, extract_dir: &Path) -> Result<PathBuf> {
        for entry in fs::read_dir(extract_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() && path.join("package.json").exists() {
                return Ok(path);
            }
        }
        // Also accept a root-level package.json
        if extract_dir.join("package.json").exists() {
            return Ok(extract_dir.to_path_buf());
        }
        Ok(extract_dir.to_path_buf())
    }

    fn get_current_platform(&self) -> Result<String> {
        let arch = std::env::consts::ARCH;
        let os = std::env::consts::OS;

        let platform = match (arch, os) {
            ("x86_64", "linux") => "x86_64-linux",
            ("aarch64", "linux") => "aarch64-linux",
            ("x86_64", "macos") => "x86_64-macos",
            ("aarch64", "macos") => "aarch64-macos",
            ("x86_64", "windows") => "x86_64-windows",
            ("aarch64", "windows") => "aarch64-windows",
            _ => anyhow::bail!("Unsupported platform: {}-{}", arch, os),
        };

        Ok(platform.to_string())
    }

    /// Uninstall a package
    pub async fn uninstall(&self, package_name: &str) -> Result<()> {
        info!("Uninstalling package: {}", package_name);

        let mut manifest = self.load_manifest()?;
        let record = manifest
            .remove(package_name)
            .with_context(|| format!("Package not installed: {}", package_name))?;

        // Remove recorded files (absolute paths used as-is)
        for path in &record.files {
            if path.exists() {
                fs::remove_file(path)?;
            }
        }

        // Remove extracted package dir
        let pkg_dir = self.config.install_dir.join("packages").join(package_name);
        if pkg_dir.exists() {
            fs::remove_dir_all(&pkg_dir)?;
        }

        self.save_manifest(&manifest)?;
        info!("Package uninstalled successfully");
        Ok(())
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_all(&path, &dest)?;
        } else {
            fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

impl Default for Installer {
    fn default() -> Self {
        Self::new(InstallConfig {
            install_dir: PathBuf::from(".poly"),
            bin_dir: None,
            verify_signatures: true,
            public_key_path: None,
        })
    }
}
