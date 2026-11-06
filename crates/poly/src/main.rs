//! Poly CLI entry point

mod crates_io;
mod extras;
mod migrate;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use poly_config::parse_poly_ini;
use poly_linker::{CBuild, GoBuild, LinkUnit, RustBuild};
use poly_resolver::{DependencyResolver, LockEntry, LockFile};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};
use tracing_subscriber;

#[derive(Parser)]
#[command(name = "poly")]
#[command(about = "Poly: one build system for Rust, C/C++ and Go", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new Poly project
    Init {
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Build the project
    Build {
        #[arg(short, long)]
        target: Option<String>,
        #[arg(short, long)]
        release: bool,
        #[arg(short, long)]
        config: Option<String>,
        /// Skip creating the combined .vfsx.tar.zst package after building
        #[arg(long)]
        no_package: bool,
        /// Show what is being rebuilt (spec §13.3)
        #[arg(long)]
        verbose: bool,
    },
    /// Add a dependency
    Add {
        dependency: String,
    },
    /// Remove a dependency
    Remove {
        dependency: String,
    },
    /// Update dependencies
    Update {
        #[arg(short, long)]
        all: bool,
    },
    /// Generate lock file
    Lock {
        #[arg(short, long)]
        config: Option<String>,
    },
    /// Clean build artifacts
    Clean,
    /// Validate configuration
    Validate,
    /// Sign the current project
    Sign {
        #[arg(short, long)]
        keyfile: Option<String>,
    },
    /// Verify package signatures
    Verify {
        #[arg(short, long)]
        package: Option<String>,
    },
    /// Uninstall a package
    Uninstall {
        package: String,
    },
    /// Run tests across all languages
    Test {
        target: Option<String>,
        #[arg(long)]
        coverage: bool,
        #[arg(long)]
        benchmark: bool,
    },
    /// Run linters across all languages
    Lint {
        target: Option<String>,
    },
    /// Format all sources
    Fmt,
    /// Generate documentation
    Doc {
        #[arg(long)]
        open: bool,
        #[arg(long)]
        serve: bool,
    },
    /// Version management
    Version {
        #[arg(short, long)]
        bump: Option<String>,
        #[arg(long)]
        check: bool,
        #[arg(long)]
        changelog: bool,
    },
    /// Generate CI configuration
    Ci {
        provider: String,
    },
    /// Migrate from another build system
    Migrate {
        from: String,
    },
    /// Generate shell completions
    Completion {
        shell: String,
    },
    /// Manage trusted signing keys
    Trust {
        #[arg(short, long)]
        add: Option<String>,
        #[arg(short, long)]
        list: bool,
        #[arg(short, long)]
        remove: Option<String>,
    },
    /// Cache management
    Cache {
        #[arg(short, long)]
        stats: bool,
    },
    /// Show outdated dependencies
    Outdated,
    /// Show dependency tree
    Tree,
    /// Toolchain management
    Toolchain {
        #[arg(short, long)]
        list: bool,
        #[arg(short, long)]
        install: Option<String>,
        #[arg(short, long)]
        use_: Option<String>,
    },
    /// Security audit
    Audit {
        #[arg(long)]
        fix: bool,
    },
    /// Generate SBOM
    Sbom,
    /// Profile a build
    Profile,
    /// IDE integration
    Ide {
        kind: String,
    },
    /// Build a release
    Release {
        #[arg(long)]
        dry_run: bool,
    },
    /// Review code (lint + optional tests)
    Review {
        #[arg(long)]
        strict: bool,
    },
    /// Show git diff
    Diff,
    /// Generate a flamegraph
    Flamegraph,
    /// Registry statistics
    Stats,
    /// Log in to a registry (stores API token locally)
    Login {
        /// Registry name (default: crates-io)
        registry: Option<String>,
        /// Provide the token on the command line instead of being prompted
        #[arg(long)]
        token: Option<String>,
    },
    /// Log out of a registry (removes stored API token)
    Logout {
        /// Registry name (default: crates-io)
        registry: Option<String>,
    },
    /// Virtual environments
    Env {
        args: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing. Default to INFO; --verbose or RUST_LOG raises it.
    let verbose = matches!(cli.command, Commands::Build { verbose: true, .. });
    tracing_subscriber::fmt()
        .with_env_filter({
            let rust_log = std::env::var("RUST_LOG").unwrap_or_default();
            if verbose {
                tracing_subscriber::EnvFilter::new("debug")
            } else if rust_log.trim().is_empty() {
                tracing_subscriber::EnvFilter::new("info")
            } else {
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
            }
        })
        .init();

    match cli.command {
        Commands::Init { name } => cmd_init(name),
        Commands::Build { target, release, config, no_package, verbose: _ } => {
            cmd_build(target, release, config, no_package).await
        }
        Commands::Add { dependency } => cmd_add(dependency),
        Commands::Remove { dependency } => cmd_remove(dependency),
        Commands::Update { all } => cmd_update(all),
        Commands::Lock { config } => cmd_lock(config),
        Commands::Clean => cmd_clean(),
        Commands::Validate => cmd_validate(),
        Commands::Sign { keyfile } => cmd_sign(keyfile),
        Commands::Verify { package } => cmd_verify(package),
        Commands::Uninstall { package } => cmd_uninstall(package).await,
        Commands::Test { target, coverage, benchmark } => {
            extras::cmd_test(target, coverage, benchmark).await
        }
        Commands::Lint { target } => extras::cmd_lint(target).await,
        Commands::Fmt => extras::cmd_fmt(),
        Commands::Doc { open, serve } => extras::cmd_doc(open, serve),
        Commands::Version { bump, check, changelog } => {
            if let Some(level) = bump {
                extras::cmd_version_bump(&level)
            } else if check {
                extras::cmd_version_check()
            } else if changelog {
                extras::cmd_version_changelog()
            } else {
                extras::cmd_version_check()
            }
        }
        Commands::Ci { provider } => extras::cmd_ci(&provider),
        Commands::Migrate { from } => extras::cmd_migrate(&from),
        Commands::Completion { shell } => extras::cmd_completion(&shell),
        Commands::Trust { add, remove, .. } => {
            if let Some(path) = add {
                extras::cmd_trust_add(&path)
            } else if let Some(name) = remove {
                extras::cmd_trust_remove(&name)
            } else {
                extras::cmd_trust_list()
            }
        }
        Commands::Cache { .. } => extras::cmd_cache_stats(),
        Commands::Outdated => extras::cmd_outdated().await,
        Commands::Tree => extras::cmd_tree(),
        Commands::Toolchain { install, use_, .. } => {
            if install.is_some() || use_.is_some() {
                extras::cmd_toolchain("use")
            } else {
                extras::cmd_toolchain("list")
            }
        }
        Commands::Audit { fix } => extras::cmd_audit(fix),
        Commands::Sbom => extras::cmd_sbom(),
        Commands::Profile => extras::cmd_profile(),
        Commands::Ide { kind } => extras::cmd_ide(&kind),
        Commands::Release { dry_run } => extras::cmd_release(dry_run),
        Commands::Review { strict } => extras::cmd_review(strict).await,
        Commands::Diff => extras::cmd_diff(),
        Commands::Flamegraph => extras::cmd_flamegraph(),
        Commands::Stats => extras::cmd_stats(),
        Commands::Login { registry, token } => cmd_login(registry, token),
        Commands::Logout { registry } => cmd_logout(registry),
        Commands::Env { args } => extras::cmd_env(&args),
    }
}

fn cmd_init(name: Option<String>) -> Result<()> {
    let project_name = name.unwrap_or_else(|| "my-poly-project".to_string());

    info!("Initializing Poly project: {}", project_name);

    // Create poly.ini
    let poly_ini = format!(
        r#"[project]
name = {}
version = 0.1.0
edition = 2024

[target.lib]
language = rust
type = lib
sources = src/lib.rs

[build.default]
targets = rust:lib

[dependencies.rust]
"#,
        project_name
    );

    std::fs::write("poly.ini", poly_ini)?;

    // Create src directory
    std::fs::create_dir_all("src")?;

    // Create basic lib.rs
    let lib_rs = r#"pub fn hello() -> &'static str {
    "Hello from Poly!"
}
"#;
    std::fs::write("src/lib.rs", lib_rs)?;

    info!("Project initialized successfully");
    info!("  Created: poly.ini");
    info!("  Created: src/lib.rs");

    Ok(())
}

