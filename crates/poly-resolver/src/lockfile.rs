//! poly.lock file generation

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use tracing::debug;

/// Lock file for all dependencies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockFile {
    pub version: u32,
    pub generated: String,
    pub dependencies: Vec<LockEntry>,
}

/// Single dependency entry in lock file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    pub language: String,
    pub name: String,
    pub version: String,
    pub checksum: String,
    pub source: String,
    pub signature: Option<String>,
}

impl LockFile {
    /// Create a new lock file
    pub fn new() -> Self {
        Self {
            version: 1,
            generated: chrono::Utc::now().to_rfc3339(),
            dependencies: Vec::new(),
        }
    }

    /// Add a dependency entry
    pub fn add_entry(&mut self, entry: LockEntry) {
        self.dependencies.push(entry);
    }

    /// Write lock file to disk
    pub fn write(&self, path: &str) -> Result<()> {
        let content = self.to_ini_format()?;
        fs::write(path, content)?;
        debug!("Wrote lock file to {}", path);
        Ok(())
    }

    /// Read lock file from disk
    pub fn read(path: &str) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        Self::from_ini_format(&content)
    }

    /// Convert to INI format (poly.lock is INI-based)
    fn to_ini_format(&self) -> Result<String> {
        let mut output = String::new();

        output.push_str(&format!("[lock]\n"));
        output.push_str(&format!("version = {}\n", self.version));
        output.push_str(&format!("generated = {}\n", self.generated));
        output.push_str("\n");

        for dep in &self.dependencies {
            output.push_str(&format!("[dependency.{}.{}]\n", dep.language, dep.name));
            output.push_str(&format!("version = {}\n", dep.version));
            if !dep.checksum.is_empty() {
                output.push_str(&format!("checksum = {}\n", dep.checksum));
            }
            output.push_str(&format!("source = {}\n", dep.source));
            if let Some(sig) = &dep.signature {
                output.push_str(&format!("signature = {}\n", sig));
            }
            output.push_str("\n");
        }

        Ok(output)
    }

    /// Parse from INI format
    fn from_ini_format(content: &str) -> Result<Self> {
        // Simple INI parsing for MVP
        let mut version = 1;
        let mut generated = String::new();
        let mut dependencies = Vec::new();

        let mut current_section: Option<String> = None;
        let mut current_dep: Option<LockEntry> = None;

        for line in content.lines() {
            let line = line.trim();

            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if line.starts_with('[') && line.ends_with(']') {
                // Save previous dependency
                if let Some(dep) = current_dep.take() {
                    dependencies.push(dep);
                }

                let section = line.trim_start_matches('[').trim_end_matches(']');
                current_section = Some(section.to_string());

                if section == "lock" {
                    continue;
                }

                if section.starts_with("dependency.") {
                    let parts: Vec<&str> = section.strip_prefix("dependency.").unwrap().split('.').collect();
                    if parts.len() == 2 {
                        current_dep = Some(LockEntry {
                            language: parts[0].to_string(),
                            name: parts[1].to_string(),
                            version: String::new(),
                            checksum: String::new(),
                            source: String::new(),
                            signature: None,
                        });
                    }
                }
            } else if let Some(key_value) = line.split_once('=') {
                let key = key_value.0.trim();
                let value = key_value.1.trim();

                match current_section.as_deref() {
                    Some("lock") => {
                        match key {
                            "version" => version = value.parse()?,
                            "generated" => generated = value.to_string(),
                            _ => {}
                        }
                    }
                    Some(_) if current_dep.is_some() => {
                        let dep = current_dep.as_mut().unwrap();
                        match key {
                            "version" => dep.version = value.to_string(),
                            "checksum" => dep.checksum = value.to_string(),
                            "source" => dep.source = value.to_string(),
                            "signature" => dep.signature = Some(value.to_string()),
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }

        // Save last dependency
        if let Some(dep) = current_dep {
            dependencies.push(dep);
        }

        Ok(Self {
            version,
            generated,
            dependencies,
        })
    }
}

impl Default for LockFile {
    fn default() -> Self {
        Self::new()
    }
}
