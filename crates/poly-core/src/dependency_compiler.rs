//! Cargo-like dependency compiler for building dependencies in correct order
//!
//! This module implements a simplified version of Cargo's build system:
//! - Resolves dependencies from crates.io into name+version package identities
//! - Unifies features and activates optional dependencies
//! - Runs build scripts (build.rs) with Cargo-compatible env vars
//! - Compiles host units (proc macros / build deps) separately from target units
//! - Compiles in topological order
//! - Passes exact --extern flags for direct dependencies only

use anyhow::{Context, Result};
use crate::dependency_downloader::{DependencyDownloader, DownloadedDependency};
use poly_registry::{CrateDependency, RegistryClient};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info};

/// Dependency compiler
pub struct DependencyCompiler {
    downloader: DependencyDownloader,
    registry_client: RegistryClient,
    build_dir: PathBuf,
    host_triple: String,
    rustc_version: String,
}

#[derive(Debug, Clone)]
pub struct CompiledDependency {
    pub name: String,
    pub version: String,
    pub lib_name: String,
    pub rlib_path: PathBuf,
    pub dependencies: Vec<String>,
    pub is_proc_macro: bool,
    pub is_host: bool,
    pub target: String,
    pub build_script_output: Option<BuildOutput>,
    /// Value of `links = "..."` in the crate's Cargo.toml, if any.
    pub links_name: Option<String>,
    /// Cache fingerprint for this unit; `""` if not computed.
    pub fingerprint: String,
    /// Checksum from the registry, if known.
    pub checksum: String,
}

/// Output from a build script
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildOutput {
    pub cfgs: Vec<String>,
    pub env: Vec<(String, String)>,
    pub rustc_flags: Vec<String>,
    pub link_search_paths: Vec<PathBuf>,
    pub link_libs: Vec<String>,
    /// `cargo:KEY=VALUE` metadata, propagated to dependents as DEP_<LINKS>_<KEY>.
    pub metadata: Vec<(String, String)>,
}

