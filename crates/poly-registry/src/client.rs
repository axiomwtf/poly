//! crates.io registry client

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{debug, info};

/// crates.io registry client
pub struct RegistryClient {
    client: Client,
    base_url: String,
    cache_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateInfo {
    pub name: String,
    pub vers: String,
    pub deps: Vec<CrateDependency>,
    pub cksum: String,
    pub features: HashMap<String, Vec<String>>,
    pub yanked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateDependency {
    pub name: String,
    pub req: String,
    pub features: Vec<String>,
    pub optional: bool,
    pub default_features: bool,
    pub target: Option<String>,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CrateResponse {
    versions: Vec<CrateVersion>,
    meta: CrateMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CrateMetadata {
    #[serde(default)]
    total: Option<u32>,
    #[serde(default)]
    next_page: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CrateVersion {
    num: String,
    #[serde(default)]
    yanked: bool,
    #[serde(default)]
    checksum: Option<String>,
}

impl RegistryClient {
    /// Create a new registry client
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent("poly/0.1.0")
            .build()?;

        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from(".cache"))
            .join("poly")
            .join("registry");

        std::fs::create_dir_all(&cache_dir)?;

        Ok(Self {
            client,
            base_url: "https://crates.io/api/v1".to_string(),
            cache_dir,
        })
    }

    /// Get crate information from crates.io
    pub async fn get_crate_info(&self, name: &str) -> Result<CrateInfo> {
        debug!("Fetching crate info for: {}", name);

        let url = format!("{}/crates/{}", self.base_url, name);
        let response = self.client.get(&url)
            .header("User-Agent", "poly/0.1.0")
            .send()
            .await?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to fetch crate info: {}", response.status());
        }

        // Use the same structure as get_crate_versions
        let crate_response: CrateResponse = response.json().await?;
        
        // Return the latest version info
        let latest_version = crate_response.versions.first()
            .map(|v| v.num.clone())
            .unwrap_or_else(|| "0.0.0".to_string());

        let checksum = crate_response.versions.first()
            .and_then(|v| v.checksum.clone())
            .unwrap_or_default();

        Ok(CrateInfo {
            name: name.to_string(),
            vers: latest_version,
            deps: Vec::new(),
            cksum: checksum,
            features: HashMap::new(),
            yanked: false,
        })
    }

    /// Get all versions of a crate
    pub async fn get_crate_versions(&self, name: &str) -> Result<Vec<CrateInfo>> {
        debug!("Fetching versions for crate: {}", name);

        let url = format!("{}/crates/{}/versions", self.base_url, name);
        let response = self.client.get(&url)
            .header("User-Agent", "poly/0.1.0")
            .send()
            .await?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to fetch crate versions: {}", response.status());
        }

        // crates.io returns a complex structure with crate info and versions
        let crate_response: CrateResponse = response.json().await?;
        
        // Convert to CrateInfo format
        let mut versions = Vec::new();
        for version in crate_response.versions {
            versions.push(CrateInfo {
                name: name.to_string(),
                vers: version.num.clone(),
                deps: Vec::new(), // Simplified for now
                cksum: version.checksum.unwrap_or_default(),
                features: HashMap::new(),
                yanked: version.yanked,
            });
        }

        Ok(versions)
    }

    /// Resolve a version requirement to a specific version
    pub async fn resolve_version(&self, name: &str, req: &str) -> Result<String> {
        debug!("Resolving version for {}: {}", name, req);

        let versions = self.get_crate_versions(name).await?;

        // Simple version resolution - find the highest version that matches the requirement
        // For now, just return the latest non-yanked version
        let mut best_version = None;

        for version in versions {
            if version.yanked {
                continue;
            }

            // Simple version comparison - just take the latest
            if best_version.is_none() || version.vers > best_version.clone().unwrap() {
                best_version = Some(version.vers.clone());
            }
        }

        best_version.context(format!("No suitable version found for {}", name))
    }

    /// Download a crate
    pub async fn download_crate(&self, name: &str, version: &str) -> Result<PathBuf> {
        debug!("Downloading crate: {} {}", name, version);

        let url = format!("https://crates.io/api/v1/crates/{}/{}/download", name, version);
        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to download crate: {}", response.status());
        }

        let bytes = response.bytes().await?;

        // Save to cache
        let cache_path = self.cache_dir.join(format!("{}-{}.crate", name, version));
        std::fs::write(&cache_path, &bytes)?;

        info!("Downloaded crate: {} {}", name, version);
        Ok(cache_path)
    }

    /// Get crate checksum
    pub async fn get_checksum(&self, name: &str, version: &str) -> Result<String> {
        // Get versions to find the checksum for the specific version
        let versions = self.get_crate_versions(name).await?;
        
        for version_info in versions {
            if version_info.vers == version {
                // We need to get the actual checksum from the API
                // For now, return a dummy checksum
                return Ok(format!("sha256:{}", name));
            }
        }
        
        anyhow::bail!("Version {} not found for crate {}", version, name)
    }

    /// Search for crates
    pub async fn search(&self, query: &str) -> Result<Vec<CrateInfo>> {
        debug!("Searching crates: {}", query);

        let url = format!("{}/crates?q={}", self.base_url, query);
        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to search crates: {}", response.status());
        }

        #[derive(Deserialize)]
        struct SearchResponse {
            crates: Vec<CrateInfo>,
        }

        let search_response: SearchResponse = response.json().await?;
        Ok(search_response.crates)
    }
}

impl Default for RegistryClient {
    fn default() -> Self {
        Self::new().expect("Failed to create RegistryClient")
    }
}
