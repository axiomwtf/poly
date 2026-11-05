//! Poly CLI entry point

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use poly_config::parse_poly_ini;
use poly_resolver::{DependencyResolver, LockFile};
use std::path::{Path, PathBuf};
use tracing::{debug, info};
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
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Init { name } => cmd_init(name),
        Commands::Build { target, release, config } => cmd_build(target, release, config).await,
        Commands::Add { dependency } => cmd_add(dependency),
        Commands::Remove { dependency } => cmd_remove(dependency),
        Commands::Update { all } => cmd_update(all),
        Commands::Lock { config } => cmd_lock(config),
        Commands::Clean => cmd_clean(),
        Commands::Validate => cmd_validate(),
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

async fn cmd_build(_target: Option<String>, _release: bool, config_path: Option<String>) -> Result<()> {
    info!("Building project");

    // Load configuration
    let config_file = config_path.unwrap_or_else(|| "poly.ini".to_string());
    let config = parse_poly_ini(&config_file)?;
    debug!("Loaded config for project: {}", config.project.name);

    // Resolve dependencies
    let resolver = DependencyResolver::new();
    let entries = resolver.resolve(
        &config.dependencies.rust,
        &config.dependencies.c,
        &config.dependencies.go,
    )?;

    // Generate lock file
    let mut lockfile = LockFile::new();
    for entry in entries {
        lockfile.add_entry(entry);
    }
    lockfile.write("poly.lock")?;

    // Create output directory
    let output_dir = PathBuf::from("target");
    std::fs::create_dir_all(&output_dir)?;

    // Build each target
    for (target_id, target_config) in &config.targets {
        match target_config.language.as_str() {
            "rust" => {
                build_rust_target(target_id, target_config, &config, &output_dir)?;
            }
            "c" | "cpp" => {
                build_c_target(target_id, target_config, &config, &output_dir)?;
            }
            "go" => {
                build_go_target(target_id, target_config, &config, &output_dir)?;
            }
            _ => {
                info!("Unknown language: {}", target_config.language);
            }
        }
    }

    info!("Build completed successfully");

    Ok(())
}

fn build_rust_target(
    target_id: &str,
    target_config: &poly_config::Target,
    config: &poly_config::Config,
    output_dir: &Path,
) -> Result<()> {
    info!("Building Rust target: {}", target_id);

    let sources = expand_globs(&target_config.sources)?;
    if sources.is_empty() {
        info!("No sources found for target {}", target_id);
        return Ok(());
    }

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
        source: sources[0].clone(),
        flags: target_config.flags.clone(),
    };

    // Determine output path
    let output = match rust_target.crate_type.as_str() {
        "bin" => output_dir.join(&rust_target.name),
        _ => output_dir.join(format!("lib{}.rlib", rust_target.name)),
    };

    // Compile
    driver.compile(
        &sources[0],
        &output,
        &rust_target,
        &[], // No dependencies for MVP
        None, // No build script for MVP
    )?;

    Ok(())
}

fn build_c_target(
    target_id: &str,
    target_config: &poly_config::Target,
    _config: &poly_config::Config,
    output_dir: &Path,
) -> Result<()> {
    info!("Building C/C++ target: {}", target_id);

    let sources = expand_globs(&target_config.sources)?;
    if sources.is_empty() {
        info!("No sources found for target {}", target_id);
        return Ok(());
    }

    let driver = poly_c::CDriver::new()?;

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
        include_dirs: Vec::new(), // TODO: Parse from config
        library_dirs: Vec::new(), // TODO: Parse from config
        libraries: Vec::new(), // TODO: Parse from config
        flags: target_config.flags.clone(),
        defines: std::collections::HashMap::new(), // TODO: Parse from config
        dependencies: Vec::new(), // TODO: Resolve from config
    };

    // Create target-specific output directory
    let target_output_dir = output_dir.join(target_id);
    std::fs::create_dir_all(&target_output_dir)?;

    // Compile all sources
    let objects = driver.compile_target(&c_target, &target_output_dir)?;

    // Determine output path
    let output = match c_target.target_type.as_str() {
        "bin" => output_dir.join(target_id),
        "static-lib" => output_dir.join(format!("lib{}.a", target_id)),
        "shared-lib" => output_dir.join(format!("lib{}.so", target_id)),
        _ => output_dir.join(format!("lib{}.a", target_id)),
    };

    // Link
    driver.link(&c_target, &objects, &output)?;

    Ok(())
}

fn build_go_target(
    target_id: &str,
    target_config: &poly_config::Target,
    config: &poly_config::Config,
    output_dir: &Path,
) -> Result<()> {
    info!("Building Go target: {}", target_id);

    let sources = expand_globs(&target_config.sources)?;
    if sources.is_empty() {
        info!("No sources found for target {}", target_id);
        return Ok(());
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
    };

    // Create work directory
    let work_dir = output_dir.join(target_id);
    std::fs::create_dir_all(&work_dir)?;

    // Determine output path
    let output = match go_target.target_type.as_str() {
        "bin" => output_dir.join(target_id),
        _ => output_dir.join(format!("lib{}.a", target_id)),
    };

    // Compile
    if go_target.cgo {
        driver.compile_with_cgo(&go_target, &output, &work_dir, &[])?;
    } else {
        driver.compile(&go_target, &output, &work_dir)?;
    }

    Ok(())
}

/// Expand glob patterns in source lists
fn expand_globs(patterns: &[String]) -> Result<Vec<PathBuf>> {
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
    let dirs_to_remove = vec!["target", "jgl_build", "poly_build"];

    for dir in dirs_to_remove {
        if PathBuf::from(dir).exists() {
            std::fs::remove_dir_all(dir)?;
            info!("Removed: {}", dir);
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