impl BuildOutput {
    /// Parse build script output from stdout
    fn parse(output: &str) -> Self {
        let mut result = BuildOutput::default();

        for line in output.lines() {
            let line = line.trim();

            // Parse cargo: directives (both `cargo:inst` and `cargo::inst`)
            if let Some(rest) = line.strip_prefix("cargo:") {
                let rest = rest.strip_prefix(':').unwrap_or(rest);
                if let Some(value) = rest.strip_prefix("rustc-cfg=") {
                    result.cfgs.push(value.to_string());
                } else if let Some(value) = rest.strip_prefix("rustc-env=") {
                    if let Some((key, val)) = value.split_once('=') {
                        result.env.push((key.to_string(), val.to_string()));
                    }
                } else if let Some(value) = rest.strip_prefix("rustc-link-search=") {
                    // Handle both native= and framework= paths
                    if let Some(path) = value.split('=').nth(1) {
                        result.link_search_paths.push(PathBuf::from(path));
                    } else {
                        result.link_search_paths.push(PathBuf::from(value));
                    }
                } else if let Some(value) = rest.strip_prefix("rustc-link-lib=") {
                    result.link_libs.push(value.to_string());
                } else if let Some(value) = rest.strip_prefix("rustc-flags=") {
                    result.rustc_flags.extend(value.split_whitespace().map(|s| s.to_string()));
                } else if !rest.starts_with("rustc-")
                    && !rest.starts_with("warning")
                    && !rest.starts_with("rerun-if")
                {
                    // `cargo:KEY=VALUE` links metadata (e.g. `cargo:root=/usr`).
                    if let Some((key, val)) = rest.split_once('=') {
                        let key = key.trim();
                        if !key.is_empty()
                            && key
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                        {
                            result.metadata.push((key.to_string(), val.to_string()));
                        }
                    }
                }
            }
        }

        result
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct PackageId {
    name: String,
    version: String,
}

impl PackageId {
    fn key(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

#[derive(Debug, Clone)]
struct RawDep {
    name: String,
    extern_name: String,
    req: String,
    features: Vec<String>,
    optional: bool,
    default_features: bool,
    kind: String,
    resolved: Option<PackageId>,
}

#[derive(Debug)]
struct PackageNode {
    dep: DownloadedDependency,
    raw_deps: Vec<RawDep>,
    deps_fetched: bool,
    feature_defs: Option<HashMap<String, Vec<String>>>,
    is_proc_macro: bool,
    crate_type: String,
    edition: String,
    lib_name: String,
    lib_path: String,
}

#[derive(Debug, Default)]
struct ResolveState {
    packages: HashMap<PackageId, PackageNode>,
    requested: HashMap<PackageId, BTreeSet<String>>,
    expanded: HashMap<PackageId, BTreeSet<String>>,
    pending: VecDeque<(PackageId, String)>,
    /// Versions pinned by poly.lock (name -> exact version).
    locked: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum UnitKind {
    Host,
    Target,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct UnitId {
    pkg: PackageId,
    kind: UnitKind,
}

impl UnitId {
    fn key(&self) -> String {
        let kind = match self.kind {
            UnitKind::Host => "host",
            UnitKind::Target => "target",
        };
        format!("{}:{}", kind, self.pkg.key())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DepEdge {
    unit: UnitId,
    extern_name: String,
}

#[derive(Debug)]
struct UnitNode {
    id: UnitId,
    lib_deps: Vec<DepEdge>,
    build_deps: Vec<DepEdge>,
    all_deps: Vec<UnitId>,
}

impl DependencyCompiler {
    /// Create a new dependency compiler
    pub fn new(build_dir: PathBuf) -> Result<Self> {
        Ok(Self {
            downloader: DependencyDownloader::new()?,
            registry_client: RegistryClient::new()?,
            build_dir,
            host_triple: Self::detect_host_triple(),
            rustc_version: Self::detect_rustc_version(),
        })
    }

    fn detect_rustc_version() -> String {
        if let Ok(output) = Command::new("rustc").arg("-vV").output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if let Some(v) = line.strip_prefix("release:") {
                        return v.trim().to_string();
                    }
                }
            }
        }
        "unknown".to_string()
    }

    fn detect_host_triple() -> String {
        if let Ok(output) = Command::new("rustc").arg("-vV").output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if let Some(host) = line.strip_prefix("host:") {
                        let host = host.trim();
                        if !host.is_empty() {
                            return host.to_string();
                        }
                    }
                }
            }
        }
        "x86_64-unknown-linux-gnu".to_string()
    }

    /// Compile all dependencies for a project
    pub async fn compile_dependencies(
        &self,
        rust_deps: &HashMap<String, String>,
        workspace_dir: &std::path::Path,
        target_triple: &str,
        profile: &str,
        locked_versions: &HashMap<String, String>,
    ) -> Result<HashMap<String, CompiledDependency>> {
        info!("Compiling {} Rust dependencies for {}", rust_deps.len(), target_triple);

        // Analyze workspace source code to find actually-used dependencies
        let analyzer = crate::source_analyzer::DependencyAnalyzer::new()?;
        let used_crates = analyzer.analyze_directory(workspace_dir)?;
        info!("Found {} crates used in source code", used_crates.len());
        debug!("Used crates: {:?}", used_crates);

        // Filter dependencies to only those actually used
        let needed_deps: HashMap<String, String> = rust_deps
            .iter()
            .filter(|(name, _)| {
                let normalized = name.replace('-', "_");
                used_crates.contains(normalized.as_str()) || used_crates.contains(name.as_str())
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        info!("Filtered to {} needed dependencies", needed_deps.len());

        let mut state = ResolveState {
            locked: locked_versions.clone(),
            ..ResolveState::default()
        };
        let mut root_ids = Vec::new();

        for (name, spec) in &needed_deps {
            let (version_req, features) = Self::parse_version_spec(spec);
            let id = self.ensure_package(&mut state, name, &version_req).await?;
            root_ids.push(id.clone());

            if let Some(feature_list) = features {
                for feature in feature_list.split(',').map(str::trim).filter(|f| !f.is_empty()) {
                    request_feature(&mut state, id.clone(), feature);
                }
            }
            // Cargo enables default features unless default-features = false.
            request_feature(&mut state, id.clone(), "default");
        }

        self.resolve_graph(&mut state, target_triple).await?;
        info!("Resolved {} packages", state.packages.len());

        let units = self.compute_units(&state, &root_ids)?;
        info!("Computed {} compilation units", units.len());

        let compiled_units = self.compile_units(&state, &units, target_triple, profile)?;
        info!("Compiled {} dependency units", compiled_units.len());

        let mut compiled = HashMap::new();
        for (unit_id, dep) in compiled_units {
            compiled.insert(unit_id.key(), dep);
        }
        Ok(compiled)
    }

    /// Compile all dependencies with features (backwards-compatible wrapper)
    pub async fn compile_dependencies_with_features(
        &self,
        rust_deps: &HashMap<String, String>,
        workspace_dir: &std::path::Path,
        target_triple: &str,
        profile: &str,
    ) -> Result<HashMap<String, CompiledDependency>> {
        self.compile_dependencies(rust_deps, workspace_dir, target_triple, profile, &HashMap::new()).await
    }

    /// Parse version spec (e.g., "1.0|full" -> ("1.0", Some("full")))
    fn parse_version_spec(spec: &str) -> (String, Option<String>) {
        if let Some((version, metadata)) = spec.split_once('|') {
            (version.to_string(), Some(metadata.to_string()))
        } else {
            (spec.to_string(), None)
        }
    }

    fn find_existing_package(
        &self,
        state: &ResolveState,
        name: &str,
        req: &str,
    ) -> Option<PackageId> {
        state
            .packages
            .keys()
            .find(|id| id.name == name && self.registry_client.version_matches(&id.version, req))
            .cloned()
    }

    async fn ensure_package(
        &self,
        state: &mut ResolveState,
        name: &str,
        req: &str,
    ) -> Result<PackageId> {
        if let Some(id) = self.find_existing_package(state, name, req) {
            return Ok(id);
        }

        let exact = PackageId {
            name: name.to_string(),
            version: String::new(),
        };
        let _ = exact;

        // Honor poly.lock: if a locked version exists and satisfies the
        // requirement, use it instead of resolving to latest.
        let mut used_locked = false;
        let mut version = if let Some(locked) = state.locked.get(name) {
            if self.registry_client.version_matches(locked, req) {
                debug!("Using locked version {} for {}", locked, name);
                used_locked = true;
                locked.clone()
            } else {
                debug!(
                    "Locked version {} for {} does not satisfy {}; re-resolving",
                    locked, name, req
                );
                self.registry_client
                    .resolve_version(name, req)
                    .await
                    .with_context(|| format!("Failed to resolve {}@{}", name, req))?
            }
        } else {
            self.registry_client
                .resolve_version(name, req)
                .await
                .with_context(|| format!("Failed to resolve {}@{}", name, req))?
        };

        let mut dep = self.downloader.download(name, &version).await;
        if dep.is_err() && used_locked {
            // Locked version no longer available — fall back to latest match.
            debug!(
                "Locked download of {}@{} failed; re-resolving",
                name, version
            );
            version = self
                .registry_client
                .resolve_version(name, req)
                .await
                .with_context(|| format!("Failed to resolve {}@{}", name, req))?;
            dep = self.downloader.download(name, &version).await;
        }
        let dep = dep.with_context(|| format!("Failed to download {}@{}", name, version))?;

        let id = PackageId {
            name: name.to_string(),
            version,
        };
        if !state.packages.contains_key(&id) {
            state.packages.insert(
                id.clone(),
                PackageNode {
                    dep,
                    raw_deps: Vec::new(),
                    deps_fetched: false,
                    feature_defs: None,
                    is_proc_macro: false,
                    crate_type: "lib".to_string(),
                    edition: "2021".to_string(),
                    lib_name: name.replace('-', "_"),
                    lib_path: "src/lib.rs".to_string(),
                },
            );
        }
        Ok(id)
    }

    async fn resolve_graph(&self, state: &mut ResolveState, target_triple: &str) -> Result<()> {
        for _ in 0..10_000 {
            let mut progress = false;

            let ids: Vec<PackageId> = state.packages.keys().cloned().collect();
            for id in ids {
                let needs_fetch = state
                    .packages
                    .get(&id)
                    .map(|node| !node.deps_fetched)
                    .unwrap_or(false);
                if needs_fetch {
                    self.fetch_package_deps(state, &id, target_triple).await?;
                    progress = true;
                }
            }

            while let Some((id, feature)) = state.pending.pop_front() {
                self.expand_feature(state, &id, &feature, target_triple).await?;
                progress = true;
            }

            if !progress {
                return Ok(());
            }
        }
        anyhow::bail!("Dependency resolution did not converge")
    }

    async fn fetch_package_deps(
        &self,
        state: &mut ResolveState,
        id: &PackageId,
        target_triple: &str,
    ) -> Result<()> {
        let (dep, crate_type, edition, lib_name, is_proc_macro, feature_defs) = {
            let node = state
                .packages
                .get(id)
                .with_context(|| format!("Missing package {}", id.key()))?;
            (
                node.dep.clone(),
                node.crate_type.clone(),
                node.edition.clone(),
                node.lib_name.clone(),
                node.is_proc_macro,
                node.feature_defs.clone(),
            )
        };
        let _ = (crate_type, edition, lib_name, is_proc_macro, feature_defs);

        let crate_deps = self
            .registry_client
            .get_crate_dependencies(&dep.name, &dep.version)
            .await
            .with_context(|| format!("Failed to get dependencies for {}@{}", dep.name, dep.version))?;

        let extern_names = Self::manifest_extern_names(&dep.path);
        let mut raw_deps = Vec::new();
        for crate_dep in crate_deps {
            if crate_dep.kind == "dev" {
                continue;
            }
            if let Some(target) = &crate_dep.target {
                // The API does not always populate `target`; RawDep no longer stores it because
                // compatibility is evaluated while fetching for this specific target triple.
                let _ = target;
            }
            if !self.crate_dep_target_matches(&crate_dep, target_triple) {
                continue;
            }

            let mut raw = RawDep {
                name: crate_dep.name.clone(),
                extern_name: extern_names
                    .get(&crate_dep.name)
                    .cloned()
                    .unwrap_or_else(|| crate_dep.name.replace('-', "_")),
                req: crate_dep.req.clone(),
                features: crate_dep.features.clone(),
                optional: crate_dep.optional,
                default_features: crate_dep.default_features,
                kind: crate_dep.kind.clone(),
                resolved: None,
            };

            if !raw.optional {
                let child = self.ensure_package(state, &raw.name, &raw.req).await?;
                seed_edge_features(state, child.clone(), &raw);
                raw.resolved = Some(child);
            }

            raw_deps.push(raw);
        }

        let node = state
            .packages
            .get_mut(id)
            .with_context(|| format!("Missing package {}", id.key()))?;
        node.raw_deps = raw_deps;
        node.deps_fetched = true;

        // Populate metadata lazily parsed from the downloaded Cargo.toml.
        let dep = state.packages.get(id).unwrap().dep.clone();
        let crate_type = Self::detect_crate_type(&dep);
        let edition = Self::detect_edition(&dep);
        let lib_name = Self::detect_lib_name(&dep);
        let lib_path = Self::detect_lib_path(&dep);
        let is_proc_macro = crate_type == "proc-macro";
        let feature_defs = Self::parse_cargo_features(&dep.path).unwrap_or_default();
        let node = state.packages.get_mut(id).unwrap();
        node.crate_type = crate_type;
        node.edition = edition;
        node.lib_name = lib_name;
        node.lib_path = lib_path;
        node.is_proc_macro = is_proc_macro;
        node.feature_defs = Some(feature_defs);

        Ok(())
    }

    fn crate_dep_target_matches(&self, dep: &CrateDependency, target_triple: &str) -> bool {
        match &dep.target {
            None => true,
            Some(target) => target_expr_matches(target, target_triple),
        }
    }

    async fn expand_feature(
        &self,
        state: &mut ResolveState,
        id: &PackageId,
        feature: &str,
        target_triple: &str,
    ) -> Result<()> {
        // A package's feature table is only available after its dependency metadata is fetched.
        let needs_fetch = state
            .packages
            .get(id)
            .map(|node| !node.deps_fetched)
            .unwrap_or(false);
        if needs_fetch {
            self.fetch_package_deps(state, id, target_triple).await?;
        }

        let (feature_defs, optional_names) = {
            let node = state
                .packages
                .get(id)
                .with_context(|| format!("Missing package {}", id.key()))?;
            let defs = node.feature_defs.clone().unwrap_or_default();
            let optional_names: Vec<String> = node
                .raw_deps
                .iter()
                .filter(|dep| dep.optional)
                .map(|dep| dep.extern_name.clone())
                .collect();
            (defs, optional_names)
        };

        if let Some(values) = feature_defs.get(feature) {
            for value in values.clone() {
                self.expand_feature_value(state, id, &value, target_triple).await?;
            }
        }

        // Optional dependencies create implicit features unless `dep:` syntax is used.
        if optional_names.iter().any(|name| dep_name_matches(name, feature)) {
            self.activate_dependency(state, id, feature, target_triple).await?;
        }

        Ok(())
    }

    async fn expand_feature_value(
        &self,
        state: &mut ResolveState,
        id: &PackageId,
        value: &str,
        target_triple: &str,
    ) -> Result<()> {
        let value = value.trim();
        if value.is_empty() {
            return Ok(());
        }

        if let Some(dep_name) = value.strip_prefix("dep:") {
            self.activate_dependency(state, id, dep_name, target_triple).await?;
            return Ok(());
        }

        if let Some((dep_part, feature_name)) = value.split_once('/') {
            let weak = dep_part.ends_with('?');
            let dep_name = dep_part.trim_end_matches('?');

            if weak {
                if let Some(child) = self.resolved_dependency(state, id, dep_name) {
                    request_feature(state, child, feature_name);
                }
            } else {
                if let Some(child) = self.activate_dependency(state, id, dep_name, target_triple).await? {
                    request_feature(state, child, feature_name);
                }
            }
            return Ok(());
        }

        // Plain value: a feature name (which may itself expand to `dep:` entries)
        // or an implicit optional-dependency feature. `expand_feature` handles both.
        request_feature(state, id.clone(), value);

        Ok(())
    }

    async fn activate_dependency(
        &self,
        state: &mut ResolveState,
        id: &PackageId,
        dep_name: &str,
        _target_triple: &str,
    ) -> Result<Option<PackageId>> {
        let raw = {
            let node = state
                .packages
                .get(id)
                .with_context(|| format!("Missing package {}", id.key()))?;
            node.raw_deps
                .iter()
                .find(|dep| raw_dep_name_matches(dep, dep_name))
                .cloned()
        };

        let Some(raw) = raw else {
            return Ok(None);
        };

        if let Some(child) = raw.resolved {
            return Ok(Some(child));
        }

        if !raw.optional {
            return Ok(None);
        }

        let child = self.ensure_package(state, &raw.name, &raw.req).await?;
        seed_edge_features(state, child.clone(), &raw);

        let node = state
            .packages
            .get_mut(id)
            .with_context(|| format!("Missing package {}", id.key()))?;
        if let Some(slot) = node
            .raw_deps
            .iter_mut()
            .find(|dep| raw_dep_name_matches(dep, dep_name))
        {
            slot.resolved = Some(child.clone());
        }

        Ok(Some(child))
    }

    fn resolved_dependency(
        &self,
        state: &ResolveState,
        id: &PackageId,
        dep_name: &str,
    ) -> Option<PackageId> {
        state
            .packages
            .get(id)?
            .raw_deps
            .iter()
            .find(|dep| raw_dep_name_matches(dep, dep_name))
            .and_then(|dep| dep.resolved.clone())
    }

    fn compute_units(
        &self,
        state: &ResolveState,
        root_ids: &[PackageId],
    ) -> Result<HashMap<UnitId, UnitNode>> {
        let mut units: HashMap<UnitId, UnitNode> = HashMap::new();
        let mut queue = VecDeque::new();

        for root in root_ids {
            let is_proc_macro = state
                .packages
                .get(root)
                .map(|node| node.is_proc_macro)
                .unwrap_or(false);
            let kind = if is_proc_macro { UnitKind::Host } else { UnitKind::Target };
            let unit_id = UnitId {
                pkg: root.clone(),
                kind,
            };
            if !units.contains_key(&unit_id) {
                units.insert(
                    unit_id.clone(),
                    UnitNode {
                        id: unit_id.clone(),
                        lib_deps: Vec::new(),
                        build_deps: Vec::new(),
                        all_deps: Vec::new(),
                    },
                );
                queue.push_back(unit_id);
            }
        }

        while let Some(unit_id) = queue.pop_front() {
            let raw_deps = state
                .packages
                .get(&unit_id.pkg)
                .map(|node| node.raw_deps.clone())
                .unwrap_or_default();

            let mut lib_deps: Vec<DepEdge> = Vec::new();
            let mut build_deps: Vec<DepEdge> = Vec::new();

            for raw in raw_deps {
                let Some(child_pkg) = raw.resolved.clone() else {
                    continue;
                };
                let child_is_proc_macro = state
                    .packages
                    .get(&child_pkg)
                    .map(|node| node.is_proc_macro)
                    .unwrap_or(false);

                let (child_kind, is_build_dep) = match unit_id.kind {
                    UnitKind::Host => (
                        UnitKind::Host,
                        raw.kind == "build",
                    ),
                    UnitKind::Target => {
                        if raw.kind == "build" {
                            (UnitKind::Host, true)
                        } else if child_is_proc_macro {
                            (UnitKind::Host, false)
                        } else {
                            (UnitKind::Target, false)
                        }
                    }
                };

                let child_unit = UnitId {
                    pkg: child_pkg,
                    kind: child_kind,
                };
                if !units.contains_key(&child_unit) {
                    units.insert(
                        child_unit.clone(),
                        UnitNode {
                            id: child_unit.clone(),
                            lib_deps: Vec::new(),
                            build_deps: Vec::new(),
                            all_deps: Vec::new(),
                        },
                    );
                    queue.push_back(child_unit.clone());
                }

                let edge = DepEdge {
                    unit: child_unit,
                    extern_name: raw.extern_name.clone(),
                };
                if is_build_dep {
                    build_deps.push(edge);
                } else {
                    lib_deps.push(edge);
                }
            }

            lib_deps.sort_by_key(|edge| edge.unit.key());
            lib_deps.dedup_by(|a, b| a.unit == b.unit);
            build_deps.sort_by_key(|edge| edge.unit.key());
            build_deps.dedup_by(|a, b| a.unit == b.unit);

            let mut all_deps: Vec<UnitId> = lib_deps
                .iter()
                .map(|edge| edge.unit.clone())
                .chain(build_deps.iter().map(|edge| edge.unit.clone()))
                .collect();
            all_deps.sort_by_key(|unit| unit.key());
            all_deps.dedup();

            if let Some(node) = units.get_mut(&unit_id) {
                node.lib_deps = lib_deps;
                node.build_deps = build_deps;
                node.all_deps = all_deps;
            }
        }

        Ok(units)
    }

    fn topo_units(&self, units: &HashMap<UnitId, UnitNode>) -> Result<Vec<UnitId>> {
        let mut dependents: HashMap<UnitId, Vec<UnitId>> = HashMap::new();
        let mut indegree: HashMap<UnitId, usize> = HashMap::new();

        for unit_id in units.keys() {
            indegree.insert(unit_id.clone(), 0);
        }
        for (unit_id, node) in units {
            for dep in &node.all_deps {
                dependents.entry(dep.clone()).or_default().push(unit_id.clone());
                *indegree.get_mut(unit_id).unwrap() += 1;
            }
        }

        let mut ready: Vec<UnitId> = indegree
            .iter()
            .filter(|(_, degree)| **degree == 0)
            .map(|(unit, _)| unit.clone())
            .collect();
        ready.sort_by_key(|unit| unit.key());

        let mut queue = VecDeque::from(ready);
        let mut result = Vec::new();

        while let Some(unit) = queue.pop_front() {
            result.push(unit.clone());
            if let Some(children) = dependents.get(&unit) {
                for child in children {
                    let degree = indegree.get_mut(child).unwrap();
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(child.clone());
                    }
                }
            }
        }

        if result.len() != units.len() {
            anyhow::bail!("Circular dependency detected in Rust dependency graph");
        }

        Ok(result)
    }

    fn compile_units(
        &self,
        state: &ResolveState,
        units: &HashMap<UnitId, UnitNode>,
        target_triple: &str,
        profile: &str,
    ) -> Result<HashMap<UnitId, CompiledDependency>> {
        let order = self.topo_units(units)?;
        let mut compiled = HashMap::new();

        for unit_id in order {
            let unit = units.get(&unit_id).unwrap();
            let pkg = state
                .packages
                .get(&unit_id.pkg)
                .with_context(|| format!("Missing package {}", unit_id.pkg.key()))?;
            let dep = self.compile_unit(state, unit, pkg, &compiled, target_triple, profile)?;
            compiled.insert(unit_id.clone(), dep);
        }

        Ok(compiled)
    }

    fn compile_unit(
        &self,
        state: &ResolveState,
        unit: &UnitNode,
        pkg: &PackageNode,
        compiled: &HashMap<UnitId, CompiledDependency>,
        target_triple: &str,
        profile: &str,
    ) -> Result<CompiledDependency> {
        let dep = &pkg.dep;
        let lib_name = pkg.lib_name.clone();
        let is_proc_macro = pkg.is_proc_macro;
        let unit_triple = match unit.id.kind {
            UnitKind::Host => self.host_triple.as_str(),
            UnitKind::Target => target_triple,
        };

        let output_dir = self
            .build_dir
            .join("deps")
            .join(unit_triple)
            .join(format!("{}-{}", dep.name, dep.version));
        fs::create_dir_all(&output_dir)
            .with_context(|| format!("Failed to create {}", output_dir.display()))?;

        let artifact_path = if is_proc_macro {
            let ext = if self.host_triple.contains("windows") {
                "dll"
            } else if self.host_triple.contains("apple") || self.host_triple.contains("darwin") {
                "dylib"
            } else {
                "so"
            };
            output_dir.join(format!("lib{}-{}.{}", lib_name, dep.version, ext))
        } else {
            output_dir.join(format!("lib{}-{}.rlib", lib_name, dep.version))
        };

        let out_dir = output_dir.join("build");
        fs::create_dir_all(&out_dir)?;
        let out_dir_abs = fs::canonicalize(&out_dir)?;

        let features = self.enabled_features(state, &unit.id.pkg);

        // Fingerprint covers everything that affects the artifact: identity,
        // features, toolchain, profile, and transitive dep fingerprints.
        let fingerprint = {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (
                &dep.name,
                &dep.version,
                unit.id.kind,
                unit_triple,
                profile,
                &features,
                &pkg.edition,
                &pkg.crate_type,
                &pkg.lib_path,
                &self.rustc_version,
            )
                .hash(&mut hasher);
            for dep_unit in &unit.all_deps {
                if let Some(compiled_dep) = compiled.get(dep_unit) {
                    compiled_dep.fingerprint.hash(&mut hasher);
                }
            }
            format!("{:016x}", hasher.finish())
        };

        let fingerprint_path = output_dir.join(".poly-fingerprint");
        let cached_build_output_path = out_dir.join("build_output.json");
        if artifact_path.exists()
            && fs::read_to_string(&fingerprint_path)
                .map(|f| f.trim() == fingerprint)
                .unwrap_or(false)
        {
            let build_output = fs::read_to_string(&cached_build_output_path)
                .ok()
                .and_then(|s| serde_json::from_str::<Option<BuildOutput>>(&s).ok())
                .flatten();
            debug!("Cache hit for {}@{} ({})", dep.name, dep.version, unit.id.key());
            return Ok(CompiledDependency {
                name: dep.name.clone(),
                version: dep.version.clone(),
                lib_name: lib_name.clone(),
                rlib_path: artifact_path,
                dependencies: unit
                    .lib_deps
                    .iter()
                    .map(|edge| edge.unit.pkg.key())
                    .collect(),
                is_proc_macro,
                is_host: matches!(unit.id.kind, UnitKind::Host),
                target: unit_triple.to_string(),
                build_script_output: build_output,
                links_name: Self::detect_links(dep),
                fingerprint,
                checksum: dep.checksum.clone(),
            });
        }

        let build_output = self.run_build_script(
            pkg,
            &out_dir,
            &unit.build_deps,
            &unit.all_deps,
            compiled,
            unit.id.kind,
            target_triple,
            profile,
            &features,
        )?;

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&(&dep.name, &dep.version, unit.id.kind, &features), &mut hasher);
        let metadata = std::hash::Hasher::finish(&hasher);

        let mut cmd = Command::new("rustc");
        cmd.arg("--crate-name")
            .arg(&lib_name)
            .arg("--crate-type")
            .arg(&pkg.crate_type)
            .arg("--edition")
            .arg(&pkg.edition)
            .arg("--cap-lints")
            .arg("allow")
            .arg("-C")
            .arg(format!("metadata={:016x}", metadata))
            .arg(dep.path.join(&pkg.lib_path))
            .arg("-o")
            .arg(&artifact_path)
            .arg("-L")
            .arg(&output_dir);

        if is_proc_macro {
            // proc_macro is a sysroot crate; rustc resolves it when given a bare --extern.
            cmd.arg("--extern").arg("proc_macro");
        } else {
            cmd.arg("--target").arg(unit_triple);
        }

        for feature in &features {
            cmd.arg("--cfg").arg(format!("feature=\"{}\"", feature));
        }

        if let Some(ref output) = build_output {
            for cfg in &output.cfgs {
                cmd.arg("--cfg").arg(cfg);
            }
        }

        // Direct dependencies only. Indirect deps are discovered through rlib metadata + -L.
        for edge in &unit.lib_deps {
            let dep_unit = &edge.unit;
            let compiled_dep = compiled.get(dep_unit).with_context(|| {
                format!(
                    "Missing compiled dependency {} for {}",
                    dep_unit.key(),
                    unit.id.key()
                )
            })?;
            cmd.arg("--extern")
                .arg(format!("{}={}", edge.extern_name, compiled_dep.rlib_path.display()));
        }

        // Search paths for transitive deps and build-script native libs.
        let mut search_paths = HashSet::new();
        for compiled_dep in compiled.values() {
            if let Some(parent) = compiled_dep.rlib_path.parent() {
                search_paths.insert(parent.to_path_buf());
            }
        }
        if let Some(ref output) = build_output {
            for path in &output.link_search_paths {
                search_paths.insert(path.clone());
            }
        }
        for path in search_paths {
            cmd.arg("-L").arg(path);
        }

        if let Some(ref output) = build_output {
            for lib in &output.link_libs {
                cmd.arg("-l").arg(lib);
            }
            for flag in &output.rustc_flags {
                cmd.arg(flag);
            }
        }

        set_rustc_env(&mut cmd, unit_triple, &self.host_triple, profile, dep);
        cmd.env("OUT_DIR", &out_dir_abs);
        if let Some(ref output) = build_output {
            for (key, value) in &output.env {
                cmd.env(key, value);
            }
        }

        debug!("Running: {:?}", cmd);
        let output = cmd
            .output()
            .with_context(|| format!("Failed to execute rustc for {}@{}", dep.name, dep.version))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            anyhow::bail!(
                "rustc failed for {}@{} ({}):\n{}{}",
                dep.name,
                dep.version,
                unit.id.key(),
                stderr,
                stdout
            );
        }

        let _ = fs::write(&fingerprint_path, &fingerprint);
        if let Ok(json) = serde_json::to_string(&build_output) {
            let _ = fs::write(&cached_build_output_path, json);
        }

        debug!(
            "Compiled {} {} ({} {})",
            dep.name,
            dep.version,
            unit.id.key(),
            unit_triple
        );

        Ok(CompiledDependency {
            name: dep.name.clone(),
            version: dep.version.clone(),
            lib_name,
            rlib_path: artifact_path,
            dependencies: unit
                .lib_deps
                .iter()
                .map(|edge| edge.unit.pkg.key())
                .collect(),
            is_proc_macro,
            is_host: matches!(unit.id.kind, UnitKind::Host),
            target: unit_triple.to_string(),
            build_script_output: build_output,
            links_name: Self::detect_links(dep),
            fingerprint,
            checksum: dep.checksum.clone(),
        })
    }

    fn enabled_features(&self, state: &ResolveState, id: &PackageId) -> Vec<String> {
        let requested = state.requested.get(id).cloned().unwrap_or_default();
        let node = match state.packages.get(id) {
            Some(node) => node,
            None => return Vec::new(),
        };
        let defs = node.feature_defs.clone().unwrap_or_default();
        let optional_names: Vec<String> = node
            .raw_deps
            .iter()
            .filter(|dep| dep.optional)
            .map(|dep| dep.extern_name.clone())
            .collect();

        let mut features: Vec<String> = requested
            .into_iter()
            .filter(|feature| {
                defs.contains_key(feature)
                    || optional_names.iter().any(|name| dep_name_matches(name, feature))
            })
            .collect();

        // Activated optional dependencies create an implicit feature of the same
        // name (unless `dep:` syntax suppresses it), which rustc sees as a cfg.
        let dep_syntax_used: HashSet<String> = defs
            .values()
            .flatten()
            .filter_map(|value| value.trim().strip_prefix("dep:").map(str::to_string))
            .collect();
        for dep in &node.raw_deps {
            if dep.optional
                && dep.resolved.is_some()
                && !dep_syntax_used
                    .iter()
                    .any(|name| raw_dep_name_matches(dep, name))
                && !features.iter().any(|f| dep_name_matches(f, &dep.extern_name))
            {
                features.push(dep.extern_name.clone());
            }
        }

        features.sort();
        features.dedup();
        features
    }

    fn run_build_script(
        &self,
        pkg: &PackageNode,
        out_dir: &Path,
        build_deps: &[DepEdge],
        all_dep_units: &[UnitId],
        compiled: &HashMap<UnitId, CompiledDependency>,
        unit_kind: UnitKind,
        target_triple: &str,
        profile: &str,
        features: &[String],
    ) -> Result<Option<BuildOutput>> {
        let dep = &pkg.dep;
        let build_script_path = match Self::detect_build_script(dep) {
            Some(path) => path,
            None => return Ok(None),
        };

        fs::create_dir_all(out_dir)?;
        let out_dir = fs::canonicalize(out_dir)?;
        let script_out = out_dir.join("build_script_bin");
        #[cfg(windows)]
        let script_bin = script_out.with_extension("exe");
        #[cfg(not(windows))]
        let script_bin = script_out;

        let unit_triple = match unit_kind {
            UnitKind::Host => self.host_triple.as_str(),
            UnitKind::Target => target_triple,
        };

        let mut compile_cmd = Command::new("rustc");
        compile_cmd
            .arg("--crate-name")
            .arg(format!("build_script_{}", dep.name.replace('-', "_")))
            .arg("--edition")
            .arg(&pkg.edition)
            .arg("--cap-lints")
            .arg("allow")
            .arg(&build_script_path)
            .arg("-o")
            .arg(&script_bin)
            .arg("-L")
            .arg(self.build_dir.join("deps"));

        for feature in features {
            compile_cmd.arg("--cfg").arg(format!("feature=\"{}\"", feature));
        }

        for edge in build_deps {
            if let Some(compiled_dep) = compiled.get(&edge.unit) {
                compile_cmd.arg("--extern").arg(format!(
                    "{}={}",
                    edge.extern_name,
                    compiled_dep.rlib_path.display()
                ));
            }
        }

        // Search paths for transitive deps of build dependencies.
        let mut search_paths = HashSet::new();
        for compiled_dep in compiled.values() {
            if let Some(parent) = compiled_dep.rlib_path.parent() {
                search_paths.insert(parent.to_path_buf());
            }
        }
        for path in search_paths {
            compile_cmd.arg("-L").arg(path);
        }

        set_rustc_env(&mut compile_cmd, unit_triple, &self.host_triple, profile, dep);
        compile_cmd.env("OUT_DIR", &out_dir);

        let compile_output = compile_cmd.output().with_context(|| {
            format!("Failed to compile build script for {}@{}", dep.name, dep.version)
        })?;
        if !compile_output.status.success() {
            let stderr = String::from_utf8_lossy(&compile_output.stderr);
            anyhow::bail!(
                "Build script compilation failed for {}@{}:\n{}",
                dep.name,
                dep.version,
                stderr
            );
        }

        let mut run_cmd = Command::new(&script_bin);
        run_cmd.current_dir(&dep.path);
        set_rustc_env(&mut run_cmd, unit_triple, &self.host_triple, profile, dep);
        run_cmd
            .env("OUT_DIR", out_dir)
            .env("RUSTC", "rustc")
            .env("RUSTDOC", "rustdoc")
            .env("CARGO", "cargo")
            .env("CARGO_ENCODED_RUSTFLAGS", "")
            .env("CARGO_CFG_TARGET_FEATURE", "");

        // Propagate `links` metadata from direct dependencies as
        // DEP_<LINKS>_<KEY> env vars, like Cargo does.
        for dep_unit in all_dep_units {
            let Some(compiled_dep) = compiled.get(dep_unit) else {
                continue;
            };
            let (Some(links), Some(output)) =
                (&compiled_dep.links_name, &compiled_dep.build_script_output)
            else {
                continue;
            };
            let prefix = format!(
                "DEP_{}_",
                links.to_uppercase().replace('-', "_")
            );
            for (key, value) in &output.metadata {
                run_cmd.env(
                    format!("{}{}", prefix, key.to_uppercase().replace('-', "_")),
                    value,
                );
            }
        }

        let output = run_cmd.output().with_context(|| {
            format!("Failed to run build script for {}@{}", dep.name, dep.version)
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            anyhow::bail!(
                "Build script failed for {}@{}:\n{}{}",
                dep.name,
                dep.version,
                stderr,
                stdout
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(Some(BuildOutput::parse(&stdout)))
    }

    fn detect_build_script(dep: &DownloadedDependency) -> Option<PathBuf> {
        let default = dep.path.join("build.rs");
        if default.exists() {
            return Some(default);
        }

        let cargo_toml = dep.path.join("Cargo.toml");
        if let Ok(content) = fs::read_to_string(&cargo_toml) {
            for line in content.lines() {
                let line = line.trim();
                if line.starts_with("build") && line.contains('=') {
                    if let Some(value) = line.split('=').nth(1) {
                        let value = value.trim().trim_matches('"');
                        let path = dep.path.join(value);
                        if path.exists() {
                            return Some(path);
                        }
                    }
                }
            }
        }
        None
    }

    fn detect_crate_type(dep: &DownloadedDependency) -> String {
        let cargo_toml = dep.path.join("Cargo.toml");
        if let Ok(content) = fs::read_to_string(&cargo_toml) {
            // Check for proc-macro = true
            if content.contains("proc-macro = true") || content.contains("proc_macro = true") {
                return "proc-macro".to_string();
            }
            // Check for crate-type
            if content.contains("crate-type") {
                if content.contains("cdylib") {
                    return "cdylib".to_string();
                }
                if content.contains("staticlib") {
                    return "staticlib".to_string();
                }
            }
        }
        "lib".to_string()
    }

    fn detect_edition(dep: &DownloadedDependency) -> String {
        let cargo_toml = dep.path.join("Cargo.toml");
        if let Ok(content) = fs::read_to_string(&cargo_toml) {
            for line in content.lines() {
                let line = line.trim();
                if line.starts_with("edition") && line.contains('=') {
                    if let Some(value) = line.split('=').nth(1) {
                        let value = value.trim().trim_matches('"');
                        if matches!(value, "2015" | "2018" | "2021" | "2024") {
                            return value.to_string();
                        }
                    }
                }
            }
        }
        "2015".to_string()
    }

    /// Read `links = "..."` from `[package]` in Cargo.toml.
    fn detect_links(dep: &DownloadedDependency) -> Option<String> {
        let cargo_toml = dep.path.join("Cargo.toml");
        let content = fs::read_to_string(&cargo_toml).ok()?;
        let mut in_package = false;
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_package = line == "[package]";
                continue;
            }
            if in_package && line.starts_with("links") && line.contains('=') {
                if let Some(value) = line.split('=').nth(1) {
                    let value = value.trim().trim_matches('"').trim_matches('\'');
                    if !value.is_empty() {
                        return Some(value.to_string());
                    }
                }
            }
        }
        None
    }

    /// Map registry package names to the extern crate names used by this
    /// manifest, honoring `package = "..."` renames like Cargo does.
    /// e.g. `[dependencies] ffi = { package = "openssl-sys" }` maps
    /// `openssl-sys` -> `ffi`.
    fn manifest_extern_names(dep_path: &Path) -> HashMap<String, String> {
        let cargo_toml = dep_path.join("Cargo.toml");
        let Ok(content) = fs::read_to_string(&cargo_toml) else {
            return HashMap::new();
        };

        // Section state: whether we're inside a deps table, and for
        // `[dependencies.NAME]` style sections the dep key being defined.
        #[derive(PartialEq)]
        enum Section {
            DepsTable,
            PerDep(String),
            Skip,
            Other,
        }

        fn classify(inner: &str) -> Section {
            let s = inner.strip_prefix("target.").unwrap_or(inner);
            for marker in [".build-dependencies", ".dependencies"] {
                if let Some(pos) = s.find(marker) {
                    let after = &s[pos + marker.len()..];
                    if after.is_empty() {
                        return Section::DepsTable;
                    }
                    if let Some(name) = after.strip_prefix('.') {
                        if !name.is_empty() && !name.contains('.') {
                            return Section::PerDep(name.trim_matches('"').trim_matches('\'').to_string());
                        }
                    }
                    return Section::Other;
                }
            }
            for marker in [".dev-dependencies"] {
                if s.find(marker).is_some() {
                    return Section::Skip;
                }
            }
            if s == "dependencies" || s == "build-dependencies" {
                return Section::DepsTable;
            }
            if s == "dev-dependencies" || s.starts_with("dev-dependencies.") {
                return Section::Skip;
            }
            for prefix in ["dependencies.", "build-dependencies."] {
                if let Some(name) = s.strip_prefix(prefix) {
                    if !name.is_empty() && !name.contains('.') {
                        return Section::PerDep(name.trim_matches('"').trim_matches('\'').to_string());
                    }
                }
            }
            Section::Other
        }

        fn extract_package_rename(value: &str) -> Option<String> {
            let bytes = value.as_bytes();
            let mut idx = 0;
            while let Some(pos) = value[idx..].find("package") {
                let start = idx + pos;
                let before_ok = start == 0
                    || (!bytes[start - 1].is_ascii_alphanumeric()
                        && bytes[start - 1] != b'_'
                        && bytes[start - 1] != b'-');
                let mut j = start + "package".len();
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                    j += 1;
                }
                if before_ok && j < bytes.len() && bytes[j] == b'=' {
                    j += 1;
                    while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                        j += 1;
                    }
                    if j < bytes.len() && (bytes[j] == b'"' || bytes[j] == b'\'') {
                        let quote = bytes[j] as char;
                        j += 1;
                        let end = value[j..]
                            .find(quote)
                            .map(|e| j + e)
                            .unwrap_or(value.len());
                        return Some(value[j..end].to_string());
                    }
                }
                idx = start + 1;
            }
            None
        }

        let mut map = HashMap::new();
        let mut section = Section::Other;
        let mut per_dep_package: Option<String> = None;

        for raw_line in content.lines() {
            let line = raw_line.trim();
            if line.starts_with('[') {
                // Flush pending per-dep section.
                if let Section::PerDep(ref key) = section {
                    let registry = per_dep_package
                        .take()
                        .unwrap_or_else(|| key.clone());
                    map.insert(registry, key.replace('-', "_"));
                } else {
                    per_dep_package = None;
                }
                let inner = line
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .trim()
                    .to_string();
                section = classify(&inner);
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            match &section {
                Section::DepsTable => {
                    let Some((key, value)) = line.split_once('=') else {
                        continue;
                    };
                    let key = key.trim().trim_matches('"').trim_matches('\'');
                    if key.is_empty() || key.contains('.') {
                        continue;
                    }
                    let registry = extract_package_rename(value)
                        .unwrap_or_else(|| key.to_string());
                    map.insert(registry, key.replace('-', "_"));
                }
                Section::PerDep(_) => {
                    if let Some(value) = line
                        .strip_prefix("package")
                        .and_then(|rest| rest.trim_start().strip_prefix('='))
                    {
                        let value = value.trim().trim_matches('"').trim_matches('\'');
                        if !value.is_empty() {
                            per_dep_package = Some(value.to_string());
                        }
                    }
                }
                _ => {}
            }
        }

        if let Section::PerDep(ref key) = section {
            let registry = per_dep_package.unwrap_or_else(|| key.clone());
            map.insert(registry, key.replace('-', "_"));
        }

        map
    }

    /// Resolve the lib target path like Cargo does: `[lib] path = "..."`
    /// when present, otherwise the conventional `src/lib.rs`.
    fn detect_lib_path(dep: &DownloadedDependency) -> String {
        let cargo_toml = dep.path.join("Cargo.toml");
        if let Ok(content) = fs::read_to_string(&cargo_toml) {
            let mut in_lib = false;
            for line in content.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    in_lib = line == "[lib]";
                    continue;
                }
                if in_lib && line.starts_with("path") && line.contains('=') {
                    if let Some(value) = line.split('=').nth(1) {
                        let value = value.trim().trim_matches('"').trim_matches('\'');
                        if !value.is_empty() {
                            return value.to_string();
                        }
                    }
                }
            }
        }
        "src/lib.rs".to_string()
    }

    fn detect_lib_name(dep: &DownloadedDependency) -> String {
        let cargo_toml = dep.path.join("Cargo.toml");
        if let Ok(content) = fs::read_to_string(&cargo_toml) {
            let mut in_lib = false;
            for line in content.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    in_lib = line == "[lib]";
                    continue;
                }
                if in_lib && line.starts_with("name") && line.contains('=') {
                    if let Some(value) = line.split('=').nth(1) {
                        let value = value.trim().trim_matches('"');
                        if !value.is_empty() {
                            return value.to_string();
                        }
                    }
                }
            }
        }
        dep.name.replace('-', "_")
    }

    fn parse_cargo_features(dep_path: &Path) -> Result<HashMap<String, Vec<String>>> {
        let cargo_toml = dep_path.join("Cargo.toml");
        let content = fs::read_to_string(&cargo_toml)
            .with_context(|| format!("Failed to read {}", cargo_toml.display()))?;

        let mut in_features = false;
        let mut section = String::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                if in_features {
                    break;
                }
                in_features = trimmed == "[features]";
                continue;
            }
            if in_features {
                section.push_str(line);
                section.push('\n');
            }
        }

        let mut features = HashMap::new();
        let entry_re = regex::Regex::new(r#"(?s)([A-Za-z0-9_-]+)\s*=\s*\[([^\]]*)\]"#)?;
        let string_re = regex::Regex::new(r#""([^"]+)""#)?;

        for captures in entry_re.captures_iter(&section) {
            let name = captures.get(1).unwrap().as_str().to_string();
            let values_raw = captures.get(2).unwrap().as_str();
            let values = string_re
                .captures_iter(values_raw)
                .map(|value| value.get(1).unwrap().as_str().to_string())
                .collect();
            features.insert(name, values);
        }

        Ok(features)
    }

    /// Get --extern flags for a target
    pub fn get_extern_flags(&self, compiled_deps: &HashMap<String, CompiledDependency>) -> Vec<String> {
        Self::get_extern_flags_static(compiled_deps)
    }

    /// Get --extern flags for a target (static version)
    pub fn get_extern_flags_static(compiled_deps: &HashMap<String, CompiledDependency>) -> Vec<String> {
        let mut flags = Vec::new();

        for dep in compiled_deps.values() {
            flags.push("--extern".to_string());
            flags.push(format!("{}={}", dep.lib_name, dep.rlib_path.display()));
        }

        flags
    }

    /// Get library search paths
    pub fn get_library_paths(&self, compiled_deps: &HashMap<String, CompiledDependency>) -> Vec<String> {
        let mut paths = Vec::new();

        for dep in compiled_deps.values() {
            if let Some(parent) = dep.rlib_path.parent() {
                paths.push("-L".to_string());
                paths.push(parent.display().to_string());
            }
        }

        paths
    }
}