/// Spec §13.7: resolve `[secrets]` entries (`env:VAR`, `vault:path`, or a
/// literal) and inject them as environment variables for every compiler and
/// tool subprocess spawned during the build.
pub(crate) fn inject_secrets(config: &poly_config::Config) {
    let Some(secrets) = &config.secrets else {
        return;
    };
    for (key, spec) in secrets {
        let value = if let Some(var) = spec.strip_prefix("env:") {
            match std::env::var(var) {
                Ok(v) => Some(v),
                Err(_) => {
                    warn!("secret '{}': env var {} is not set", key, var);
                    None
                }
            }
        } else if let Some(path) = spec.strip_prefix("vault:") {
            let vault = std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".config/poly/vault").join(path))
                .unwrap_or_else(|_| PathBuf::from(path));
            match std::fs::read_to_string(&vault) {
                Ok(v) => Some(v.trim().to_string()),
                Err(e) => {
                    warn!("secret '{}': cannot read vault file {}: {}", key, vault.display(), e);
                    None
                }
            }
        } else {
            Some(spec.clone())
        };
        if let Some(v) = value {
            // SAFETY: runs once during command dispatch, before any build
            // worker threads exist; env mutation is process-global.
            unsafe { std::env::set_var(key, v) };
        }
    }
}

