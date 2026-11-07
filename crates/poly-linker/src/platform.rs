//! Per-platform unified linking (spec §8.2).
//!
//! Every target produced for a platform is linked into ONE executable by
//! poly-linker. The first `bin` target (in topological build order) is the
//! primary entrypoint; every other target contributes its objects or
//! archives to the final link.

use anyhow::Result;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Per-target build info retained so the linker can merge every target
/// into the single per-platform executable.
#[derive(Debug)]
pub struct RustBuild {
    pub output: PathBuf,
    pub source: PathBuf,
    pub target: poly_rust::RustTarget,
}

#[derive(Debug)]
pub struct CBuild {
    pub output: PathBuf,
    pub target: poly_c::CTarget,
    pub objects_dir: PathBuf,
}

#[derive(Debug)]
pub struct GoBuild {
    pub output: PathBuf,
    pub target: poly_go::GoTarget,
    pub work_dir: PathBuf,
}

#[derive(Debug)]
pub struct LinkUnit {
    pub id: String,
    pub language: String,
    pub kind: String,
    pub artifact: Option<PathBuf>,
    pub rust: Option<RustBuild>,
    pub c: Option<CBuild>,
    pub go: Option<GoBuild>,
}

/// Link every target produced for a platform into ONE executable.
/// Output: dist/{profile}/{platform}/{triple}/bin/{project}.
pub fn link_platform_executable(
    units: &[LinkUnit],
    platform_dir: &Path,
    target_triple: &str,
    project_name: &str,
) -> Result<Option<PathBuf>> {
    let bin_count = units.iter().filter(|u| u.kind == "bin").count();
    let Some(primary_idx) = units.iter().position(|u| u.kind == "bin") else {
        warn!("No bin target for {}: no platform executable produced", target_triple);
        return Ok(None);
    };
    if bin_count > 1 {
        warn!(
            "{} bin targets for {}: only the first is merged into the platform executable",
            bin_count, target_triple
        );
    }
    let primary = &units[primary_idx];
    let Some(primary_artifact) = primary.artifact.clone() else {
        return Ok(None);
    };

    let target_dir = platform_dir.join(target_triple);
    let unified_dir = target_dir.join("bin");
    std::fs::create_dir_all(&unified_dir)?;
    let ext = if target_triple.contains("windows") { ".exe" } else { "" };
    let unified = unified_dir.join(format!("{}{}", project_name, ext));

    // Collect foreign-language artifacts that must be merged into the exe.
    let linklibs_dir = target_dir.join(".linklibs");
    let mut foreign_libs: Vec<PathBuf> = Vec::new();

    for (i, unit) in units.iter().enumerate() {
        if i == primary_idx {
            continue;
        }
        match unit.language.as_str() {
            "c" | "cpp" => {
                if unit.kind == "bin" {
                    warn!(
                        "C/C++ bin target '{}' has its own entrypoint; not merged",
                        unit.id
                    );
                    continue;
                }
                if let Some(artifact) = &unit.artifact {
                    let e = artifact
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if matches!(e.as_str(), "a" | "so" | "lib" | "dll" | "dylib") {
                        foreign_libs.push(artifact.clone());
                    }
                }
            }
            "rust" => {
                // Rust rlib deps of a Rust primary are already inside the
                // exe via --extern; only non-Rust primaries need the code
                // recompiled as a self-contained static library.
                if primary.language == "rust"
                    || !matches!(unit.kind.as_str(), "lib" | "static-lib" | "cdylib")
                {
                    continue;
                }
                if let Some(rb) = &unit.rust {
                    std::fs::create_dir_all(&linklibs_dir)?;
                    let out = linklibs_dir.join(format!("lib{}.a", rb.target.name));
                    let mut t = rb.target.clone();
                    t.crate_type = "staticlib".to_string();
                    poly_rust::RustDriver::new()?.compile(&rb.source, &out, &t, &[], None)?;
                    foreign_libs.push(out);
                }
            }
            "go" => {
                if primary.language == "go" {
                    continue;
                }
                if let Some(gb) = &unit.go {
                    std::fs::create_dir_all(&linklibs_dir)?;
                    let out = linklibs_dir.join(format!("lib{}.a", unit.id.replace('-', "_")));
                    // `go build` runs with cwd = work_dir; the -o path must
                    // be absolute or the archive lands inside the work dir.
                    let out = std::path::absolute(&out)?;
                    let driver = poly_go::GoDriver::new()?;
                    match driver.build_c_archive(&gb.target, &out, &gb.work_dir) {
                        Ok(()) => foreign_libs.push(out),
                        Err(e) => warn!(
                            "Go target '{}' could not be merged as a C archive: {}",
                            unit.id, e
                        ),
                    }
                }
            }
            _ => {}
        }
    }

    if foreign_libs.is_empty() && primary_artifact.exists() {
        // Single-language (or Rust-only) build: the primary artifact already
        // is the platform executable - just give it the unified name.
        std::fs::copy(&primary_artifact, &unified)?;
        return Ok(Some(unified));
    }

    info!(
        "Linking {} foreign libraries into {} for {}",
        foreign_libs.len(),
        unified.display(),
        target_triple
    );

    match primary.language.as_str() {
        "rust" => {
            let rb = primary
                .rust
                .as_ref()
                .expect("rust bin unit missing build info");
            let mut t = rb.target.clone();
            let mut parts: Vec<String> = foreign_libs
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            // Windows archives (e.g. Go c-archive) reference the mingw
            // system libraries that `go build` would add automatically.
            if target_triple.contains("windows") {
                parts.extend(
                    [
                        "-lmingwex", "-lmingw32", "-lmsvcrt", "-lkernel32", "-luser32",
                        "-lgdi32", "-ladvapi32", "-lshell32", "-lole32", "-luuid",
                        "-lwinmm", "-lws2_32",
                    ]
                    .iter()
                    .map(|s| s.to_string()),
                );
            }
            let link_args = parts.join(" ");
            if !link_args.is_empty() {
                t.flags.push("-C".to_string());
                t.flags.push(format!("link-args={}", link_args));
            }
            poly_rust::RustDriver::new()?.compile(&rb.source, &unified, &t, &[], None)?;
        }
        "c" | "cpp" => {
            let cb = primary
                .c
                .as_ref()
                .expect("c bin unit missing build info");
            let driver = poly_c::CDriver::new()?.for_target(target_triple)?;
            let objects = collect_objects(&cb.objects_dir);
            let mut lt = cb.target.clone();
            for lib in &foreign_libs {
                if let Some(parent) = lib.parent() {
                    lt.library_dirs.push(parent.to_path_buf());
                }
                if let Some(stem) = lib.file_stem().and_then(|s| s.to_str()) {
                    lt.libraries
                        .push(stem.trim_start_matches("lib").to_string());
                }
            }
            driver.link(&lt, &objects, &unified)?;
        }
        "go" => {
            let gb = primary
                .go
                .as_ref()
                .expect("go bin unit missing build info");
            let driver = poly_go::GoDriver::new()?;
            let mut gt = gb.target.clone();
            let args = foreign_libs
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(" ");
            gt.flags.push(format!(
                "-ldflags=-linkmode=external -extldflags '{}'",
                args
            ));
            let merged = if gt.cgo {
                driver
                    .compile_with_cgo(&gt, &unified, &gb.work_dir, &[])
                    .is_ok()
            } else {
                driver.compile(&gt, &unified, &gb.work_dir).is_ok()
            };
            if !merged {
                warn!("External-link merge failed for Go primary; using Go-only executable");
                std::fs::copy(&primary_artifact, &unified)?;
            }
        }
        other => {
            anyhow::bail!("Cannot link unified executable with primary language '{}'", other);
        }
    }

    Ok(Some(unified))
}

/// Recursively collect `.o` files under a C objects staging dir.
fn collect_objects(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = walkdir::WalkDir::new(dir).into_iter().collect::<Result<Vec<_>, _>>() {
        for entry in entries {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("o") {
                out.push(p.to_path_buf());
            }
        }
    }
    out
}
