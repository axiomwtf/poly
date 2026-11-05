//! VFSX verifier for package verification

use anyhow::{Context, Result};
use ed25519_dalek::{Signature as EdSignature, Verifier, VerifyingKey};
use serde_json;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use tracing::debug;

use crate::manifest::{VfsxManifest, Signature};

/// VFSX verifier for checking package signatures
pub struct VfsxVerifier {
    trusted_keys: Vec<[u8; 32]>,
}

impl VfsxVerifier {
    /// Create a new verifier with trusted keys
    pub fn new(trusted_keys: Vec<[u8; 32]>) -> Self {
        Self { trusted_keys }
    }

    /// Create a verifier that trusts all keys
    pub fn trust_all() -> Self {
        Self {
            trusted_keys: Vec::new(),
        }
    }

    /// Verify a manifest signature
    pub fn verify_manifest(&self, manifest: &VfsxManifest) -> Result<bool> {
        debug!("Verifying VFSX manifest");

        if manifest.signatures.is_empty() {
            debug!("No signatures found in manifest");
            return Ok(false);
        }

        // Serialize manifest without signatures for verification
        let mut manifest_clone = manifest.clone();
        manifest_clone.signatures.clear();
        let manifest_json = serde_json::to_string(&manifest_clone)?;

        // Hash the manifest
        let mut hasher = Sha256::new();
        hasher.update(manifest_json.as_bytes());
        let hash = hasher.finalize();

        // Verify each signature
        for signature in &manifest.signatures {
            if self.verify_signature(&hash, signature)? {
                debug!("Signature verified successfully");
                return Ok(true);
            }
        }

        debug!("No valid signatures found");
        Ok(false)
    }

    /// Verify a single signature
    fn verify_signature(&self, hash: &[u8], signature: &Signature) -> Result<bool> {
        // Parse public key
        let public_key_bytes = hex::decode(&signature.key_id)
            .context("Invalid public key format")?;

        let public_key_array: [u8; 32] = public_key_bytes.try_into()
            .map_err(|_| anyhow::anyhow!("Invalid public key length"))?;

        // Check if key is trusted
        if !self.trusted_keys.is_empty() && !self.trusted_keys.contains(&public_key_array) {
            debug!("Public key not in trusted keys list");
            return Ok(false);
        }

        // Parse signature
        let signature_bytes = hex::decode(&signature.signature)
            .context("Invalid signature format")?;

        let signature_array: [u8; 64] = signature_bytes.try_into()
            .map_err(|_| anyhow::anyhow!("Invalid signature length"))?;

        // Verify signature
        let verifying_key = VerifyingKey::from_bytes(&public_key_array)
            .context("Invalid public key")?;

        let ed_signature = EdSignature::from_bytes(&signature_array);

        match verifying_key.verify(hash, &ed_signature) {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Verify a package directory
    pub fn verify_package(&self, package_dir: &Path) -> Result<bool> {
        debug!("Verifying package: {}", package_dir.display());

        let manifest_path = package_dir.join("vfsx.manifest.json");
        if !manifest_path.exists() {
            debug!("No manifest found in package");
            return Ok(false);
        }

        let manifest_content = fs::read_to_string(&manifest_path)?;
        let manifest: VfsxManifest = serde_json::from_str(&manifest_content)?;

        self.verify_manifest(&manifest)
    }

    /// Verify a package archive
    pub fn verify_archive(&self, archive_path: &Path) -> Result<bool> {
        debug!("Verifying archive: {}", archive_path.display());

        // Extract archive to temporary directory
        let temp_dir = archive_path.with_extension("verify_tmp");
        fs::create_dir_all(&temp_dir)?;

        // Extract archive
        self.extract_archive(archive_path, &temp_dir)?;

        // Verify package
        let result = self.verify_package(&temp_dir);

        // Cleanup
        fs::remove_dir_all(&temp_dir)?;

        result
    }

    fn extract_archive(&self, archive_path: &Path, dest_dir: &Path) -> Result<()> {
        use tar::Archive;
        use xz2::read::XzDecoder;

        let file = fs::File::open(archive_path)?;
        let decoder = XzDecoder::new(file);
        let mut archive = Archive::new(decoder);

        archive.unpack(dest_dir)?;

        Ok(())
    }

    /// Add a trusted key
    pub fn add_trusted_key(&mut self, public_key: [u8; 32]) {
        self.trusted_keys.push(public_key);
    }

    /// Remove a trusted key
    pub fn remove_trusted_key(&mut self, public_key: [u8; 32]) {
        self.trusted_keys.retain(|&key| key != public_key);
    }

    /// List trusted keys
    pub fn trusted_keys(&self) -> &[[u8; 32]] {
        &self.trusted_keys
    }
}

impl Default for VfsxVerifier {
    fn default() -> Self {
        Self::trust_all()
    }
}