fn request_feature(state: &mut ResolveState, id: PackageId, feature: &str) {
    if feature.is_empty() {
        return;
    }
    if state
        .expanded
        .get(&id)
        .map(|features| features.contains(feature))
        .unwrap_or(false)
    {
        return;
    }
    let requested = state.requested.entry(id.clone()).or_default();
    if requested.insert(feature.to_string()) {
        state.pending.push_back((id, feature.to_string()));
    }
}

fn seed_edge_features(state: &mut ResolveState, child: PackageId, raw: &RawDep) {
    for feature in &raw.features {
        request_feature(state, child.clone(), feature);
    }
    if raw.default_features {
        request_feature(state, child, "default");
    }
}

fn raw_dep_name_matches(dep: &RawDep, name: &str) -> bool {
    dep_name_matches(&dep.name, name) || dep_name_matches(&dep.extern_name, name)
}

fn dep_name_matches(raw_name: &str, feature_name: &str) -> bool {
    raw_name == feature_name
        || raw_name.replace('-', "_") == feature_name
        || raw_name.replace('_', "-") == feature_name
}

fn set_rustc_env(
    cmd: &mut Command,
    target_triple: &str,
    host_triple: &str,
    profile: &str,
    dep: &DownloadedDependency,
) {
    let (opt_level, debug) = if profile == "release" {
        ("3", "false")
    } else {
        ("0", "true")
    };

    cmd.env("TARGET", target_triple)
        .env("HOST", host_triple)
        .env("PROFILE", profile)
        .env("OPT_LEVEL", opt_level)
        .env("DEBUG", debug)
        .env("NUM_JOBS", std::thread::available_parallelism().map(|n| n.to_string()).unwrap_or_else(|_| "1".to_string()))
        .env("CARGO_PKG_NAME", &dep.name)
        .env("CARGO_PKG_VERSION", &dep.version)
        .env("CARGO_PKG_VERSION_MAJOR", dep.version.split('.').next().unwrap_or("0"))
        .env("CARGO_PKG_VERSION_MINOR", dep.version.split('.').nth(1).unwrap_or("0"))
        .env("CARGO_PKG_VERSION_PATCH", dep.version.split('.').nth(2).unwrap_or("0"))
        .env("CARGO_PKG_VERSION_PRE", "")
        .env("CARGO_MANIFEST_DIR", &dep.path);

    let info = TargetInfo::from_triple(target_triple);
    cmd.env("CARGO_CFG_TARGET_OS", &info.os)
        .env("CARGO_CFG_TARGET_FAMILY", &info.family)
        .env("CARGO_CFG_TARGET_ARCH", &info.arch)
        .env("CARGO_CFG_TARGET_ENV", &info.env)
        .env("CARGO_CFG_TARGET_VENDOR", &info.vendor)
        .env("CARGO_CFG_TARGET_ENDIAN", &info.endian)
        .env("CARGO_CFG_TARGET_POINTER_WIDTH", info.pointer_width.to_string());
}

