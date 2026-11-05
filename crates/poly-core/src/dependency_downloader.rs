//! Dependency downloader for crates.io packages
//!
//! This module handles downloading and caching dependencies from crates.io.

use anyhow::Result;
use poly_registry::RegistryClient;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

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

        // For now, use a placeholder checksum to avoid extra API calls
        // In a full implementation, we'd verify the checksum
        let checksum = "placeholder".to_string();

        debug!("Downloaded dependency: {} {}", name, version);
        
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
            let version_req_str = version_req.as_str();
            
            debug!("Resolving dependency: {} {}", name, version_req_str);

            // Try to find a matching version in the cache first
            let cached_version = self.find_cached_version(name, version_req_str);
            
            let mut downloaded_dep = None;
            
            if let Some(version) = cached_version {
                debug!("Using cached version for {}: {}", name, version);
                match self.download(name, &version).await {
                    Ok(dep) => {
                        downloaded_dep = Some(dep);
                    }
                    Err(e) => {
                        debug!("Failed to use cached version for {} {}: {}", name, version, e);
                    }
                }
            }
            
            // If no cached version or download failed, try to resolve through API
            if downloaded_dep.is_none() {
                // Resolve version through API
                match self.client.resolve_version(name.as_str(), version_req_str).await {
                    Ok(version) => {
                        debug!("Resolved {} to version {}", name, version);
                        match self.download(name, &version).await {
                            Ok(dep) => {
                                downloaded_dep = Some(dep);
                            }
                            Err(e) => {
                                debug!("Download with resolved version failed for {} {}: {}", name, version, e);
                            }
                        }
                    }
                    Err(e) => {
                        debug!("Failed to resolve version for {} {}: {}", name, version_req_str, e);
                        
                        // Try to get latest version as fallback
                        if let Ok(info) = self.client.get_crate_info(name).await {
                            debug!("Trying latest version for {}: {}", name, info.vers);
                            if let Ok(dep) = self.download(name, &info.vers).await {
                                downloaded_dep = Some(dep);
                            }
                        }
                    }
                }
            }
            
            if let Some(dep) = downloaded_dep {
                debug!("Downloaded dependency: {} {}", name, dep.version);
                downloaded.insert(name.clone(), dep);
            } else {
                warn!("Failed to download dependency: {} {}", name, version_req_str);
            }
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

    /// Find a cached version that matches the version requirement
    fn find_cached_version(&self, name: &str, version_req: &str) -> Option<String> {
        let prefix = format!("{}-", name);
        let mut versions = Vec::new();

        if let Ok(entries) = fs::read_dir(&self.cache_dir) {
            for entry in entries.flatten() {
                if let Some(file_name) = entry.file_name().to_str() {
                    if file_name.starts_with(&prefix) {
                        let version = file_name[prefix.len()..].to_string();
                        versions.push(version);
                    }
                }
            }
        }

        // Find the best matching version using semver
        versions
            .into_iter()
            .filter(|v| self.version_matches(v, version_req))
            .max_by(|a, b| self.compare_versions(a, b))
    }

    /// Check if a version matches a requirement
    /// Handles: =1.0.0 (exact), ^1.0 (caret), ~1.0 (tilde), 1.0 (caret), * (any)
    fn version_matches(&self, version: &str, req: &str) -> bool {
        let version_parts: Vec<u64> = version
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect();

        if version_parts.is_empty() {
            return false;
        }

        // Handle wildcard
        if req == "*" || req.is_empty() {
            return true;
        }

        // Handle exact version (=1.0.0)
        if let Some(exact) = req.strip_prefix('=') {
            return version == exact;
        }

        // Handle caret requirement (^1.0)
        if let Some(caret) = req.strip_prefix('^') {
            return self.caret_matches(version, caret);
        }

        // Handle tilde requirement (~1.0)
        if let Some(tilde) = req.strip_prefix('~') {
            return self.tilde_matches(version, tilde);
        }

        // Default to caret requirement (1.0 means ^1.0)
        self.caret_matches(version, req)
    }
    
    /// Check caret requirement (^1.0 means >=1.0.0, <2.0.0)
    fn caret_matches(&self, version: &str, req: &str) -> bool {
        let version_parts: Vec<u64> = version
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect();

        let req_parts: Vec<u64> = req
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect();

        if req_parts.is_empty() || version_parts.is_empty() {
            return false;
        }

        // For 0.x versions, minor is the "major" version
        if req_parts[0] == 0 {
            if version_parts[0] != 0 {
                return false;
            }
            if req_parts.len() > 1 {
                if version_parts.get(1).copied().unwrap_or(0) != req_parts[1] {
                    return false;
                }
            }
            if req_parts.len() > 2 {
                if version_parts.get(2).copied().unwrap_or(0) < req_parts[2] {
                    return false;
                }
            }
        } else {
            if version_parts[0] != req_parts[0] {
                return false;
            }
            if req_parts.len() > 1 {
                if version_parts.get(1).copied().unwrap_or(0) < req_parts[1] {
                    return false;
                }
            }
            if req_parts.len() > 2 {
                let ver_minor = version_parts.get(1).copied().unwrap_or(0);
                let req_minor = req_parts.get(1).copied().unwrap_or(0);
                if ver_minor == req_minor && version_parts.get(2).copied().unwrap_or(0) < req_parts[2] {
                    return false;
                }
            }
        }

        true
    }
    
    /// Check tilde requirement (~1.0 means >=1.0.0, <1.1.0)
    fn tilde_matches(&self, version: &str, req: &str) -> bool {
        let version_parts: Vec<u64> = version
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect();

        let req_parts: Vec<u64> = req
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect();

        if req_parts.is_empty() || version_parts.is_empty() {
            return false;
        }

        // Major must match
        if version_parts[0] != req_parts[0] {
            return false;
        }

        // Minor must match if specified
        if req_parts.len() > 1 {
            if version_parts.get(1).copied().unwrap_or(0) != req_parts[1] {
                return false;
            }
        }

        // Patch must be >= req patch if specified
        if req_parts.len() > 2 {
            if version_parts.get(2).copied().unwrap_or(0) < req_parts[2] {
                return false;
            }
        }

        true
    }

    /// Compare two versions for sorting (returns Ordering)
    fn compare_versions(&self, a: &str, b: &str) -> std::cmp::Ordering {
        let a_parts: Vec<u64> = a
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect();
        let b_parts: Vec<u64> = b
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect();

        for i in 0..3 {
            let a_val = a_parts.get(i).copied().unwrap_or(0);
            let b_val = b_parts.get(i).copied().unwrap_or(0);
            match a_val.cmp(&b_val) {
                std::cmp::Ordering::Equal => continue,
                ord => return ord,
            }
        }
        std::cmp::Ordering::Equal
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
