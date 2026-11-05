//! Dependency downloader for crates.io packages
//!
//! This module handles downloading and caching dependencies from crates.io.

use anyhow::Result;
use poly_registry::RegistryClient;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Dependency downloader
pub struct DependencyDownloader {
    client: RegistryClient,
    cache_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct DownloadedDependency {
    pub name: String,
    pub version: String,
    pub path: PathBuf,
    pub checksum: String,
}

impl DependencyDownloader {
    /// Create a new dependency downloader
    pub fn new() -> Result<Self> {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from(".cache"))
            .join("poly")
            .join("dependencies");
        
        fs::create_dir_all(&cache_dir)?;
        
        Ok(Self {
            client: RegistryClient::new()?,
            cache_dir,
        })
    }

    /// Download a dependency from crates.io
    pub async fn download(&self, name: &str, version: &str) -> Result<DownloadedDependency> {
        debug!("Downloading dependency: {} {}", name, version);

        // Check if already cached
        let cache_path = self.cache_dir.join(format!("{}-{}", name, version));
        if cache_path.exists() {
            debug!("Using cached dependency: {} {}", name, version);
            return Ok(DownloadedDependency {
                name: name.to_string(),
                version: version.to_string(),
                path: cache_path,
                checksum: "cached".to_string(),
            });
        }

        // Download from crates.io
        let crate_path = self.client.download_crate(name, version).await?;
        
        // Extract the crate
        let extract_path = self.extract_crate(&crate_path, name, version)?;
        
        // Get checksum
        let checksum = self.client.get_checksum(name, version).await?;

        info!("Downloaded dependency: {} {}", name, version);
        
        Ok(DownloadedDependency {
            name: name.to_string(),
            version: version.to_string(),
            path: extract_path,
            checksum,
        })
    }

    /// Extract a .crate file
    fn extract_crate(&self, crate_path: &Path, name: &str, version: &str) -> Result<PathBuf> {
        debug!("Extracting crate: {}", crate_path.display());

        let extract_dir = self.cache_dir.join(format!("{}-{}", name, version));
        
        if extract_dir.exists() {
            fs::remove_dir_all(&extract_dir)?;
        }
        
        fs::create_dir_all(&extract_dir)?;

        // Extract .crate file (it's a tar.gz)
        let file = fs::File::open(crate_path)?;
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        
        archive.unpack(&extract_dir)?;

        // The crate is usually in a subdirectory like name-version/
        let crate_subdir = extract_dir.join(format!("{}-{}", name, version));
        if crate_subdir.exists() {
            // Move contents up one level
            for entry in fs::read_dir(&crate_subdir)? {
                let entry = entry?;
                let dest = extract_dir.join(entry.file_name());
                fs::rename(entry.path(), dest)?;
            }
            fs::remove_dir(&crate_subdir)?;
        }

        Ok(extract_dir)
    }

    /// Download multiple dependencies
    pub async fn download_all(
        &self,
        dependencies: &HashMap<String, String>,
    ) -> Result<HashMap<String, DownloadedDependency>> {
        let mut downloaded = HashMap::new();

        for (name, version_spec) in dependencies {
            // Parse version spec (e.g., "1.0|full" -> version "1.0", features "full")
            let (version_req, _features) = self.parse_version_spec(version_spec);
            
            // Resolve version
            let version = self.client.resolve_version(name, &version_req).await?;
            
            // Download
            let dep = self.download(name, &version).await?;
            downloaded.insert(name.clone(), dep);
        }

        Ok(downloaded)
    }

    fn parse_version_spec(&self, spec: &str) -> (String, Option<String>) {
        if let Some((version, metadata)) = spec.split_once('|') {
            (version.to_string(), Some(metadata.to_string()))
        } else {
            (spec.to_string(), None)
        }
    }

    /// Get the path to a downloaded dependency
    pub fn get_dependency_path(&self, name: &str, version: &str) -> PathBuf {
        self.cache_dir.join(format!("{}-{}", name, version))
    }

    /// Clean the cache
    pub fn clean_cache(&self) -> Result<()> {
        if self.cache_dir.exists() {
            fs::remove_dir_all(&self.cache_dir)?;
        }
        fs::create_dir_all(&self.cache_dir)?;
        Ok(())
    }
}

impl Default for DependencyDownloader {
    fn default() -> Self {
        Self::new().expect("Failed to create DependencyDownloader")
    }
}
