//! Configuration model for poly.ini

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Complete poly.ini configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub project: Project,
    pub workspace: Option<Workspace>,
    pub targets: HashMap<String, Target>,
    pub dependencies: Dependencies,
    pub build: BuildConfig,
    pub publish: Option<PublishConfig>,
    pub vfsx: Option<VfsxConfig>,
    pub lint: Option<LintConfig>,
    pub format: Option<FormatConfig>,
    pub cache: Option<CacheConfig>,
    pub toolchain: Option<ToolchainConfig>,
    pub cross: Option<CrossConfig>,
    pub plugins: Option<HashMap<String, String>>,
    pub hooks: Option<HooksConfig>,
    pub secrets: Option<HashMap<String, String>>,
}

/// Project metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub version: String,
    pub edition: Option<String>,
    // crates.io / registry publish metadata (all optional)
    pub description: Option<String>,
    pub license: Option<String>,
    pub license_file: Option<String>,
    pub homepage: Option<String>,
    pub documentation: Option<String>,
    pub repository: Option<String>,
    pub readme: Option<String>,
    pub keywords: Vec<String>,
    pub categories: Vec<String>,
    pub authors: Vec<String>,
}

/// Workspace configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub members: Vec<String>,
    #[serde(rename = "sharedDeps")]
    #[allow(non_snake_case)]
    pub shared_deps: Vec<String>,
}

/// Build target
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub language: String,
    pub type_: String,
    pub sources: Vec<String>,
    pub compiler: Option<String>,
    pub flags: Vec<String>,
    pub edition: Option<String>,
    pub features: Vec<String>,
    pub module: Option<String>,
    pub cgo: Option<bool>,
}

/// Dependencies by language
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependencies {
    pub rust: HashMap<String, String>,
    pub c: HashMap<String, String>,
    pub go: HashMap<String, String>,
    pub workspace: HashMap<String, HashMap<String, String>>,
}

/// Build configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    pub default: BuildProfile,
    pub profiles: HashMap<String, BuildProfile>,
}

/// Build profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildProfile {
    pub targets: Vec<String>,
    pub rust_flags: Vec<String>,
    pub c_flags: Vec<String>,
    pub go_flags: Vec<String>,
}

/// Publish configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishConfig {
    pub targets: Vec<String>,
    pub platforms: Vec<String>,
    pub include_docs: bool,
    /// Registry base URL (e.g. https://registry.example.com). None = local only.
    pub registry: Option<String>,
    /// Env var holding the registry auth token.
    pub token_env: Option<String>,
    /// Override the crates.io crate name. None = auto-generated stub name.
    pub crate_name: Option<String>,
}

/// VFSX signing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VfsxConfig {
    pub sign: bool,
    pub keyfile: String,
    pub verify: bool,
    pub trust_store: String,
}

/// Linting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintConfig {
    pub rust: String,
    pub c: String,
    pub go: String,
    pub run_on_build: bool,
}

/// Formatting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatConfig {
    pub rust: String,
    pub c: String,
    pub go: String,
}

/// Cache configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    pub local: String,
    pub remote: Option<String>,
    pub enabled: bool,
}

/// Toolchain configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolchainConfig {
    pub rust: ToolchainSpec,
    pub c: ToolchainSpec,
    pub go: ToolchainSpec,
}

/// Toolchain specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolchainSpec {
    pub version: String,
    pub channel: Option<String>,
    pub compiler: Option<String>,
}

/// Cross-compilation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossConfig {
    pub targets: Vec<String>,
    pub sysroot: String,
}

/// Hooks configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HooksConfig {
    pub pre_build: Option<Hook>,
    pub post_build: Option<Hook>,
    pub pre_test: Option<Hook>,
    pub post_test: Option<Hook>,
}

/// Hook definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    pub command: String,
}