struct TargetInfo {
    os: String,
    family: String,
    arch: String,
    env: String,
    vendor: String,
    endian: String,
    pointer_width: u32,
}

impl TargetInfo {
    fn from_triple(triple: &str) -> Self {
        let parts: Vec<&str> = triple.split('-').collect();
        let arch = parts.first().unwrap_or(&"").to_string();
        let vendor = parts.get(1).unwrap_or(&"").to_string();
        let os = parts.get(2).unwrap_or(&"").to_string();
        let env = parts.get(3).unwrap_or(&"").to_string();
        let family = if os == "windows" {
            "windows".to_string()
        } else if matches!(os.as_str(), "linux" | "macos" | "ios" | "android" | "freebsd" | "openbsd" | "netbsd" | "dragonfly" | "solaris" | "illumos" | "haiku" | "redox" | "fuchsia" | "vxworks" | "horizon" | "vita" | "psp" | "ps4" | "ps5" | "xous") {
            "unix".to_string()
        } else if os.contains("wasi") || arch.contains("wasm") {
            "wasm".to_string()
        } else {
            "unknown".to_string()
        };
        let pointer_width = if arch.contains("64") || matches!(arch.as_str(), "x86_64" | "aarch64" | "arm64") {
            64
        } else if matches!(arch.as_str(), "x86" | "i386" | "i686" | "arm" | "armv7" | "thumbv7") {
            32
        } else {
            64
        };
        Self {
            os,
            family,
            arch,
            env,
            vendor,
            endian: "little".to_string(),
            pointer_width,
        }
    }
}