async fn cmd_build(target: Option<String>, release: bool, config_path: Option<String>, no_package: bool) -> Result<()> {
    info!("Building project");

    // Load configuration
    let config_file = config_path.unwrap_or_else(|| "poly.ini".to_string());
    let config = parse_poly_ini(&config_file)?;
    inject_secrets(&config);
    debug!("Loaded config for project: {}", config.project.name);

    // Run pre-build hook if configured
    if let Some(hooks) = &config.hooks {
        if let Some(hook) = &hooks.pre_build {
            info!("Running pre-build hook: {}", hook.command);
            let status = Command::new("sh")
                .arg("-c")
                .arg(&hook.command)
                .status()?;
            if !status.success() {
                anyhow::bail!("pre-build hook failed: {}", hook.command);
            }
        }
    }

    // Create output directory: dist/{profile}/
    let profile = if release { "release" } else { "debug" };
    let output_dir = PathBuf::from("dist").join(profile);
    std::fs::create_dir_all(&output_dir)?;

    // Target triples for cross-compilation
    let targets = vec![
        "x86_64-unknown-linux-gnu",
        "x86_64-pc-windows-gnu",
    ];

    // Resolve dependencies (lock entries for c/go; rust versions are
    // resolved for real during dependency compilation below)
    let resolver = DependencyResolver::new();
    let entries = resolver.resolve(
        &config.dependencies.rust,
        &config.dependencies.c,
        &config.dependencies.go,
    )?;

    // Honor versions pinned in an existing poly.lock
    let locked_versions = read_locked_rust_versions();

    // Build each target for each platform
    let mut all_binaries: Vec<(String, PathBuf)> = Vec::new(); // (platform, path)
    let mut rust_lock_entries: Vec<LockEntry> = Vec::new();
    let mut first_platform = true;

    for target_triple in &targets {
        let platform = if target_triple.contains("windows") { "windows" } else { "linux" };
        let platform_dir = output_dir.join(platform);
        std::fs::create_dir_all(&platform_dir)?;
        
        info!("Building for platform: {} ({})", platform, target_triple);

        let compiled_deps = if !config.dependencies.rust.is_empty() {
            info!("Compiling Rust dependencies for {}...", target_triple);
            let dep_compiler = poly_core::DependencyCompiler::new(
                poly_core::project_cache_dir(&config.project.name).join(profile),
            )?;
            let workspace_dir = std::env::current_dir()?;
            let compiled = dep_compiler
                .compile_dependencies(&config.dependencies.rust, &workspace_dir, target_triple, profile, &locked_versions)
                .await?;

            // Record the real resolved versions for poly.lock (once)
            if first_platform {
                let mut seen = HashSet::new();
                for dep in compiled.values() {
                    if seen.insert((dep.name.clone(), dep.version.clone())) {
                        rust_lock_entries.push(LockEntry {
                            language: "rust".to_string(),
                            name: dep.name.clone(),
                            version: dep.version.clone(),
                            checksum: dep.checksum.clone(),
                            source: "https://crates.io".to_string(),
                            signature: None,
                        });
                    }
                }
            }
            compiled
        } else {
            std::collections::HashMap::new()
        };

        let mut compiled_workspace: HashMap<String, PathBuf> = HashMap::new();
        let mut link_units: Vec<LinkUnit> = Vec::new();
        let mut build_targets = parse_build_targets(&config)?;

        // `poly build --target rust:lib` builds a single target (spec §7.4).
        if let Some(spec) = &target {
            build_targets.retain(|id| {
                id == spec
                    || config
                        .targets
                        .get(id)
                        .map(|tc| format!("{}:{}", tc.language, id) == *spec)
                        .unwrap_or(false)
            });
            if build_targets.is_empty() {
                anyhow::bail!("No matching build target: {}", spec);
            }
        }

        // When targets span multiple languages, bin targets defer their final
        // link to the unified step so foreign objects/archives can be merged
        // (spec §8.2: one executable per platform).
        let has_c = build_targets.iter().any(|id| {
            config.targets.get(id).map(|t| matches!(t.language.as_str(), "c" | "cpp")).unwrap_or(false)
        });
        let has_go = build_targets.iter().any(|id| {
            config.targets.get(id).map(|t| t.language == "go").unwrap_or(false)
        });
        let has_rust = build_targets.iter().any(|id| {
            config.targets.get(id).map(|t| t.language == "rust").unwrap_or(false)
        });
        let defer_rust_bin = has_c || has_go;
        let defer_c_bin = has_rust || has_go;

        for target_id in build_targets {
            if let Some(target_config) = config.targets.get(&target_id) {
                match target_config.language.as_str() {
                    "rust" => {
                        if let Some(build) = build_rust_target(
                            &target_id,
                            target_config,
                            &config,
                            &platform_dir,
                            &compiled_deps,
                            &compiled_workspace,
                            target_triple,
                            defer_rust_bin && target_config.type_ == "bin",
                        )? {
                            // Store the compiled artifact for dependents
                            let crate_name = target_id.replace('-', "_");
                            if build.output.exists() {
                                compiled_workspace.insert(crate_name, build.output.clone());
                            }
                            link_units.push(LinkUnit {
                                id: target_id.clone(),
                                language: "rust".to_string(),
                                kind: target_config.type_.clone(),
                                artifact: Some(build.output.clone()),
                                rust: Some(build),
                                c: None,
                                go: None,
                            });
                        }
                    }
                    "c" | "cpp" => {
                        if let Some(build) = build_c_target(
                            &target_id,
                            target_config,
                            &config,
                            &platform_dir,
                            target_triple,
                            defer_c_bin && target_config.type_ == "bin",
                        )? {
                            link_units.push(LinkUnit {
                                id: target_id.clone(),
                                language: target_config.language.clone(),
                                kind: target_config.type_.clone(),
                                artifact: Some(build.output.clone()),
                                rust: None,
                                c: Some(build),
                                go: None,
                            });
                        }
                    }
                    "go" => {
                        if let Some(build) = build_go_target(
                            &target_id,
                            target_config,
                            &config,
                            &platform_dir,
                            target_triple,
                        )? {
                            link_units.push(LinkUnit {
                                id: target_id.clone(),
                                language: "go".to_string(),
                                kind: target_config.type_.clone(),
                                artifact: Some(build.output.clone()),
                                rust: None,
                                c: None,
                                go: Some(build),
                            });
                        }
                    }
                    _ => {
                        info!("Unknown language: {}", target_config.language);
                    }
                }
            }
        }

        // Spec §8.2: link every target produced for this platform into ONE
        // executable via poly-linker.
        if let Some(unified) = poly_linker::link_platform_executable(
            &link_units,
            &platform_dir,
            target_triple,
            &config.project.name,
        )? {
            all_binaries.push((platform.to_string(), unified));
        }
        first_platform = false;
    }

    // Write poly.lock with real resolved versions
    let mut lockfile = LockFile::new();
    for entry in rust_lock_entries {
        lockfile.add_entry(entry);
    }
    for entry in entries {
        if entry.language != "rust" {
            lockfile.add_entry(entry);
        }
    }
    lockfile.write("poly.lock")?;

    // Package all binaries as VFSX archives (one per platform)
    if !all_binaries.is_empty() {
        package_binaries_as_vfsx(&all_binaries, &output_dir, &config.project.name, profile)?;
    }

    // Create the combined installable package unless --no-package
    if !no_package && !all_binaries.is_empty() {
        match create_combined_package(&config, profile).await {
            Ok(pkg) => {
                info!("Created package: {}", pkg.display());
                // The merged archive is the deliverable — drop the per-profile
                // build tree so dist/ only holds the package.
                if let Err(e) = std::fs::remove_dir_all(&output_dir) {
                    warn!("Failed to remove {}: {}", output_dir.display(), e);
                }
            }
            Err(e) => warn!("Packaging failed: {}", e),
        }
    }

    // Run post-build hook if configured
    if let Some(hooks) = &config.hooks {
        if let Some(hook) = &hooks.post_build {
            info!("Running post-build hook: {}", hook.command);
            let status = Command::new("sh")
                .arg("-c")
                .arg(&hook.command)
                .status()?;
            if !status.success() {
                warn!("post-build hook failed: {}", hook.command);
            }
        }
    }

    clean_build_intermediates(&output_dir)?;

    info!("Build completed successfully");

    Ok(())
}

/// Remove intermediate artifacts after a successful build. Deliverables are
/// `bin/` executables and `*.vfsx.tar.zst` packages; rlibs, objects, and
/// `.linklibs` are rebuilt on the next run. Dep artifacts now live in the
/// project cache (`~/.cache/poly/{project}/{profile}/deps`), so any legacy
/// `dist/{profile}/deps` directory is removed as well.
fn clean_build_intermediates(output_dir: &Path) -> Result<()> {
    if !output_dir.is_dir() {
        return Ok(());
    }
    let legacy_deps = output_dir.join("deps");
    if legacy_deps.is_dir() {
        std::fs::remove_dir_all(&legacy_deps)?;
    }

    for platform_entry in std::fs::read_dir(output_dir)? {
        let platform_dir = platform_entry?.path();
        if !platform_dir.is_dir() {
            continue;
        }
        for triple_entry in std::fs::read_dir(&platform_dir)? {
            let triple_dir = triple_entry?.path();
            if !triple_dir.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(&triple_dir)? {
                let entry = entry?;
                if entry.file_name() == "bin" {
                    continue;
                }
                let path = entry.path();
                if path.is_dir() {
                    std::fs::remove_dir_all(&path)?;
                } else {
                    std::fs::remove_file(&path)?;
                }
            }
        }
    }
    Ok(())
}

