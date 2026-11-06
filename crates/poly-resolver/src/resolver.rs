//! SAT-based dependency resolver

use anyhow::Result;
use crate::lockfile::LockEntry;
use std::collections::HashMap;
use tracing::debug;

/// Dependency resolver using SAT solving
pub struct DependencyResolver {
    // For MVP, we'll use a simple resolver
    // Full SAT solver integration will come later
}

impl DependencyResolver {
    pub fn new() -> Self {
        Self {}
    }

    /// Resolve dependencies and produce lock entries
    pub fn resolve(
        &self,
        rust_deps: &HashMap<String, String>,
        c_deps: &HashMap<String, String>,
        go_deps: &HashMap<String, String>,
    ) -> Result<Vec<LockEntry>> {
        debug!("Resolving dependencies");

        let mut entries = Vec::new();

        // Resolve Rust dependencies
        for (name, version_spec) in rust_deps {
            let entry = self.resolve_rust_dep(name, version_spec)?;
            entries.push(entry);
        }

        // Resolve C dependencies
        for (name, version_spec) in c_deps {
            let entry = self.resolve_c_dep(name, version_spec)?;
            entries.push(entry);
        }

        // Resolve Go dependencies
        for (name, version_spec) in go_deps {
            let entry = self.resolve_go_dep(name, version_spec)?;
            entries.push(entry);
        }

        debug!("Resolved {} dependencies", entries.len());
        Ok(entries)
    }

    fn resolve_rust_dep(&self, name: &str, version_spec: &str) -> Result<LockEntry> {
        // MVP: simple version resolution
        // Parse metadata (e.g., "1.0|full" -> version "1.0", features "full")
        let (version, _metadata) = self.parse_version_spec(version_spec);

        // Rust deps come from crates.io, which mandates sha256 checksums.
        Ok(LockEntry {
            language: "rust".to_string(),
            name: name.to_string(),
            version,
            checksum: self.placeholder_checksum("sha256"),
            source: "https://crates.io".to_string(),
            signature: None,
        })
    }

    fn resolve_c_dep(&self, name: &str, version_spec: &str) -> Result<LockEntry> {
        let (version, source) = self.parse_version_spec(version_spec);

        // `|poly` pulls from the Poly registry — vfsx archives, content-addressed
        // by blake3 and ed25519-signed (spec §5.1, §6.1). `|system` uses
        // pkg-config; `git:`/`path:` values are non-registry sources. Anything
        // else defaults to crates.io (sha256, registry-mandated).
        let (source_url, checksum, signature) = match source.as_deref() {
            Some("poly") => (
                poly_registry_url(),
                self.placeholder_checksum("blake3"),
                Some("vfsx:pending".to_string()),
            ),
            Some("system") => ("system".to_string(), String::new(), None),
            _ if version.starts_with("git:") || version.starts_with("path:") => {
                (version.clone(), String::new(), None)
            }
            _ => (
                "https://crates.io".to_string(),
                self.placeholder_checksum("sha256"),
                None,
            ),
        };

        Ok(LockEntry {
            language: "c".to_string(),
            name: name.to_string(),
            version,
            checksum,
            source: source_url,
            signature,
        })
    }

    fn resolve_go_dep(&self, name: &str, version_spec: &str) -> Result<LockEntry> {
        let (version, _metadata) = self.parse_version_spec(version_spec);

        // proxy.golang.org is an external registry — sha256 placeholder until
        // the real dirhash checksum is recorded at download time.
        Ok(LockEntry {
            language: "go".to_string(),
            name: name.to_string(),
            version,
            checksum: self.placeholder_checksum("sha256"),
            source: "https://proxy.golang.org".to_string(),
            signature: None,
        })
    }

    fn parse_version_spec(&self, spec: &str) -> (String, Option<String>) {
        // Parse "version|metadata" format
        if let Some((version, metadata)) = spec.split_once('|') {
            (version.to_string(), Some(metadata.to_string()))
        } else {
            (spec.to_string(), None)
        }
    }

    /// Placeholder checksum for MVP, prefixed with the integrity scheme:
    /// `sha256:` for external registries that mandate it, `blake3:` for
    /// vfsx-archive packages (vfsx content addressing is blake3-based).
    /// The real value is filled once the package is downloaded.
    fn placeholder_checksum(&self, scheme: &str) -> String {
        const ZEROES: &str =
            "0000000000000000000000000000000000000000000000000000000000000000";
        format!("{}:{}", scheme, ZEROES)
    }
}

/// Poly registry URL for `|poly` dependencies: `POLY_REGISTRY_URL` when set,
/// otherwise the `poly://` scheme marker until a canonical registry exists.
fn poly_registry_url() -> String {
    std::env::var("POLY_REGISTRY_URL").unwrap_or_else(|_| "poly://registry".to_string())
}

impl Default for DependencyResolver {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of dependency resolution
#[derive(Debug)]
pub struct ResolutionResult {
    pub entries: Vec<LockEntry>,
    pub conflicts: Vec<String>,
}