fn target_expr_matches(expr: &str, target_triple: &str) -> bool {
    let expr = expr.trim();
    if expr.is_empty() {
        return true;
    }

    // Exact target triples are only compatible with themselves.
    if expr.contains('-') && !expr.contains('(') && !expr.contains('=') {
        return expr == target_triple;
    }

    let info = TargetInfo::from_triple(target_triple);
    let mut parser = CfgParser {
        input: expr,
        pos: 0,
        info: &info,
    };
    parser.parse_expr().unwrap_or(false)
}

struct CfgParser<'a> {
    input: &'a str,
    pos: usize,
    info: &'a TargetInfo,
}

impl<'a> CfgParser<'a> {
    fn parse_expr(&mut self) -> Result<bool, ()> {
        self.skip_ws();
        if self.consume("any") {
            return self.parse_list(false);
        }
        if self.consume("all") {
            return self.parse_list(true);
        }
        if self.consume("not") {
            self.skip_ws();
            self.expect('(')?;
            let value = self.parse_expr()?;
            self.skip_ws();
            self.expect(')')?;
            return Ok(!value);
        }
        if self.consume("cfg") {
            self.skip_ws();
            self.expect('(')?;
            let value = self.parse_expr()?;
            self.skip_ws();
            self.expect(')')?;
            return Ok(value);
        }
        self.parse_atom()
    }