/// Package built binaries as VFSX archives (one per platform)
/// Create the combined `dist/{name}-{version}.vfsx.tar.zst` package with
/// package.json + poly.lock + per-platform binaries under bin/.
async fn create_combined_package(config: &poly_config::Config, profile: &str) -> Result<PathBuf> {
    let platforms = config
        .publish
        .as_ref()
        .map(|p| p.platforms.clone())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![
            "x86_64-linux".to_string(),
            "x86_64-windows".to_string(),
        ]);
    let include_docs = config
        .publish
        .as_ref()
        .map(|p| p.include_docs)
        .unwrap_or(false);

    let package_config = poly_package::PackageConfig {
        name: config.project.name.clone(),
        version: config.project.version.clone(),
        description: config.project.description.clone(),
        authors: config.project.authors.clone(),
        license: config
            .project
            .license
            .clone()
            .unwrap_or_else(|| "UNLICENSED".to_string()),
        platforms,
        include_docs,
        compression: "zstd".to_string(),
        dependencies: sibling_crates(config),
    };

    let packager = poly_package::Packager::new(
        package_config,
        PathBuf::from("."),
        PathBuf::from("dist"),
        profile.to_string(),
    );
    packager.create_package().await
}

fn package_binaries_as_vfsx(
    binaries: &[(String, PathBuf)], // (platform, path)
    output_dir: &Path,
    project_name: &str,
    profile: &str,
) -> Result<()> {
    info!("Packaging {} binaries as VFSX archives", binaries.len());
    
    let vfsx = poly_vfsx::Vfsx::new(false);
    
    // Group binaries by platform
    let mut by_platform: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for (platform, path) in binaries {
        by_platform.entry(platform.clone()).or_default().push(path.clone());
    }
    
    // Create one VFSX archive per platform
    for (platform, platform_binaries) in by_platform {
        info!("Packaging {} binaries for {}", platform_binaries.len(), platform);
        
        // Create a temporary directory with all binaries for this platform
        let temp_dir = output_dir.join(format!(".vfsx_temp_{}", platform));
        std::fs::create_dir_all(&temp_dir)?;
        
        for binary_path in &platform_binaries {
            let binary_name = binary_path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("binary");
            let binary_in_temp = temp_dir.join(binary_name);
            std::fs::copy(binary_path, &binary_in_temp)?;
        }
        
        // Create VFSX archive for this platform (.vfsx.tar.zst per spec §8.1)
        let vfsx_path = output_dir.join(format!("{}-{}.{}.vfsx.tar.zst", project_name, platform, profile));
        vfsx.create(&temp_dir, &vfsx_path, "zstd")?;
        
        // Clean up temp directory
        let _ = std::fs::remove_dir_all(&temp_dir);
        
        info!("Created VFSX package: {}", vfsx_path.display());
    }
    
    Ok(())
}

/// Parse build targets in dependency order
pub(crate) fn parse_build_targets(config: &poly_config::Config) -> Result<Vec<String>> {
    let default_targets = &config.build.default.targets;
    
    // Build dependency graph for workspace crates
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    
    for target in default_targets {
        // Strip language prefix (e.g., "rust:poly" -> "poly")
        let crate_name = target.split(':').last().unwrap_or(target);
        let deps = config.dependencies.workspace.get(crate_name)
            .map(|deps| deps.keys().cloned().collect())
            .unwrap_or_default();
        graph.insert(crate_name.to_string(), deps);
    }
    
    // Topological sort
    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();
    let mut result = Vec::new();
    
    for target in default_targets {
        let crate_name = target.split(':').last().unwrap_or(target);
        if !visited.contains(crate_name) {
            topo_sort(crate_name, &graph, &mut visited, &mut visiting, &mut result)?;
        }
    }
    
    Ok(result)
}

fn topo_sort(
    node: &str,
    graph: &HashMap<String, Vec<String>>,
    visited: &mut HashSet<String>,
    visiting: &mut HashSet<String>,
    result: &mut Vec<String>,
) -> Result<()> {
    if visited.contains(node) {
        return Ok(());
    }
    
    if visiting.contains(node) {
        anyhow::bail!("Circular dependency detected: {}", node);
    }
    
    visiting.insert(node.to_string());
    
    if let Some(deps) = graph.get(node) {
        for dep in deps {
            topo_sort(dep, graph, visited, visiting, result)?;
        }
    }
    
    visiting.remove(node);
    visited.insert(node.to_string());
    result.push(node.to_string());
    
    Ok(())
}

/// Pick the crate root from expanded sources: `lib.rs` for library targets,
/// `main.rs` for binaries, first source otherwise. rustc compiles a single
/// root file; sibling files are modules reached through `mod` declarations.
pub(crate) fn pick_rust_root(sources: &[PathBuf], type_: &str) -> PathBuf {
    let preferred = if type_ == "bin" { "main.rs" } else { "lib.rs" };
    sources
        .iter()
        .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(preferred))
        .cloned()
        .unwrap_or_else(|| sources[0].clone())
}

