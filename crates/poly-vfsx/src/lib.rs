//! Poly VFSX: wrapper for external vfsx command
//!
//! This crate provides a wrapper around the external vfsx command
//! for creating, verifying, and managing VFSX archives.

use anyhow::Result;
use std::path::Path;
use std::process::Command;
use tracing::{debug, info};

/// VFSX wrapper for external command
pub struct Vfsx {
    verbose: bool,
}

impl Vfsx {
    /// Create a new VFSX wrapper
    pub fn new(verbose: bool) -> Self {
        Self { verbose }
    }

    /// Create a VFSX archive from a directory
    pub fn create(&self, source_dir: &Path, output_path: &Path, compression: &str) -> Result<()> {
        debug!("Creating VFSX archive: {} -> {}", source_dir.display(), output_path.display());

        let mut cmd = Command::new("vfsx");
        cmd.arg("create")
            .arg(source_dir)
            .arg("-o")
            .arg(output_path)
            .arg("-c")
            .arg(compression);

        if self.verbose {
            cmd.arg("-v");
        }

        let output = cmd.output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("vfsx create failed: {}", stderr);
        }

        info!("VFSX archive created: {}", output_path.display());
        Ok(())
    }

    /// Verify a VFSX archive signature
    pub fn verify(&self, archive_path: &Path, key_path: &Path) -> Result<bool> {
        debug!("Verifying VFSX archive: {}", archive_path.display());

        let mut cmd = Command::new("vfsx");
        cmd.arg("verify")
            .arg("--key")
            .arg(key_path)
            .arg(archive_path);

        if self.verbose {
            cmd.arg("-v");
        }

        let output = cmd.output()?;

        if output.status.success() {
            info!("VFSX archive verified successfully");
            Ok(true)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!("VFSX verification failed: {}", stderr);
            Ok(false)
        }
    }

    /// Extract a VFSX archive
    pub fn extract(&self, archive_path: &Path, dest_dir: &Path) -> Result<()> {
        debug!("Extracting VFSX archive: {} -> {}", archive_path.display(), dest_dir.display());

        let mut cmd = Command::new("vfsx");
        cmd.arg("extract")
            .arg(archive_path)
            .arg("-o")
            .arg(dest_dir);

        if self.verbose {
            cmd.arg("-v");
        }

        let output = cmd.output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("vfsx extract failed: {}", stderr);
        }

        info!("VFSX archive extracted to: {}", dest_dir.display());
        Ok(())
    }

    /// List contents of a VFSX archive
    pub fn list(&self, archive_path: &Path) -> Result<String> {
        debug!("Listing VFSX archive contents: {}", archive_path.display());

        let mut cmd = Command::new("vfsx");
        cmd.arg("list")
            .arg(archive_path);

        if self.verbose {
            cmd.arg("-v");
        }

        let output = cmd.output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("vfsx list failed: {}", stderr);
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Generate a new keypair
    pub fn key_gen(&self, output_path: &Path) -> Result<()> {
        debug!("Generating VFSX keypair: {}", output_path.display());

        let mut cmd = Command::new("vfsx");
        cmd.arg("key")
            .arg("gen")
            .arg("-o")
            .arg(output_path);

        if self.verbose {
            cmd.arg("-v");
        }

        let output = cmd.output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("vfsx key gen failed: {}", stderr);
        }

        info!("VFSX keypair generated: {}", output_path.display());
        Ok(())
    }

    /// Export public key as a VFSX archive
    pub fn key_export(&self, private_key_path: &Path, public_key_path: &Path) -> Result<()> {
        debug!("Exporting public key: {} -> {}", private_key_path.display(), public_key_path.display());

        let mut cmd = Command::new("vfsx");
        cmd.arg("key")
            .arg("export")
            .arg(private_key_path)
            .arg("-o")
            .arg(public_key_path);

        if self.verbose {
            cmd.arg("-v");
        }

        let output = cmd.output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("vfsx key export failed: {}", stderr);
        }

        info!("Public key exported: {}", public_key_path.display());
        Ok(())
    }
}

impl Default for Vfsx {
    fn default() -> Self {
        Self::new(false)
    }
}
