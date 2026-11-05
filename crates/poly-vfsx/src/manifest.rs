//! VFSX manifest and package metadata

use serde::{Deserialize, Serialize};

/// VFSX package manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VfsxManifest {
    pub package: PackageInfo,
    pub files: Vec<FileEntry>,
    pub signatures: Vec<Signature>,
    pub created: String,
}

/// Package information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub authors: Vec<String>,
    pub license: String,
    pub repository: Option<String>,
    pub keywords: Vec<String>,
    pub platforms: Vec<String>,
}

/// File entry in the package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub checksum: String,
    pub content_type: Option<String>,
}

/// Signature for verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signature {
    pub algorithm: String,
    pub key_id: String,
    pub signature: String,
    pub signed_at: String,
}

impl VfsxManifest {
    /// Create a new manifest
    pub fn new(package: PackageInfo) -> Self {
        Self {
            package,
            files: Vec::new(),
            signatures: Vec::new(),
            created: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Add a file entry
    pub fn add_file(&mut self, path: String, size: u64, checksum: String) {
        self.files.push(FileEntry {
            path,
            size,
            checksum,
            content_type: None,
        });
    }

    /// Add a signature
    pub fn add_signature(&mut self, signature: Signature) {
        self.signatures.push(signature);
    }
}