fn build_rust_target(
    target_id: &str,
    target_config: &poly_config::Target,
    config: &poly_config::Config,
    output_dir: &Path,
    compiled_deps: &std::collections::HashMap<String, poly_core::CompiledDependency>,
    compiled_workspace: &HashMap<String, PathBuf>,
    target_triple: &str,
    defer_link: bool,
) -> Result<Option<RustBuild>> {
    info!("Building Rust target: {} for {}", target_id, target_triple);

    let sources = expand_globs(&target_config.sources)?;
    if sources.is_empty() {
        info!("No sources found for target {}", target_id);
        return Ok(None);
    }

    // rustc takes a single crate root: lib.rs for libs, main.rs for bins.
    // Other globbed sources are modules reached via `mod` declarations.
    let root = pick_rust_root(&sources, &target_config.type_);

    let driver = poly_rust::RustDriver::new()?;

    // Create Rust target
    let rust_target = poly_rust::RustTarget {
        name: target_id.replace('-', "_"),
        crate_type: match target_config.type_.as_str() {
            "lib" => "lib".to_string(),
            "bin" => "bin".to_string(),
            "static-lib" => "staticlib".to_string(),
            "cdylib" => "cdylib".to_string(),
            _ => "lib".to_string(),
        },
        edition: target_config.edition.clone()
            .or(config.project.edition.clone())
            .unwrap_or_else(|| "2024".to_string()),
        features: target_config.features.clone(),
        dependencies: Vec::new(), // TODO: Resolve from config
        source: root.clone(),
        flags: target_config.flags.clone(),
        target_triple: Some(target_triple.to_string()),
        env: manifest_env(target_id, config, &root),
    };

    // Determine output path (add target triple subdirectory)
    let target_dir = output_dir.join(target_triple);
    std::fs::create_dir_all(&target_dir)?;
    
    let output = match rust_target.crate_type.as_str() {
        "bin" => {
            // Windows binaries need .exe extension
            let ext = if target_triple.contains("windows") { ".exe" } else { "" };
            target_dir.join(format!("{}{}", rust_target.name, ext))
        }
        _ => target_dir.join(format!("lib{}.rlib", rust_target.name)),
    };

    // Build --extern and -L/--cfg flags for this target
    let mut all_flags = rust_target.flags.clone();
    all_flags.extend(rust_dep_args(
        target_id,
        target_config,
        config,
        compiled_deps,
        compiled_workspace,
        target_triple,
    ));

    // Update target with new flags
    let mut rust_target_with_deps = rust_target.clone();
    rust_target_with_deps.flags = all_flags;

    // When foreign-language targets will be merged by poly-linker, the bin's
    // final link is deferred — the unified step performs the only rustc
    // invocation, with foreign archives passed via -C link-args.
    if !(defer_link && rust_target.crate_type == "bin") {
        // Compile - dependencies are passed via --extern flags in target.flags
        driver.compile(
            &root,
            &output,
            &rust_target_with_deps,
            &[], // Empty - we use --extern flags in target.flags
            None, // No build script for MVP
        )?;
    }

    Ok(Some(RustBuild {
        output,
        source: sources[0].clone(),
        target: rust_target_with_deps,
    }))
}

/// Build --extern/-L/-l flags for a rust target: direct deps for the given
/// triple, workspace path deps, and native link data from build scripts.
pub(crate) fn rust_dep_args(
    target_id: &str,
    _target_config: &poly_config::Target,
    config: &poly_config::Config,
    compiled_deps: &std::collections::HashMap<String, poly_core::CompiledDependency>,
    compiled_workspace: &HashMap<String, PathBuf>,
    target_triple: &str,
) -> Vec<String> {
    let mut args = Vec::new();

    for dep in compiled_deps.values() {
        if dep.target != target_triple {
            continue;
        }
        args.push("--extern".to_string());
        args.push(format!("{}={}", dep.lib_name, dep.rlib_path.display()));
    }

    // Workspace dependencies (poly.ini keys use the hyphenated target id)
    if let Some(workspace_deps) = config.dependencies.workspace.get(target_id) {
        for dep_name in workspace_deps.keys() {
            let dep_crate_name = dep_name.replace('-', "_");
            if let Some(artifact_path) = compiled_workspace.get(&dep_crate_name) {
                args.push("--extern".to_string());
                args.push(format!("{}={}", dep_crate_name, artifact_path.display()));
            }
        }
    }

    // Library search paths — include host (proc-macro/build) artifact dirs
    // too, since target rlibs reference them transitively. Also add native
    // link-search paths emitted by build scripts (e.g. windows import libs).
    for dep in compiled_deps.values() {
        if let Some(parent) = dep.rlib_path.parent() {
            args.push("-L".to_string());
            args.push(parent.display().to_string());
        }
        if dep.target == target_triple {
            if let Some(output) = &dep.build_script_output {
                for path in &output.link_search_paths {
                    args.push("-L".to_string());
                    args.push(path.display().to_string());
                }
                for lib in &output.link_libs {
                    args.push("-l".to_string());
                    args.push(lib.clone());
                }
            }
        }
    }
    for artifact_path in compiled_workspace.values() {
        if let Some(parent) = artifact_path.parent() {
            args.push("-L".to_string());
            args.push(parent.display().to_string());
        }
    }

    args
}

/// Resolve `[dependencies.c]` entries into compiler/linker inputs.
/// `name = ver|system` goes through pkg-config (spec §6.1/§7.2); other
/// sources are reported and skipped.
fn resolve_c_dependencies(
    config: &poly_config::Config,
    target_triple: &str,
) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<String>, Vec<String>) {
    let mut include_dirs = Vec::new();
    let mut library_dirs = Vec::new();
    let mut libraries = Vec::new();
    let mut flags = Vec::new();

    for (name, spec) in &config.dependencies.c {
        let source = spec.rsplit('|').next().unwrap_or("");
        if source == "system" {
            if target_triple.contains("windows") {
                warn!("system dependency '{}' skipped for windows target", name);
                continue;
            }
            match pkg_config_flags(name) {
                Ok((cflags, libs)) => {
                    for f in cflags.into_iter().chain(libs) {
                        if let Some(dir) = f.strip_prefix("-I") {
                            include_dirs.push(PathBuf::from(dir));
                        } else if let Some(dir) = f.strip_prefix("-L") {
                            library_dirs.push(PathBuf::from(dir));
                        } else if let Some(lib) = f.strip_prefix("-l") {
                            libraries.push(lib.to_string());
                        } else {
                            flags.push(f);
                        }
                    }
                }
                Err(e) => warn!("pkg-config failed for '{}': {}", name, e),
            }
        } else if source == "poly" || spec.starts_with("git:") || spec.starts_with("path:") {
            warn!("C dependency '{}': source '{}' is not yet supported", name, spec);
        } else {
            warn!("C dependency '{}': crates.io source is not yet supported for C", name);
        }
    }

    (include_dirs, library_dirs, libraries, flags)
}

/// `pkg-config --cflags --libs <name>` split into flag tokens.
fn pkg_config_flags(name: &str) -> Result<(Vec<String>, Vec<String>)> {
    let cflags = Command::new("pkg-config")
        .args(["--cflags", name])
        .output()
        .context("pkg-config not found")?;
    let libs = Command::new("pkg-config")
        .args(["--libs", name])
        .output()
        .context("pkg-config not found")?;
    if !cflags.status.success() || !libs.status.success() {
        anyhow::bail!("pkg-config has no package '{}'", name);
    }
    let split = |o: &std::process::Output| {
        String::from_utf8_lossy(&o.stdout)
            .split_whitespace()
            .map(|s| s.to_string())
            .collect()
    };
    Ok((split(&cflags), split(&libs)))
}

