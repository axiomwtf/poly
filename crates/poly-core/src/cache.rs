//! Build cache (local and remote)

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::fs;
use tracing::debug;

/// Cache entry metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// SHA256 hash of inputs
    pub hash: String,
    /// Output files
    pub outputs: Vec<PathBuf>,
    /// Timestamp
    pub timestamp: i64,
}

/// Build cache
pub struct Cache {
    /// Local cache directory
    local_dir: PathBuf,
    /// Remote cache URL (optional)
    remote_url: Option<String>,
}

impl Cache {
    /// Create a new cache with the given local directory
    pub fn new(local_dir: PathBuf) -> Self {
        Self {
            local_dir,
            remote_url: None,
        }
    }

    /// Set remote cache URL
    pub fn with_remote(mut self, url: String) -> Self {
        self.remote_url = Some(url);
        self
    }

    /// Initialize the cache directory
    pub fn init(&self) -> Result<()> {
        fs::create_dir_all(&self.local_dir)?;
        Ok(())
    }

    /// Compute hash of input files
    pub fn hash_inputs(&self, inputs: &[PathBuf]) -> Result<String> {
        let mut hasher = Sha256::new();

        for input in inputs {
            if input.exists() {
                let contents = fs::read(input)?;
                hasher.update(&contents);
            }
        }

        Ok(format!("{:x}", hasher.finalize()))
    }

    /// Check if a cache entry exists for the given inputs
    pub fn get(&self, hash: &str) -> Option<CacheEntry> {
        let entry_path = self.local_dir.join(format!("{}.json", hash));

        if !entry_path.exists() {
            return None;
        }

        match fs::read_to_string(&entry_path) {
            Ok(contents) => serde_json::from_str(&contents).ok(),
            Err(_) => None,
        }
    }

    /// Store a cache entry
    pub fn put(&self, hash: &str, entry: CacheEntry) -> Result<()> {
        let entry_path = self.local_dir.join(format!("{}.json", hash));
        let contents = serde_json::to_string_pretty(&entry)?;
        fs::write(entry_path, contents)?;
        debug!("Cached entry with hash: {}", hash);
        Ok(())
    }

    /// Clear the cache
    pub fn clear(&self) -> Result<()> {
        if self.local_dir.exists() {
            fs::remove_dir_all(&self.local_dir)?;
        }
        self.init()?;
        Ok(())
    }

    /// Get cache statistics
    pub fn stats(&self) -> Result<CacheStats> {
        let mut entries = 0;
        let mut total_size = 0;

        if self.local_dir.exists() {
            for entry in fs::read_dir(&self.local_dir)? {
                let entry = entry?;
                entries += 1;
                total_size += entry.metadata()?.len();
            }
        }

        Ok(CacheStats {
            entries,
            total_size,
            local_dir: self.local_dir.clone(),
        })
    }
}

/// Cache statistics
#[derive(Debug)]
pub struct CacheStats {
    pub entries: u64,
    pub total_size: u64,
    pub local_dir: PathBuf,
}
