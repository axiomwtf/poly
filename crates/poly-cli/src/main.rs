//! Poly CLI entry point

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use poly_config::parse_poly_ini;
use poly_resolver::{DependencyResolver, LockFile};
use std::path::PathBuf;
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
    Lock,
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
        Commands::Build { target, release } => cmd_build(target, release).await,
        Commands::Add { dependency } => cmd_add(dependency),
        Commands::Remove { dependency } => cmd_remove(dependency),
        Commands::Update { all } => cmd_update(all),
        Commands::Lock => cmd_lock(),
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

async fn cmd_build(_target: Option<String>, _release: bool) -> Result<()> {
    info!("Building project");

    // Load configuration
    let config = parse_poly_ini("poly.ini")?;
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
    std::fs::create_dir_all("target")?;

    // Build each target
    for (target_id, target_config) in &config.targets {
        match target_config.language.as_str() {
            "rust" => {
                let sources = expand_globs(&target_config.sources)?;
                if sources.is_empty() {
                    info!("No sources found for target {}", target_id);
                    continue;
                }

                info!("Building Rust target: {}", target_id);

                // For MVP, just use rustc directly on the first source
                let driver = poly_rust::RustDriver::new()?;
                let edition = target_config.edition.as_deref().or(config.project.edition.as_deref());
                let crate_type = match target_config.type_.as_str() {
                    "lib" => Some("lib"),
                    "bin" => Some("bin"),
                    "static-lib" => Some("staticlib"),
                    "cdylib" => Some("cdylib"),
                    _ => None,
                };

                for source in sources {
                    let output = match crate_type {
                        Some("bin") => PathBuf::from(format!("target/{}", target_id)),
                        _ => PathBuf::from(format!("target/{}.rlib", target_id)),
                    };

                    driver.compile(
                        &source,
                        &output,
                        edition,
                        &target_config.features,
                        &target_config.flags,
                        crate_type,
                    )?;
                }
            }
            "c" | "cpp" => {
                let sources = expand_globs(&target_config.sources)?;
                if sources.is_empty() {
                    info!("No sources found for target {}", target_id);
                    continue;
                }

                info!("Building C/C++ target: {}", target_id);

                let driver = poly_c::CDriver::new()?;
                let is_cpp = target_config.language == "cpp";

                for source in sources {
                    let ext = source.extension().and_then(|s| s.to_str()).unwrap_or("");
                    let is_cpp_file = is_cpp || ext == "cpp" || ext == "cc" || ext == "cxx";
                    let output = PathBuf::from(format!(
                        "target/{}.{}",
                        target_id,
                        if is_cpp_file { "o" } else { "o" }
                    ));

                    driver.compile(&source, &output, &target_config.flags, is_cpp_file)?;
                }
            }
            "go" => {
                let sources = expand_globs(&target_config.sources)?;
                if sources.is_empty() {
                    info!("No sources found for target {}", target_id);
                    continue;
                }

                info!("Building Go target: {}", target_id);

                let driver = poly_go::GoDriver::new()?;
                let output = PathBuf::from(format!("target/{}", target_id));
                let cgo = target_config.cgo.unwrap_or(false);

                driver.compile(
                    &sources,
                    &output,
                    target_config.module.as_deref(),
                    cgo,
                    &target_config.flags,
                )?;
            }
            _ => {
                info!("Unknown language: {}", target_config.language);
            }
        }
    }

    info!("Build completed successfully");

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

fn cmd_lock() -> Result<()> {
    info!("Generating lock file");

    let config = parse_poly_ini("poly.ini")?;

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