fn build_c_target(
    target_id: &str,
    target_config: &poly_config::Target,
    config: &poly_config::Config,
    output_dir: &Path,
    target_triple: &str,
    defer_link: bool,
) -> Result<Option<CBuild>> {
    info!("Building C/C++ target: {} for {}", target_id, target_triple);

    let all_sources = expand_globs(&target_config.sources)?;
    if all_sources.is_empty() {
        info!("No sources found for target {}", target_id);
        return Ok(None);
    }
    // Headers are inputs for include scanning, not compile units — compiling
    // them produces PCH objects that corrupt the archive.
    let sources: Vec<PathBuf> = all_sources
        .into_iter()
        .filter(|p| {
            !matches!(
                p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase().as_str(),
                "h" | "hh" | "hpp" | "hxx" | "h++" | "cuh"
            )
        })
        .collect();
    if sources.is_empty() {
        info!("No compilable sources found for target {}", target_id);
        return Ok(None);
    }

    let base_driver = poly_c::CDriver::new()?;
    let driver = base_driver.for_target(target_triple)?;

    // System dependencies via pkg-config (spec §7.2: pkg-config for system
    // dependencies only, not as a build system).
    let (dep_includes, dep_libdirs, dep_libs, dep_flags) =
        resolve_c_dependencies(config, target_triple);

    // Create C target
    let c_target = poly_c::CTarget {
        name: target_id.to_string(),
        language: target_config.language.clone(),
        target_type: match target_config.type_.as_str() {
            "bin" => "bin".to_string(),
            "lib" => "lib".to_string(),
            "static-lib" => "static-lib".to_string(),
            "shared-lib" => "shared-lib".to_string(),
            _ => "bin".to_string(),
        },
        sources: sources.clone(),
        headers: Vec::new(), // TODO: Extract headers
        include_dirs: dep_includes,
        library_dirs: dep_libdirs,
        libraries: dep_libs,
        flags: {
            let mut f = target_config.flags.clone();
            f.extend(dep_flags);
            f
        },
        defines: std::collections::HashMap::new(), // TODO: Parse from config
        dependencies: Vec::new(), // TODO: Resolve from config
        target_triple: target_triple.to_string(),
    };

    // Per-triple output dir, matching the Rust layout:
    // dist/{profile}/{platform}/{triple}/
    let target_dir = output_dir.join(target_triple);
    std::fs::create_dir_all(&target_dir)?;

    // Object files go in a hidden staging dir so they can't collide with
    // the final binary path.
    let target_output_dir = target_dir.join(".objects").join(target_id);
    std::fs::create_dir_all(&target_output_dir)?;

    // Compile all sources
    let objects = driver.compile_target(&c_target, &target_output_dir)?;

    // Determine output path (add .exe for Windows)
    let ext = if target_triple.contains("windows") { ".exe" } else { "" };
    let output = match c_target.target_type.as_str() {
        "bin" => target_dir.join(format!("{}{}", target_id, ext)),
        "static-lib" => target_dir.join(format!("lib{}.a", target_id)),
        "shared-lib" => target_dir.join(format!("lib{}.so", target_id)),
        _ => target_dir.join(format!("lib{}.a", target_id)),
    };

    // Link — deferred for bin targets when foreign languages will be merged
    // by poly-linker (spec §8.2 single-executable packaging).
    if !(defer_link && c_target.target_type == "bin") {
        driver.link(&c_target, &objects, &output)?;
    } else {
        info!("Deferring link of C/C++ bin target {} to unified link step", target_id);
    }

    Ok(Some(CBuild {
        output,
        target: c_target,
        objects_dir: target_output_dir,
    }))
}

fn build_go_target(
    target_id: &str,
    target_config: &poly_config::Target,
    config: &poly_config::Config,
    output_dir: &Path,
    target_triple: &str,
) -> Result<Option<GoBuild>> {
    info!("Building Go target: {} for {}", target_id, target_triple);

    let sources = expand_globs(&target_config.sources)?;
    if sources.is_empty() {
        info!("No sources found for target {}", target_id);
        return Ok(None);
    }

    let driver = poly_go::GoDriver::new()?;

    // Create Go target
    let go_target = poly_go::GoTarget {
        name: target_id.to_string(),
        module: target_config.module.clone()
            .unwrap_or_else(|| format!("github.com/{}/{}", config.project.name, target_id)),
        package: target_config.module.clone()
            .unwrap_or_else(|| format!("github.com/{}/{}", config.project.name, target_id)),
        sources: sources.clone(),
        dependencies: Vec::new(), // TODO: Resolve from config
        cgo: target_config.cgo.unwrap_or(false),
        build_tags: Vec::new(), // TODO: Parse from config
        flags: target_config.flags.clone(),
        target_type: match target_config.type_.as_str() {
            "bin" => "bin".to_string(),
            "lib" => "lib".to_string(),
            _ => "bin".to_string(),
        },
        goos: None,
        goarch: None,
    };

    // Create work directory with a go module layout: sources are copied
    // into work_dir/<pkg>/ and built as the relative package "./<pkg>".
    // Hidden staging dir avoids colliding with the output binary path.
    let target_dir = output_dir.join(target_triple);
    std::fs::create_dir_all(&target_dir)?;
    let work_dir = target_dir.join(".gobuild").join(target_id);
    std::fs::create_dir_all(&work_dir)?;

    let (goos, goarch) = if target_triple.contains("windows") {
        (Some("windows".to_string()), Some("amd64".to_string()))
    } else if target_triple.contains("linux") {
        (Some("linux".to_string()), Some("amd64".to_string()))
    } else {
        (None, None)
    };

    // All sources must belong to a single Go package (one directory).
    let pkg_dir = sources[0]
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let pkg_name = pkg_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "pkg".to_string());
    let dest_dir = work_dir.join(&pkg_name);
    std::fs::create_dir_all(&dest_dir)?;
    for src in &sources {
        if let Some(name) = src.file_name() {
            std::fs::copy(src, dest_dir.join(name))?;
        }
    }

    // Determine output path (add .exe for Windows). Must be absolute:
    // `go build` runs with cwd = work_dir.
    let ext = if target_triple.contains("windows") { ".exe" } else { "" };
    let output = match go_target.target_type.as_str() {
        "bin" => target_dir.join(format!("{}{}", target_id, ext)),
        _ => target_dir.join(format!("lib{}.a", target_id)),
    };
    let output = std::path::absolute(&output)?;

    let mut go_target = go_target;
    go_target.package = format!("./{}", pkg_name);
    go_target.goos = goos;
    go_target.goarch = goarch;

    // Spec §7.3: pure-Go packages go through `go tool compile`/`go tool link`
    // directly (no go.mod). Packages using cgo keep the `go build` path since
    // they need the cgo tool's generated files.
    let uses_cgo = go_target.cgo
        || sources.iter().any(|s| {
            std::fs::read_to_string(s)
                .map(|c| {
                    c.contains("import \"C\"")
                        || c.lines().any(|l| l.trim() == "\"C\"")
                })
                .unwrap_or(false)
        });

    if uses_cgo {
        driver.compile_with_cgo(&go_target, &output, &work_dir, &[])?;
    } else {
        match driver.compile_native(&go_target, &output, &work_dir) {
            Ok(()) => {}
            Err(e) => {
                info!("go tool compile/link failed ({}), falling back to go build", e);
                driver.compile(&go_target, &output, &work_dir)?;
            }
        }
    }

    Ok(Some(GoBuild {
        output,
        target: go_target,
        work_dir,
    }))
}

