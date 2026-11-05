//! Registry-based dependency resolver

use anyhow::Result;
use std::collections::HashMap;
use tracing::debug;

use crate::client::RegistryClient;

/// Registry-based dependency resolver
pub struct RegistryResolver {
    client: RegistryClient,
}

#[derive(Debug, Clone)]
pub struct ResolvedDependency {
    pub name: String,
    pub version: String,
    pub source: String,
    pub checksum: String,
    pub dependencies: Vec<ResolvedDependency>,
}

impl RegistryResolver {
    /// Create a new registry resolver
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: RegistryClient::new()?,
        })
    }

    /// Resolve dependencies for a project
    pub async fn resolve(
        &self,
        rust_deps: &HashMap<String, String>,
        c_deps: &HashMap<String, String>,
        go_deps: &HashMap<String, String>,
    ) -> Result<Vec<ResolvedDependency>> {
        debug!("Resolving dependencies from registry");

        let mut resolved = Vec::new();

        // Resolve Rust dependencies
        for (name, version_spec) in rust_deps {
            let dep = self.resolve_rust_dep(name, version_spec).await?;
            resolved.push(dep);
        }

        // Resolve C dependencies (from crates.io or system)
        for (name, version_spec) in c_deps {
            let dep = self.resolve_c_dep(name, version_spec).await?;
            resolved.push(dep);
        }

        // Resolve Go dependencies (from proxy.golang.org)
        for (name, version_spec) in go_deps {
            let dep = self.resolve_go_dep(name, version_spec).await?;
            resolved.push(dep);
        }

        debug!("Resolved {} dependencies", resolved.len());
        Ok(resolved)
    }

    async fn resolve_rust_dep(&self, name: &str, version_spec: &str) -> Result<ResolvedDependency> {
        debug!("Resolving Rust dependency: {} {}", name, version_spec);

        // Parse version spec (e.g., "1.0|full" -> version "1.0", features "full")
        let (version_req, _features) = self.parse_version_spec(version_spec);

        // Resolve version from crates.io
        let version = self.client.resolve_version(name, &version_req).await?;

        // Get checksum
        let checksum = self.client.get_checksum(name, &version).await?;

        Ok(ResolvedDependency {
            name: name.to_string(),
            version,
            source: "https://crates.io".to_string(),
            checksum,
            dependencies: Vec::new(), // TODO: Resolve transitive dependencies
        })
    }

    async fn resolve_c_dep(&self, name: &str, version_spec: &str) -> Result<ResolvedDependency> {
        debug!("Resolving C dependency: {} {}", name, version_spec);

        let (version, source) = self.parse_version_spec(version_spec);

        let source_url = match source.as_deref() {
            Some("system") => "system",
            _ => "https://crates.io",
        };

        // For C dependencies, we might need to check if they're available as Rust crates
        // or use pkg-config for system dependencies
        let checksum = if source_url == "system" {
            "system".to_string()
        } else {
            // Try to find as a Rust crate
            match self.client.get_checksum(name, &version).await {
                Ok(checksum) => checksum,
                Err(_) => "unknown".to_string(),
            }
        };

        Ok(ResolvedDependency {
            name: name.to_string(),
            version,
            source: source_url.to_string(),
            checksum,
            dependencies: Vec::new(),
        })
    }

    async fn resolve_go_dep(&self, name: &str, version_spec: &str) -> Result<ResolvedDependency> {
        debug!("Resolving Go dependency: {} {}", name, version_spec);

        let (version, _metadata) = self.parse_version_spec(version_spec);

        // Go dependencies come from proxy.golang.org
        // For now, we'll just record the version
        Ok(ResolvedDependency {
            name: name.to_string(),
            version,
            source: "https://proxy.golang.org".to_string(),
            checksum: "go-module".to_string(), // Go uses its own checksum database
            dependencies: Vec::new(),
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
}

impl Default for RegistryResolver {
    fn default() -> Self {
        Self::new().expect("Failed to create RegistryResolver")
    }
}