    fn parse_list(&mut self, all: bool) -> Result<bool, ()> {
        self.skip_ws();
        self.expect('(')?;
        let mut result = all;
        loop {
            self.skip_ws();
            if self.peek() == Some(')') {
                self.pos += 1;
                break;
            }
            let value = self.parse_expr()?;
            result = if all { result && value } else { result || value };
            self.skip_ws();
            if self.peek() == Some(',') {
                self.pos += 1;
                continue;
            }
            if self.peek() == Some(')') {
                self.pos += 1;
                break;
            }
            return Err(());
        }
        Ok(result)
    }

    fn parse_atom(&mut self) -> Result<bool, ()> {
        self.skip_ws();
        if self.peek() == Some('(') {
            self.pos += 1;
            let value = self.parse_expr()?;
            self.skip_ws();
            self.expect(')')?;
            return Ok(value);
        }

        let ident = self.parse_ident()?;
        self.skip_ws();
        let mut value = None;
        if self.peek() == Some('=') {
            self.pos += 1;
            self.skip_ws();
            value = Some(self.parse_string()?);
        }
        Ok(self.eval_atom(&ident, value.as_deref()))
    }

    fn eval_atom(&self, ident: &str, value: Option<&str>) -> bool {
        match (ident, value) {
            ("windows", None) => self.info.os == "windows",
            ("unix", None) => self.info.family == "unix",
            ("target_os", Some(v)) => self.info.os == v,
            ("target_family", Some(v)) => self.info.family == v,
            ("target_arch", Some(v)) => self.info.arch == v,
            ("target_env", Some(v)) => self.info.env == v,
            ("target_vendor", Some(v)) => self.info.vendor == v,
            ("target_endian", Some(v)) => self.info.endian == v,
            ("target_pointer_width", Some(v)) => self.info.pointer_width.to_string() == v,
            ("target_has_atomic", Some(v)) => matches!(v, "8" | "16" | "32" | "64" | "ptr") && self.info.pointer_width >= v.parse::<u32>().unwrap_or(64),
            ("target_feature", Some("sse2")) => matches!(self.info.arch.as_str(), "x86" | "x86_64" | "i686" | "i586"),
            ("target_feature", _) => false,
            _ => false,
        }
    }