/// Expand glob patterns in source lists
pub(crate) fn expand_globs(patterns: &[String]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    for pattern in patterns {
        if pattern.contains('*') {
            // Use glob to expand pattern
            let paths = glob::glob(pattern)
                .context(format!("Invalid glob pattern: {}", pattern))?;

            for entry in paths {
                match entry {
                    Ok(path) => {
                        if path.is_file() {
                            files.push(path);
                        }
                    }
                    Err(e) => {
                        debug!("Error reading glob entry: {}", e);
                    }
                }
            }
        } else {
            // Direct path
            let path = PathBuf::from(pattern);
            if path.exists() && path.is_file() {
                files.push(path);
            }
        }
    }

    Ok(files)
}

fn cmd_add(dependency: String) -> Result<()> {
    info!("Adding dependency: {}", dependency);

    // Parse dependency format: lang:name
    let parts: Vec<&str> = dependency.split(':').collect();
    if parts.len() != 2 {
        anyhow::bail!("Invalid dependency format. Use: lang:name (e.g., rust:serde)");
    }

    let lang = parts[0];
    let name = parts[1];

    // For MVP, just print what would be added
    info!("Would add {} dependency: {}", lang, name);
    info!("(Full implementation would modify poly.ini)");

    Ok(())
}

fn cmd_remove(dependency: String) -> Result<()> {
    info!("Removing dependency: {}", dependency);

    let parts: Vec<&str> = dependency.split(':').collect();
    if parts.len() != 2 {
        anyhow::bail!("Invalid dependency format. Use: lang:name (e.g., rust:serde)");
    }

    let lang = parts[0];
    let name = parts[1];

    info!("Would remove {} dependency: {}", lang, name);
    info!("(Full implementation would modify poly.ini)");

    Ok(())
}

fn cmd_update(all: bool) -> Result<()> {
    if all {
        info!("Updating all dependencies");
    } else {
        info!("Update requires a specific dependency or --all flag");
    }

    info!("(Full implementation would update poly.lock)");

    Ok(())
}

fn cmd_lock(config_path: Option<String>) -> Result<()> {
    info!("Generating lock file");

    let config_file = config_path.unwrap_or_else(|| "poly.ini".to_string());
    let config = parse_poly_ini(&config_file)?;

    let resolver = DependencyResolver::new();
    let entries = resolver.resolve(
        &config.dependencies.rust,
        &config.dependencies.c,
        &config.dependencies.go,
    )?;

    let mut lockfile = LockFile::new();
    for entry in entries {
        lockfile.add_entry(entry);
    }
    lockfile.write("poly.lock")?;

    info!("Lock file generated: poly.lock");

    Ok(())
}

fn cmd_clean() -> Result<()> {
    info!("Cleaning build artifacts");

    // For MVP, just remove common build directories
    let dirs_to_remove = vec!["target", "jgl_build", "poly_build", "dist"];

    for dir in dirs_to_remove {
        if PathBuf::from(dir).exists() {
            std::fs::remove_dir_all(dir)?;
            info!("Removed: {}", dir);
        }
    }

    // Remove this project's compiled-dependency cache too
    if let Ok(config) = parse_poly_ini("poly.ini") {
        let cache = poly_core::project_cache_dir(&config.project.name);
        if cache.exists() {
            std::fs::remove_dir_all(&cache)?;
            info!("Removed: {}", cache.display());
        }
    }

    info!("Clean completed");

    Ok(())
}

fn cmd_validate() -> Result<()> {
    info!("Validating poly.ini");

    let config = parse_poly_ini("poly.ini")?;

    info!("Configuration is valid");
    info!("  Project: {} v{}", config.project.name, config.project.version);
    info!("  Targets: {}", config.targets.len());
    info!("  Rust dependencies: {}", config.dependencies.rust.len());
    info!("  C dependencies: {}", config.dependencies.c.len());
    info!("  Go dependencies: {}", config.dependencies.go.len());

    Ok(())
}

fn cmd_sign(keyfile: Option<String>) -> Result<()> {
    info!("Signing project");

    let config = parse_poly_ini("poly.ini")?;

    // vfsx manages its own identity keypair in the user config dir;
    // `key gen` is a no-op when keys already exist.
    let vfsx = poly_vfsx::Vfsx::new(false);
    vfsx.key_gen(false)?;

    // Export the public key archive for verifiers. The configured keyfile
    // (or --keyfile) names the exported pubkey archive.
    let public_key_path = keyfile
        .or_else(|| config.vfsx.as_ref().map(|v| v.keyfile.clone()))
        .unwrap_or_else(|| "pubkey.vfsx".to_string());
    vfsx.key_export(Path::new(&public_key_path))?;

    info!("VFSX identity ready; public key exported to {}", public_key_path);

    // Signing happens at archive creation time (vfsx create auto-signs with
    // the loaded identity). Verify existing dist archives if present.
    let dist = PathBuf::from("dist");
    if dist.is_dir() {
        let mut verified = 0usize;
        for entry in walkdir::WalkDir::new(&dist) {
            let entry = entry?;
            let path = entry.path();
            if path.is_file()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains(".vfsx"))
                    .unwrap_or(false)
                && vfsx.verify(path, Path::new(&public_key_path))?
            {
                verified += 1;
            }
        }
        if verified > 0 {
            info!("{} signed archive(s) verified", verified);
        }
    }

    Ok(())
}

