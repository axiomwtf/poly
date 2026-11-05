//! VFSX signer for package signing

use anyhow::{Context, Result};
use ed25519_dalek::{SigningKey, Signer};
use serde_json;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use tracing::debug;

use crate::manifest::{VfsxManifest, Signature};

/// VFSX signer for creating signed packages
pub struct VfsxSigner {
    signing_key: SigningKey,
}

impl VfsxSigner {
    /// Create a new signer from a private key file
    pub fn from_keyfile(path: &Path) -> Result<Self> {
        let key_data = fs::read(path)
            .context(format!("Failed to read keyfile: {}", path.display()))?;

        let signing_key = SigningKey::from_bytes(&key_data.try_into()
            .map_err(|_| anyhow::anyhow!("Invalid keyfile format"))?);

        Ok(Self { signing_key })
    }

    /// Generate a new keypair
    pub fn generate() -> Self {
        use rand_core::RngCore;
        let mut rng = rand::thread_rng();
        let mut secret_key = [0u8; 32];
        rng.fill_bytes(&mut secret_key);
        let signing_key = SigningKey::from_bytes(&secret_key);
        Self { signing_key }
    }

    /// Get the public key for verification
    pub fn public_key(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    /// Sign a manifest
    pub fn sign_manifest(&self, manifest: &VfsxManifest) -> Result<Signature> {
        debug!("Signing VFSX manifest");

        // Serialize manifest to JSON
        let manifest_json = serde_json::to_string(manifest)?;

        // Hash the manifest
        let mut hasher = Sha256::new();
        hasher.update(manifest_json.as_bytes());
        let hash = hasher.finalize();

        // Sign the hash
        let signature = self.signing_key.sign(&hash);

        Ok(Signature {
            algorithm: "ed25519".to_string(),
            key_id: hex::encode(self.public_key()),
            signature: hex::encode(signature.to_bytes()),
            signed_at: chrono::Utc::now().to_rfc3339(),
        })
    }

    /// Sign a package directory
    pub fn sign_package(&self, package_dir: &Path, manifest: &mut VfsxManifest) -> Result<()> {
        debug!("Signing package: {}", package_dir.display());

        // Create signature
        let signature = self.sign_manifest(manifest)?;
        manifest.add_signature(signature);

        // Write signed manifest
        let manifest_path = package_dir.join("vfsx.manifest.json");
        let manifest_json = serde_json::to_string_pretty(manifest)?;
        fs::write(&manifest_path, manifest_json)?;

        debug!("Package signed successfully");
        Ok(())
    }

    /// Create a signed package archive
    pub fn create_package(
        &self,
        source_dir: &Path,
        output_path: &Path,
        manifest: &VfsxManifest,
    ) -> Result<()> {
        debug!("Creating signed package: {}", output_path.display());

        // Create temporary directory for package
        let temp_dir = output_path.with_extension("tmp");
        fs::create_dir_all(&temp_dir)?;

        // Copy files to temp directory
        self.copy_directory_contents(source_dir, &temp_dir)?;

        // Sign the package
        let mut signed_manifest = manifest.clone();
        self.sign_package(&temp_dir, &mut signed_manifest)?;

        // Create archive
        self.create_archive(&temp_dir, output_path)?;

        // Cleanup
        fs::remove_dir_all(&temp_dir)?;

        debug!("Package created successfully");
        Ok(())
    }

    fn copy_directory_contents(&self, src: &Path, dst: &Path) -> Result<()> {
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let path = entry.path();
            let dest = dst.join(entry.file_name());

            if path.is_dir() {
                fs::create_dir_all(&dest)?;
                self.copy_directory_contents(&path, &dest)?;
            } else {
                fs::copy(&path, &dest)?;
            }
        }
        Ok(())
    }

    fn create_archive(&self, source_dir: &Path, output_path: &Path) -> Result<()> {
        use tar::Builder;
        use xz2::write::XzEncoder;

        let file = fs::File::create(output_path)?;
        let encoder = XzEncoder::new(file, 9);
        let mut tar_builder = Builder::new(encoder);

        // Add all files to archive
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

        Ok(())
    }
}

impl Default for VfsxSigner {
    fn default() -> Self {
        Self::generate()
    }
}
