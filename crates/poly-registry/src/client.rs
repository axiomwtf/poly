//! crates.io registry client

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;
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
    pub links: Option<String>,
    pub rust_version: Option<String>,
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
    pub package: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CrateResponse {
    #[serde(rename = "crate")]
    crate_info: CrateMetadata,
    versions: Vec<serde_json::Value>,
    keywords: Vec<String>,
    categories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VersionsResponse {
    versions: Vec<serde_json::Value>,
    meta: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CrateMetadata {
    name: String,
    #[serde(default)]
    max_version: Option<String>,
    #[serde(default)]
    newest_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CrateVersion {
    num: String,
    #[serde(default)]
    yanked: bool,
    #[serde(default)]
    checksum: Option<String>,
    #[serde(default)]
    features: HashMap<String, Vec<String>>,
    #[serde(default)]
    links: Option<serde_json::Value>,
    #[serde(default)]
    rust_version: Option<String>,
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

        fs::create_dir_all(&cache_dir)?;

        Ok(Self {
            client,
            base_url: "https://crates.io/api/v1".to_string(),
            cache_dir,
        })
    }

    /// Get cached response or fetch from API
    async fn get_cached_or_fetch(&self, url: &str, cache_key: &str) -> Result<serde_json::Value> {
        let cache_path = self.cache_dir.join(format!("{}.json", cache_key));
        
        // Check cache first
        if cache_path.exists() {
            if let Ok(content) = fs::read_to_string(&cache_path) {
                if let Ok(json) = serde_json::from_str(&content) {
                    debug!("Using cached response for: {}", cache_key);
                    return Ok(json);
                }
            }
        }

        // Fetch from API with retry
        let mut retries = 0;
        let max_retries = 3;
        
        loop {
            debug!("Fetching from API: {}", url);
            
            // Add delay to avoid rate limiting
            sleep(Duration::from_millis(1000)).await;
            
            let response = self.client.get(url)
                .header("User-Agent", "poly/0.1.0")
                .send()
                .await?;

            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                retries += 1;
                if retries > max_retries {
                    anyhow::bail!("Rate limited after {} retries", max_retries);
                }
                let delay = Duration::from_secs(2u64.pow(retries));
                debug!("Rate limited, retrying in {:?}", delay);
                sleep(delay).await;
                continue;
            }

            if !response.status().is_success() {
                anyhow::bail!("Failed to fetch: {}", response.status());
            }

            let json: serde_json::Value = response.json().await?;
            
            // Cache the response
            if let Ok(content) = serde_json::to_string_pretty(&json) {
                let _ = fs::write(&cache_path, content);
            }

            return Ok(json);
        }
    }

    /// Get crate information from crates.io
    pub async fn get_crate_info(&self, name: &str) -> Result<CrateInfo> {
        debug!("Fetching crate info for: {}", name);

        let url = format!("{}/crates/{}", self.base_url, name);
        let cache_key = format!("crate-{}", name);
        let json = self.get_cached_or_fetch(&url, &cache_key).await?;
        
        let crate_response: CrateResponse = serde_json::from_value(json)
            .context("Failed to parse crate response")?;
        
        // Get the latest version from the versions array
        let latest_version = crate_response.versions.first()
            .and_then(|v| v.get("num").and_then(|n| n.as_str()))
            .unwrap_or("0.0.0")
            .to_string();

        let checksum = crate_response.versions.first()
            .and_then(|v| v.get("checksum").and_then(|c| c.as_str()))
            .unwrap_or("")
            .to_string();

        // Get dependencies for the latest version
        let deps = self.get_crate_dependencies(name, &latest_version).await?;

        Ok(CrateInfo {
            name: crate_response.crate_info.name.clone(),
            vers: latest_version,
            deps,
            cksum: checksum,
            features: HashMap::new(),
            yanked: false,
            links: None,
            rust_version: None,
        })
    }

    /// Get all versions of a crate
    pub async fn get_crate_versions(&self, name: &str) -> Result<Vec<CrateInfo>> {
        debug!("Fetching versions for crate: {}", name);

        let url = format!("{}/crates/{}/versions", self.base_url, name);
        let cache_key = format!("versions-{}", name);
        let json = self.get_cached_or_fetch(&url, &cache_key).await?;
        
        let versions_response: VersionsResponse = serde_json::from_value(json)
            .context("Failed to parse versions response")?;
        
        // Convert to CrateInfo format
        let mut versions = Vec::new();
        for version_value in versions_response.versions {
            if let Some(version_obj) = version_value.as_object() {
                let num = version_obj.get("num")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                
                if num.is_empty() {
                    continue;
                }

                let yanked = version_obj.get("yanked")
                    .and_then(|y| y.as_bool())
                    .unwrap_or(false);

                let checksum = version_obj.get("checksum")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();

                let mut features = HashMap::new();
                if let Some(features_obj) = version_obj.get("features").and_then(|f| f.as_object()) {
                    for (key, value) in features_obj {
                        if let Some(features_array) = value.as_array() {
                            let feature_list: Vec<String> = features_array
                                .iter()
                                .filter_map(|f| f.as_str().map(|s| s.to_string()))
                                .collect();
                            features.insert(key.clone(), feature_list);
                        }
                    }
                }

                let rust_version = version_obj.get("rust_version")
                    .and_then(|r| r.as_str())
                    .map(|s| s.to_string());

                // Don't fetch dependencies here - that's too many API calls
                // Dependencies will be fetched on demand when needed
                let deps = Vec::new();

                versions.push(CrateInfo {
                    name: name.to_string(),
                    vers: num,
                    deps,
                    cksum: checksum,
                    features,
                    yanked,
                    links: None,
                    rust_version,
                });
            }
        }

        Ok(versions)
    }

    /// Resolve a version requirement to a specific version
    pub async fn resolve_version(&self, name: &str, req: &str) -> Result<String> {
        debug!("Resolving version for {}: {}", name, req);

        let versions = self.get_crate_versions(name).await?;

        // Find the highest version that matches the requirement
        let mut best_version: Option<String> = None;

        for version in versions {
            if version.yanked {
                continue;
            }

            if self.version_matches(&version.vers, req) {
                if best_version.is_none() || self.compare_versions(&version.vers, best_version.as_ref().unwrap()) == std::cmp::Ordering::Greater {
                    best_version = Some(version.vers.clone());
                }
            }
        }

        best_version.context(format!("No suitable version found for {}", name))
    }

    /// Check if a version matches a requirement
    /// Handles comma-separated requirements and: =1.0.0 (exact), ^1.0 (caret), ~1.0 (tilde), 1.0 (caret), * (any)
    pub fn version_matches(&self, version: &str, req: &str) -> bool {
        req.split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .all(|part| self.version_matches_single(version, part))
    }

    fn version_matches_single(&self, version: &str, req: &str) -> bool {
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

        // Handle >=, <=, >, <
        if let Some(rest) = req.strip_prefix(">=") {
            return self.compare_versions(version, rest.trim()) != std::cmp::Ordering::Less;
        }
        if let Some(rest) = req.strip_prefix("<=") {
            return self.compare_versions(version, rest.trim()) != std::cmp::Ordering::Greater;
        }
        if let Some(rest) = req.strip_prefix('>') {
            return self.compare_versions(version, rest.trim()) == std::cmp::Ordering::Greater;
        }
        if let Some(rest) = req.strip_prefix('<') {
            return self.compare_versions(version, rest.trim()) == std::cmp::Ordering::Less;
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

    /// Compare two versions for sorting
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

    /// Get crate dependencies for a specific version
    pub async fn get_crate_dependencies(&self, name: &str, version: &str) -> Result<Vec<CrateDependency>> {
        debug!("Fetching dependencies for {} {}", name, version);

        let url = format!("{}/crates/{}/{}/dependencies", self.base_url, name, version);
        let cache_key = format!("deps-{}-{}", name, version);
        let json = self.get_cached_or_fetch(&url, &cache_key).await?;
        
        let mut deps = Vec::new();
        
        if let Some(deps_array) = json.get("dependencies").and_then(|d| d.as_array()) {
            for dep_value in deps_array {
                if let Some(dep_obj) = dep_value.as_object() {
                    // Skip dev dependencies
                    if let Some(kind) = dep_obj.get("kind").and_then(|k| k.as_str()) {
                        if kind == "dev" {
                            continue;
                        }
                    }

                    let dep_name = dep_obj.get("crate_id")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    
                    if dep_name.is_empty() {
                        continue;
                    }

                    let req = dep_obj.get("req")
                        .and_then(|r| r.as_str())
                        .unwrap_or("*")
                        .to_string();

                    let optional = dep_obj.get("optional")
                        .and_then(|o| o.as_bool())
                        .unwrap_or(false);

                    let default_features = dep_obj.get("default_features")
                        .and_then(|d| d.as_bool())
                        .unwrap_or(true);

                    let target = dep_obj.get("target")
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string());

                    let kind = dep_obj.get("kind")
                        .and_then(|k| k.as_str())
                        .unwrap_or("normal")
                        .to_string();

                    // Parse features
                    let mut features = Vec::new();
                    if let Some(features_array) = dep_obj.get("features").and_then(|f| f.as_array()) {
                        for feature in features_array {
                            if let Some(feature_str) = feature.as_str() {
                                features.push(feature_str.to_string());
                            }
                        }
                    }

                    deps.push(CrateDependency {
                        name: dep_name,
                        req,
                        features,
                        optional,
                        default_features,
                        target,
                        kind,
                        package: None,
                    });
                }
            }
        }

        Ok(deps)
    }

    /// Get crate checksum
    pub async fn get_checksum(&self, name: &str, version: &str) -> Result<String> {
        // Get versions to find the checksum for the specific version
        let versions = self.get_crate_versions(name).await?;
        
        for version_info in versions {
            if version_info.vers == version {
                return Ok(version_info.cksum);
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