    fn consume(&mut self, word: &str) -> bool {
        let rest = &self.input[self.pos..];
        if rest.starts_with(word) {
            let after = rest[word.len()..].trim_start();
            if after.starts_with('(') {
                self.pos += word.len();
                return true;
            }
        }
        false
    }

    fn parse_ident(&mut self) -> Result<String, ()> {
        self.skip_ws();
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input[self.pos..].chars().next().unwrap();
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                self.pos += ch.len_utf8();
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(());
        }
        Ok(self.input[start..self.pos].to_string())
    }

    fn parse_string(&mut self) -> Result<String, ()> {
        self.skip_ws();
        if self.peek() != Some('"') {
            return Err(());
        }
        self.pos += 1;
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input[self.pos..].chars().next().unwrap();
            self.pos += ch.len_utf8();
            if ch == '"' {
                return Ok(self.input[start..self.pos - 1].to_string());
            }
        }
        Err(())
    }

    fn expect(&mut self, ch: char) -> Result<(), ()> {
        self.skip_ws();
        if self.peek() == Some(ch) {
            self.pos += ch.len_utf8();
            Ok(())
        } else {
            Err(())
        }
    }

    fn peek(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn skip_ws(&mut self) {
        while self.pos < self.input.len() {
            let ch = self.input[self.pos..].chars().next().unwrap();
            if ch.is_whitespace() {
                self.pos += ch.len_utf8();
            } else {
                break;
            }
        }
    }
}