fn cmd_verify(package: Option<String>) -> Result<()> {
    info!("Verifying package signatures");

    let package_path = package.unwrap_or_else(|| "dist".to_string());
    let path = PathBuf::from(&package_path);

    if !path.exists() {
        anyhow::bail!("Package path does not exist: {}", package_path);
    }

    // Look for public key archive (exported via `poly sign` / `vfsx key export`)
    let public_key_path = ["pubkey.vfsx", "poly.pub"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists());
    let public_key_path = match public_key_path {
        Some(p) => p,
        None => {
            info!("No public key found (pubkey.vfsx)");
            info!("Run 'poly sign' to generate keys first");
            return Ok(());
        }
    };

    let vfsx = poly_vfsx::Vfsx::new(false);
    
    if path.is_file() {
        if vfsx.verify(&path, &public_key_path)? {
            info!("Package signature verified successfully");
        } else {
            info!("Package signature verification failed");
        }
    } else if path.is_dir() {
        // Verify all .vfsx files in directory
        for entry in std::fs::read_dir(&path)? {
            let entry = entry?;
            let file_path = entry.path();
            
            if file_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.contains(".vfsx"))
                .unwrap_or(false)
            {
                if vfsx.verify(&file_path, &public_key_path)? {
                    info!("{}: verified", file_path.display());
                } else {
                    info!("{}: verification failed", file_path.display());
                }
            }
        }
    }

    Ok(())
}

fn cmd_login(registry: Option<String>, token: Option<String>) -> Result<()> {
    let registry = registry.unwrap_or_else(|| "crates-io".to_string());
    let token = match token {
        Some(t) => t,
        None => {
            // Prompt without echoing.
            let t = rpassword::prompt_password("Enter API token: ")?;
            if t.trim().is_empty() {
                anyhow::bail!("Empty token");
            }
            t.trim().to_string()
        }
    };
    let path = crates_io::save_token(&token, &registry)?;
    info!("Saved {} token to {}", registry, path.display());
    Ok(())
}

fn cmd_logout(registry: Option<String>) -> Result<()> {
    let registry = registry.unwrap_or_else(|| "crates-io".to_string());
    if crates_io::clear_token(&registry)? {
        info!("Removed {} token", registry);
    } else {
        info!("No stored token for {}", registry);
    }
    Ok(())
}

/// CARGO_PKG_* / CARGO_MANIFEST_DIR env vars cargo would normally provide,
/// so `env!("CARGO_PKG_*")` works in Poly-built crates.
fn manifest_env(target_id: &str, config: &poly_config::Config, first_source: &Path) -> Vec<(String, String)> {
    let p = &config.project;
    let mut v: Vec<(String, String)> = vec![
        ("CARGO_PKG_NAME".into(), target_id.replace('-', "_")),
        ("CARGO_PKG_VERSION".into(), p.version.clone()),
        ("CARGO_MANIFEST_DIR".into(), first_source
            .parent()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".into())),
    ];
    let parts: Vec<&str> = p.version.split('.').collect();
    v.push(("CARGO_PKG_VERSION_MAJOR".into(), parts.first().unwrap_or(&"0").to_string()));
    v.push(("CARGO_PKG_VERSION_MINOR".into(), parts.get(1).unwrap_or(&"0").to_string()));
    v.push(("CARGO_PKG_VERSION_PATCH".into(), parts.get(2).unwrap_or(&"0").to_string()));
    v.push(("CARGO_PKG_AUTHORS".into(), p.authors.join(":")));
    v.push(("CARGO_PKG_DESCRIPTION".into(), p.description.clone().unwrap_or_default()));
    v.push(("CARGO_PKG_HOMEPAGE".into(), p.homepage.clone().unwrap_or_default()));
    v.push(("CARGO_PKG_REPOSITORY".into(), p.repository.clone().unwrap_or_default()));
    v.push(("CARGO_PKG_LICENSE".into(), p.license.clone().unwrap_or_default()));
    if let Some(r) = &p.readme {
        v.push(("CARGO_PKG_README".into(), r.clone()));
    }
    v
}

/// Crate names of every Rust publish target — recorded as package
/// dependencies so a meta install can pull the whole family.
fn sibling_crates(config: &poly_config::Config) -> Vec<String> {
    let specs = config
        .publish
        .as_ref()
        .filter(|p| !p.targets.is_empty())
        .map(|p| p.targets.clone())
        .unwrap_or_else(|| config.build.default.targets.clone());
    specs.iter()
        .filter(|t| t.starts_with("rust:"))
        .map(|t| crates_io::crate_name_for(config, t.trim_start_matches("rust:")))
        .collect()
}


/// Read rust dependency versions pinned in an existing poly.lock.
pub(crate) fn read_locked_rust_versions() -> HashMap<String, String> {
    let mut locked = HashMap::new();
    if let Ok(lock) = LockFile::read("poly.lock") {
        for entry in lock.dependencies {
            // Only trust entries that look like resolved semver versions
            // (x.y.z). Older lockfiles stored requirements like "1.0".
            if entry.language == "rust"
                && entry.version.split('.').count() == 3
                && entry.version.split('.').all(|p| p.chars().all(|c| c.is_ascii_digit()))
            {
                locked.insert(entry.name, entry.version);
            }
        }
    }
    locked
}

/// Default install root: ~/.local/share/poly, binaries to ~/.local/bin
fn default_install_config() -> poly_package::InstallConfig {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let home = PathBuf::from(home);
    poly_package::InstallConfig {
        install_dir: home.join(".local/share/poly"),
        bin_dir: Some(home.join(".local/bin")),
        verify_signatures: true,
        public_key_path: ["pubkey.vfsx", "poly.pub"]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.exists()),
    }
}


async fn cmd_uninstall(package: String) -> Result<()> {
    info!("Uninstalling package: {}", package);

    let installer = poly_package::Installer::new(default_install_config());
    installer.uninstall(&package).await?;

    Ok(())
}
