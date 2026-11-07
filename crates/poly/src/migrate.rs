//! `poly migrate` - convert Cargo.toml / go.mod / CMakeLists.txt to poly.ini.
//!
//! Produces a complete poly.ini: [project] metadata, [target.*] sections,
//! dependency tables, and [workspace] members for cargo workspaces.
//! Unrepresentable constructs are noted in migration-report.txt.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn migrate(from: &str) -> Result<()> {
    match from {
        "cargo" => migrate_cargo(),
        "go-mod" | "gomod" => migrate_go_mod(),
        "cmake" => migrate_cmake(),
        other => anyhow::bail!("Unknown source: {} (cargo|go-mod|cmake)", other),
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

struct Report {
    lines: Vec<String>,
}

impl Report {
    fn new() -> Self {
        Self { lines: Vec::new() }
    }
    fn note(&mut self, msg: impl Into<String>) {
        self.lines.push(msg.into());
    }
}

fn backup_poly_ini() -> Result<()> {
    let poly_ini = PathBuf::from("poly.ini");
    if poly_ini.exists() {
        std::fs::copy(&poly_ini, "poly.ini.bak")?;
    }
    Ok(())
}

fn write_output(ini: &str, report: &Report) -> Result<()> {
    backup_poly_ini()?;
    std::fs::write("poly.ini", ini)?;
    let mut text = String::from("# poly migrate report\n\n");
    if report.lines.is_empty() {
        text.push_str("no issues - everything migrated cleanly\n");
    } else {
        for l in &report.lines {
            text.push_str(l);
            text.push('\n');
        }
    }
    std::fs::write("migration-report.txt", text)?;
    Ok(())
}

fn ini_str(v: &toml::Value) -> Option<String> {
    v.as_str().map(|s| s.to_string())
}

fn ini_array(values: &[String]) -> String {
    values.join(", ")
}

/// poly.ini has no quoting semantics - values are taken verbatim after `=`.
fn ini_val(s: &str) -> String {
    s.to_string()
}

fn push_kv(out: &mut String, key: &str, value: &str) {
    if !value.is_empty() {
        out.push_str(&format!("{} = {}\n", key, ini_val(value)));
    }
}

// ---------------------------------------------------------------------------
// Cargo.toml migration
// ---------------------------------------------------------------------------

fn migrate_cargo() -> Result<()> {
    let manifest_path = PathBuf::from("Cargo.toml");
    if !manifest_path.exists() {
        anyhow::bail!("No Cargo.toml found");
    }
    let root_dir = std::env::current_dir()?;
    let text = std::fs::read_to_string(&manifest_path)?;
    let doc: toml::Value = toml::from_str(&text).context("Failed to parse Cargo.toml")?;

    let mut report = Report::new();
    let mut ini = String::new();

    // Workspace members
    let mut member_pkgs: Vec<CargoPkg> = Vec::new();
    let ws_edition = doc
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("edition"))
        .and_then(ini_str);
    if let Some(members) = doc
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
    {
        for pat in members.iter().filter_map(|m| m.as_str()) {
            let pat = root_dir.join(pat).join("Cargo.toml");
            let pat = pat.to_string_lossy().to_string();
            for entry in glob::glob(&pat)
                .with_context(|| format!("bad workspace member glob: {}", pat))?
            {
                if let Ok(p) = entry {
                    if let Ok(pkg) = CargoPkg::load(&p, ws_edition.as_deref()) {
                        member_pkgs.push(pkg);
                    }
                }
            }
        }
        member_pkgs.sort_by(|a, b| a.name.cmp(&b.name));
        member_pkgs.dedup_by(|a, b| a.name == b.name);
    }
    let member_names: Vec<String> = member_pkgs.iter().map(|p| p.name.clone()).collect();

    // [workspace.dependencies] - shared version specs, become global rust deps
    let ws_deps = doc
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(|d| d.as_table());

    // [project]: root package, or workspace.package + dir name for virtual manifests
    let ws_pkg = doc.get("workspace").and_then(|w| w.get("package"));
    if doc.get("package").is_some() {
        emit_project(&mut ini, &doc, &root_dir);
    } else {
        ini.push_str("[project]\n");
        let dir_name = root_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "migrated".into());
        push_kv(&mut ini, "name", &dir_name);
        let version = ws_pkg
            .and_then(|p| p.get("version"))
            .and_then(ini_str)
            .unwrap_or_else(|| "0.1.0".into());
        push_kv(&mut ini, "version", &version);
        if let Some(e) = ws_pkg.and_then(|p| p.get("edition")).and_then(ini_str) {
            push_kv(&mut ini, "edition", &e);
        }
        for (k, ik) in [
            ("description", "description"),
            ("license", "license"),
            ("repository", "repository"),
            ("homepage", "homepage"),
        ] {
            if let Some(v) = ws_pkg.and_then(|p| p.get(k)).and_then(ini_str) {
                push_kv(&mut ini, ik, &v);
            }
        }
        ini.push('\n');
        report.note("virtual workspace manifest: [project] filled from [workspace.package] + directory name");
    }

    // [workspace] section
    if !member_names.is_empty() {
        ini.push_str("\n[workspace]\n");
        push_kv(&mut ini, "members", &ini_array(&member_names));
        if let Some(ws) = ws_deps {
            let shared: Vec<String> = ws.keys().cloned().collect();
            if !shared.is_empty() {
                push_kv(&mut ini, "sharedDeps", &ini_array(&shared));
            }
        }
        ini.push('\n');
    }

    // Members in dependency order (deps before dependents)
    let order = topo_members(&member_pkgs);

    // Resolve each member's deps once; reused for the global union and
    // per-member [dependencies.workspace.X] sections.
    let mut member_lines: std::collections::HashMap<String, Vec<(String, String)>> =
        std::collections::HashMap::new();
    for pkg in &member_pkgs {
        if let Some(table) = pkg.doc.get("dependencies").and_then(|d| d.as_table()) {
            let lines = cargo_dep_lines(Some(table), ws_deps, &member_names, &mut report, &pkg.name);
            member_lines.insert(pkg.name.clone(), lines);
        }
    }

    // [dependencies.rust]: poly compiles only the GLOBAL dep set - member
    // registry deps must be unioned in or they never reach rustc. `path`
    // entries (sibling crates) stay member-local.
    let mut global_deps: Vec<(String, String)> = Vec::new();
    if let Some(ws) = ws_deps {
        global_deps.extend(cargo_dep_lines(Some(ws), None, &member_names, &mut report, "workspace.dependencies"));
    }
    if doc.get("package").is_some() {
        global_deps.extend(cargo_dep_lines(
            doc.get("dependencies").and_then(|d| d.as_table()),
            ws_deps,
            &member_names,
            &mut report,
            "root package",
        ));
    }
    for lines in member_lines.values() {
        for (name, spec) in lines {
            if spec != "path" {
                global_deps.push((name.clone(), spec.clone()));
            }
        }
    }
    global_deps.sort();
    let mut deduped: Vec<(String, String)> = Vec::new();
    for (name, spec) in global_deps {
        if let Some(existing) = deduped.iter_mut().find(|(n, _)| *n == name) {
            if existing.1 != spec {
                report.note(format!("dep '{}': conflicting specs '{}' vs '{}' - kept '{}'", name, existing.1, spec, existing.1));
            }
        } else {
            deduped.push((name, spec));
        }
    }
    if !deduped.is_empty() {
        ini.push_str("\n[dependencies.rust]\n");
        for (name, spec) in &deduped {
            ini.push_str(&format!("{} = {}\n", name, ini_val(spec)));
        }
    }
    for section in ["dev-dependencies", "build-dependencies"] {
        if doc.get(section).and_then(|d| d.as_table()).map(|t| !t.is_empty()).unwrap_or(false) {
            report.note(format!("[{}] not migrated (poly.ini has no dev/build-dep split)", section));
        }
    }

    let mut all_target_ids: Vec<String> = Vec::new();

    for i in order {
        let pkg = &member_pkgs[i];
        let mut raw = Vec::new();
        collect_cargo_targets(pkg, &mut report, &mut raw);
        // Assign ids: single target keeps package name; lib+bin → name-lib + name
        let has_lib = raw.iter().any(|t| t.ty == "lib");
        let has_bin = raw.iter().any(|t| t.ty == "bin");
        for t in raw {
            let id = if t.ty == "lib" {
                // lib ids come from the package name (hyphenated), not the
                // underscored crate name
                if has_lib && has_bin {
                    format!("{}-lib", sanitize(&pkg.name))
                } else {
                    sanitize(&pkg.name)
                }
            } else {
                sanitize(&t.name)
            };
            let edition = pkg.edition.clone();
            let body = target_body("rust", &t.ty, &pkg.dir, &t.path, &edition, &mut report);
            ini.push_str(&format!("\n[target.{}]\n{}\n", id, body));
            all_target_ids.push(format!("rust:{}", id));
        }
        if let Some(lines) = member_lines.get(&pkg.name) {
            if !lines.is_empty() {
                ini.push_str(&format!("\n[dependencies.workspace.{}]\n", pkg.name));
                for (name, spec) in lines {
                    ini.push_str(&format!("{} = {}\n", name, ini_val(spec)));
                }
            }
        }
        report.note(format!("migrated package: {}", pkg.name));
    }

    // Root package targets (non-virtual manifests with [package] + [workspace])
    if doc.get("package").is_some() && !member_pkgs.is_empty() {
        let root_pkg = CargoPkg::load(&manifest_path, ws_edition.as_deref())?;
        let mut raw = Vec::new();
        collect_cargo_targets(&root_pkg, &mut report, &mut raw);
        for t in raw {
            let id = sanitize(&t.name);
            let body = target_body("rust", &t.ty, &root_pkg.dir, &t.path, &root_pkg.edition, &mut report);
            ini.push_str(&format!("\n[target.{}]\n{}\n", id, body));
            all_target_ids.push(format!("rust:{}", id));
        }
    }

    ini.push_str("\n[build.default]\n");
    push_kv(&mut ini, "targets", &ini_array(&all_target_ids));
    ini.push('\n');

    write_output(&ini, &report)?;
    tracing::info!("Migrated Cargo.toml -> poly.ini (report: migration-report.txt)");
    Ok(())
}

struct CargoPkg {
    name: String,
    edition: String,
    dir: PathBuf,
    doc: toml::Value,
}

impl CargoPkg {
    fn load(manifest: &Path, ws_edition: Option<&str>) -> Result<Self> {
        let text = std::fs::read_to_string(manifest)?;
        let doc: toml::Value = toml::from_str(&text)
            .with_context(|| format!("Failed to parse {}", manifest.display()))?;
        let pkg = &doc["package"];
        let name = pkg
            .get("name")
            .and_then(ini_str)
            .with_context(|| format!("{} missing [package].name", manifest.display()))?;
        let dir = manifest.parent().unwrap_or(Path::new(".")).to_path_buf();
        let edition = pkg
            .get("edition")
            .and_then(ini_str)
            .map(String::from)
            .or_else(|| ws_edition.map(String::from))
            .unwrap_or_else(|| "2021".into());
        Ok(Self { name, edition, dir, doc })
    }
}

struct RawTarget {
    name: String,
    ty: String,
    path: String,
}

/// Discover cargo targets: explicit [lib]/[[bin]] plus src/ autodiscovery.
fn collect_cargo_targets(pkg: &CargoPkg, report: &mut Report, out: &mut Vec<RawTarget>) {
    let src = pkg.dir.join("src");
    let mut saw_lib = false;
    let mut saw_bin = false;

    if let Some(lib) = pkg.doc.get("lib") {
        if let Some(path) = lib.get("path").and_then(ini_str) {
            let tname = lib
                .get("name")
                .and_then(ini_str)
                .unwrap_or_else(|| pkg.name.replace('-', "_"));
            out.push(RawTarget { name: tname, ty: "lib".into(), path });
            saw_lib = true;
        }
    }
    if let Some(bins) = pkg.doc.get("bin").and_then(|b| b.as_array()) {
        for b in bins {
            let bname = b.get("name").and_then(ini_str).unwrap_or_else(|| pkg.name.clone());
            let path = b.get("path").and_then(ini_str).unwrap_or_else(|| "src/main.rs".into());
            out.push(RawTarget { name: bname, ty: "bin".into(), path });
            saw_bin = true;
        }
    }
    if !saw_lib && src.join("lib.rs").exists() {
        out.push(RawTarget {
            name: pkg.name.replace('-', "_"),
            ty: "lib".into(),
            path: "src/lib.rs".into(),
        });
    }
    if !saw_bin && src.join("main.rs").exists() {
        out.push(RawTarget {
            name: pkg.name.clone(),
            ty: "bin".into(),
            path: "src/main.rs".into(),
        });
    }
    let bin_dir = src.join("bin");
    if bin_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&bin_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("rs") {
                    let stem = p.file_stem().unwrap().to_string_lossy().to_string();
                    out.push(RawTarget {
                        name: stem.clone(),
                        ty: "bin".into(),
                        path: format!("src/bin/{}.rs", stem),
                    });
                }
            }
        }
    }
    if out.is_empty() {
        report.note(format!("{}: no rust targets found", pkg.name));
    }
}

/// Order member indices so dependencies build before dependents.
fn topo_members(pkgs: &[CargoPkg]) -> Vec<usize> {
    let names: Vec<&str> = pkgs.iter().map(|p| p.name.as_str()).collect();
    let mut done = vec![false; pkgs.len()];
    let mut order = Vec::new();
    for i in 0..pkgs.len() {
        topo_visit(i, pkgs, &names, &mut done, &mut order);
    }
    order
}

fn topo_visit(
    i: usize,
    pkgs: &[CargoPkg],
    names: &[&str],
    done: &mut [bool],
    order: &mut Vec<usize>,
) {
    if done[i] {
        return;
    }
    done[i] = true;
    // Deps on sibling packages must come first
    if let Some(deps) = pkgs[i].doc.get("dependencies").and_then(|d| d.as_table()) {
        for dep_name in deps.keys() {
            if let Some(j) = names.iter().position(|n| *n == dep_name) {
                topo_visit(j, pkgs, names, done, order);
            }
        }
    }
    order.push(i);
}

/// Emit [project] from a root cargo manifest ([package] or [workspace.package]).
fn emit_project(ini: &mut String, doc: &toml::Value, _root_dir: &Path) {
    ini.push_str("[project]\n");
    let pkg = doc.get("package").unwrap_or_else(|| &doc["workspace"]["package"]);
    push_kv(ini, "name", &pkg.get("name").and_then(ini_str).unwrap_or_else(|| "migrated".into()));
    push_kv(
        ini,
        "version",
        &pkg.get("version").and_then(ini_str).unwrap_or_else(|| "0.1.0".into()),
    );
    if let Some(e) = pkg.get("edition").and_then(ini_str) {
        push_kv(ini, "edition", &e);
    }
    for (k, ik) in [
        ("description", "description"),
        ("license", "license"),
        ("repository", "repository"),
        ("homepage", "homepage"),
        ("documentation", "documentation"),
        ("readme", "readme"),
    ] {
        if let Some(v) = pkg.get(k).and_then(ini_str) {
            push_kv(ini, ik, &v);
        }
    }
    for (k, ik) in [("keywords", "keywords"), ("categories", "categories"), ("authors", "authors")] {
        if let Some(arr) = pkg.get(k).and_then(|v| v.as_array()) {
            let vals: Vec<String> = arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
            if !vals.is_empty() {
                push_kv(ini, ik, &ini_array(&vals));
            }
        }
    }
    if let Some(lf) = pkg.get("license-file").and_then(ini_str) {
        push_kv(ini, "licenseFile", &lf);
    }
    ini.push('\n');
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Build a [target.X] body. `path` is relative to pkg.dir; sources become a
/// glob over the root file's directory (relative to cwd) so module siblings
/// are visible to rustc's module resolution.
fn target_body(
    language: &str,
    ty: &str,
    pkg_dir: &Path,
    path: &str,
    edition: &str,
    report: &mut Report,
) -> String {
    let dir = Path::new(path).parent().unwrap_or_else(|| Path::new("."));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let full_dir = pkg_dir.join(dir);
    let rel_dir = full_dir.strip_prefix(&cwd).unwrap_or(&full_dir);
    let sources = if rel_dir.as_os_str().is_empty() {
        "*.rs".to_string()
    } else {
        format!("{}/*.rs", rel_dir.to_string_lossy())
    };
    if !pkg_dir.join(path).exists() {
        report.note(format!("target source not found: {}", pkg_dir.join(path).display()));
    }
    let mut body = String::new();
    body.push_str(&format!("language = {}\n", language));
    body.push_str(&format!("type = {}\n", ty));
    body.push_str(&format!("sources = {}\n", sources));
    if language == "rust" {
        body.push_str(&format!("edition = {}\n", edition));
    }
    body
}

/// Convert a cargo dependency table honoring workspace inheritance and
/// sibling-member detection. Returns (name, spec) pairs; spec is "path"
/// for workspace members.
fn cargo_dep_lines(
    deps: Option<&toml::value::Table>,
    ws_deps: Option<&toml::value::Table>,
    member_names: &[String],
    report: &mut Report,
    ctx: &str,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(table) = deps else { return out };
    for (name, spec) in table {
        match spec {
            toml::Value::String(v) => {
                out.push((name.clone(), v.clone()));
            }
            toml::Value::Table(t) => {
                // workspace = true → inherit root [workspace.dependencies] spec
                if t.get("workspace").and_then(|v| v.as_bool()) == Some(true) {
                    if let Some(root_spec) = ws_deps.and_then(|w| w.get(name)) {
                        let mut resolved = cargo_dep_lines(
                            Some(&toml::value::Table::from_iter([(name.clone(), root_spec.clone())])),
                            ws_deps,
                            member_names,
                            report,
                            ctx,
                        );
                        // Merge member-side extra features
                        if let Some(feats) = t.get("features").and_then(|f| f.as_array()) {
                            let extra: Vec<String> =
                                feats.iter().filter_map(|f| f.as_str().map(String::from)).collect();
                            if !extra.is_empty() {
                                if let Some(first) = resolved.first_mut() {
                                    if let Some((_, spec)) = first.1.split_once('|') {
                                        first.1 = format!("{}|{},{}", first.1.split('|').next().unwrap(), spec, extra.join(","));
                                    } else {
                                        first.1 = format!("{}|{}", first.1, extra.join(","));
                                    }
                                }
                            }
                        }
                        out.append(&mut resolved);
                    } else {
                        report.note(format!("{} dep '{}': workspace=true but not in [workspace.dependencies]", ctx, name));
                    }
                    continue;
                }
                // Sibling workspace member → path marker
                let pkg_name = t.get("package").and_then(ini_str).unwrap_or_else(|| name.clone());
                if member_names.iter().any(|m| *m == pkg_name || *m == *name) {
                    out.push((name.clone(), "path".to_string()));
                    continue;
                }
                if t.contains_key("git") {
                    report.note(format!("{} dep '{}': git source not supported by poly, skipped", ctx, name));
                    continue;
                }
                if t.contains_key("path") {
                    report.note(format!("{} dep '{}': non-workspace path dep not supported, skipped", ctx, name));
                    continue;
                }
                let version = t.get("version").and_then(ini_str).unwrap_or_else(|| "*".into());
                let mut meta = Vec::new();
                if let Some(feats) = t.get("features").and_then(|f| f.as_array()) {
                    meta.extend(feats.iter().filter_map(|f| f.as_str().map(String::from)));
                }
                if t.get("default-features").and_then(|v| v.as_bool()) == Some(false) {
                    report.note(format!("{} dep '{}': default-features=false not expressible in poly.ini", ctx, name));
                }
                if t.get("optional").and_then(|v| v.as_bool()) == Some(true) {
                    report.note(format!("{} dep '{}': optional dep migrated as required", ctx, name));
                }
                let emit_name = if pkg_name != *name { pkg_name } else { name.clone() };
                if emit_name != *name {
                    report.note(format!("{} dep '{}': renamed to package '{}'", ctx, name, emit_name));
                }
                out.push((emit_name, join_spec(&version, &meta)));
            }
            _ => {
                report.note(format!("{} dep '{}': unsupported spec form", ctx, name));
            }
        }
    }
    out.sort();
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

fn join_spec(version: &str, features: &[String]) -> String {
    if features.is_empty() {
        version.to_string()
    } else {
        format!("{}|{}", version, features.join(","))
    }
}



// ---------------------------------------------------------------------------
// go.mod migration
// ---------------------------------------------------------------------------

fn migrate_go_mod() -> Result<()> {
    let gomod = PathBuf::from("go.mod");
    if !gomod.exists() {
        anyhow::bail!("No go.mod found");
    }
    let content = std::fs::read_to_string(&gomod)?;
    let mut report = Report::new();
    let mut ini = String::from("[project]\n");

    let mut module = String::new();
    let mut go_version = String::new();
    let mut requires: Vec<(String, String)> = Vec::new();
    let mut in_block = false;

    for raw in content.lines() {
        let line = raw.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line == "require (" {
            in_block = true;
            continue;
        }
        if in_block && line == ")" {
            in_block = false;
            continue;
        }
        if let Some(rest) = line.strip_prefix("module ") {
            module = rest.trim().to_string();
            continue;
        }
        if let Some(rest) = line.strip_prefix("go ") {
            go_version = rest.trim().to_string();
            continue;
        }
        if let Some(rest) = line.strip_prefix("require ") {
            parse_go_require(rest, &mut requires);
            continue;
        }
        if in_block {
            parse_go_require(line, &mut requires);
            continue;
        }
        if line.starts_with("replace ") || line.starts_with("exclude ") || line.starts_with("retract ") {
            report.note(format!("go.mod directive not migrated: {}", line));
        }
    }

    let pkg_name = module.rsplit('/').next().unwrap_or("app").to_string();
    push_kv(&mut ini, "name", &pkg_name);
    push_kv(&mut ini, "version", "0.1.0");
    ini.push('\n');

    if !requires.is_empty() {
        ini.push_str("[dependencies.go]\n");
        for (name, ver) in &requires {
            ini.push_str(&format!("{} = {}\n", name, ver));
        }
        ini.push('\n');
    }

    // Go targets: every .go file tree becomes bin targets per directory
    // containing package main, or a single lib target otherwise.
    let mut targets = Vec::new();
    collect_go_targets(Path::new("."), &mut targets)?;
    for (id, sources) in &targets {
        ini.push_str(&format!("[target.{}]\n", id));
        ini.push_str("language = go\n");
        ini.push_str("type = bin\n");
        ini.push_str(&format!("sources = {}\n", ini_array(sources)));
        if !module.is_empty() {
            ini.push_str(&format!("module = {}\n", module));
        }
        ini.push('\n');
    }

    let ids: Vec<String> = targets.iter().map(|(id, _)| id.clone()).collect();
    ini.push_str("[build.default]\n");
    push_kv(&mut ini, "targets", &ini_array(&ids));
    ini.push('\n');

    if !go_version.is_empty() {
        report.note(format!("go directive version {} recorded (toolchain pinning via [toolchain])", go_version));
    }

    write_output(&ini, &report)?;
    tracing::info!("Migrated go.mod -> poly.ini (report: migration-report.txt)");
    Ok(())
}

fn parse_go_require(line: &str, out: &mut Vec<(String, String)>) {
    // forms: "name version" or "name version // indirect" or "name v1.2.3 => other v4"
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 2 {
        out.push((parts[0].to_string(), parts[1].to_string()));
    }
}

fn collect_go_targets(dir: &Path, out: &mut Vec<(String, Vec<String>)>) -> Result<()> {
    const SKIP: &[&str] = &["vendor", "node_modules", ".git", "dist", "target"];
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if SKIP.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            collect_go_targets(&path, out)?;
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("go") {
            continue;
        }
        if name.ends_with("_test.go") {
            continue;
        }
        // Check for package main
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        if !content.contains("package main") {
            continue;
        }
        // One target per directory of main-package files
        let dir_path = path.parent().unwrap_or(Path::new("."));
        let id = dir_path
            .strip_prefix(".")
            .unwrap_or(dir_path)
            .to_string_lossy()
            .replace('/', "-")
            .trim_start_matches('-')
            .to_string();
        let id = if id.is_empty() { "app".to_string() } else { sanitize(&id) };
        // Collect all non-test .go files in that dir
        let mut sources = Vec::new();
        for e in std::fs::read_dir(dir_path)? {
            let e = e?;
            let p = e.path();
            let n = e.file_name().to_string_lossy().to_string();
            if p.extension().and_then(|x| x.to_str()) == Some("go")
                && !n.ends_with("_test.go")
            {
                sources.push(p.to_string_lossy().to_string());
            }
        }
        sources.sort();
        // Merge if same dir seen twice
        if let Some(existing) = out.iter_mut().find(|(eid, _)| *eid == id) {
            existing.1 = sources;
        } else {
            out.push((id, sources));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// CMakeLists.txt migration
// ---------------------------------------------------------------------------

fn migrate_cmake() -> Result<()> {
    let cmake_path = PathBuf::from("CMakeLists.txt");
    if !cmake_path.exists() {
        anyhow::bail!("No CMakeLists.txt found");
    }
    let content = std::fs::read_to_string(&cmake_path)?;
    let mut report = Report::new();
    let mut ini = String::from("[project]\n");

    let mut name = "migrated".to_string();
    let mut version = "0.1.0".to_string();
    let mut targets: Vec<(String, String)> = Vec::new(); // (id, body)
    let mut global_includes: Vec<String> = Vec::new();

    // Join logical lines: cmake commands may span multiple physical lines
    let logical = join_cmake_lines(&content);
    for line in &logical {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(rest) = t.strip_prefix("cmake_minimum_required") {
            let _ = rest;
            continue;
        }
        if let Some(rest) = t.strip_prefix("project(") {
            let inner = rest.trim_end_matches(')').trim();
            let mut parts = inner.split_whitespace();
            if let Some(n) = parts.next() {
                name = n.to_string();
            }
            // project(name VERSION x.y.z LANGUAGES ...)
            let toks: Vec<&str> = inner.split_whitespace().collect();
            if let Some(i) = toks.iter().position(|t| *t == "VERSION") {
                if let Some(v) = toks.get(i + 1) {
                    version = v.to_string();
                }
            }
            continue;
        }
        for (func, ty) in [
            ("add_executable", "bin"),
            ("add_library", "static-lib"),
        ] {
            if t.starts_with(func) {
                if let Some(body) = cmake_target_body(t, func, ty, &mut report) {
                    targets.push(body);
                }
                break;
            }
        }
        if t.starts_with("include_directories") {
            if let Some(inner) = extract_parens(t) {
                for tok in inner.split_whitespace() {
                    global_includes.push(tok.to_string());
                }
            }
            continue;
        }
        if t.starts_with("find_package") || t.starts_with("find_library")
            || t.starts_with("find_path") || t.starts_with("add_subdirectory")
            || t.starts_with("set(") || t.starts_with("option(")
            || t.starts_with("if") || t.starts_with("endif") || t.starts_with("foreach")
            || t.starts_with("endforeach") || t.starts_with("else") || t.starts_with("elseif")
        {
            report.note(format!("cmake directive not migrated: {}", t.split('(').next().unwrap_or(t)));
            continue;
        }
        if !t.contains('(') {
            continue; // comments/blank-ish
        }
    }

    push_kv(&mut ini, "name", &name);
    push_kv(&mut ini, "version", &version);
    ini.push('\n');

    if !global_includes.is_empty() {
        ini.push_str("[dependencies.c]\n");
        for inc in &global_includes {
            ini.push_str(&format!("{} = system\n", inc));
        }
        ini.push('\n');
    }

    let mut ids = Vec::new();
    for (id, body) in &targets {
        ids.push(id.clone());
        ini.push_str(&format!("[target.{}]\n{}\n", id, body));
    }

    ini.push_str("[build.default]\n");
    push_kv(&mut ini, "targets", &ini_array(&ids));
    ini.push('\n');

    write_output(&ini, &report)?;
    tracing::info!("Migrated CMakeLists.txt -> poly.ini (report: migration-report.txt)");
    Ok(())
}

/// Join cmake physical lines into logical commands (paren-balanced).
fn join_cmake_lines(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for raw in content.lines() {
        // strip comments outside strings (good enough for migration)
        let line = raw.split('#').next().unwrap_or("");
        for c in line.chars() {
            if c == '(' {
                depth += 1;
            } else if c == ')' {
                depth -= 1;
            }
        }
        cur.push_str(line);
        cur.push(' ');
        if depth <= 0 {
            let t = cur.trim().to_string();
            if !t.is_empty() {
                out.push(t);
            }
            cur.clear();
            depth = 0;
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn extract_parens(s: &str) -> Option<String> {
    let start = s.find('(')? + 1;
    let end = s.rfind(')')?;
    if end <= start {
        return None;
    }
    Some(s[start..end].to_string())
}

fn cmake_target_body(
    line: &str,
    func: &str,
    ty: &str,
    report: &mut Report,
) -> Option<(String, String)> {
    let inner = extract_parens(line)?;
    let toks: Vec<&str> = inner.split_whitespace().collect();
    if toks.is_empty() {
        return None;
    }
    let name = toks[0].to_string();
    let mut idx = 1;
    // add_library(name STATIC|SHARED|OBJECT|MODULE|INTERFACE src...)
    let mut real_ty = ty.to_string();
    if func == "add_library" {
        if let Some(first) = toks.get(idx) {
            if ["STATIC", "SHARED", "OBJECT", "MODULE", "INTERFACE"].contains(first) {
                real_ty = match *first {
                    "SHARED" => "cdylib".to_string(),
                    "OBJECT" | "INTERFACE" | "MODULE" => "static-lib".to_string(),
                    _ => "static-lib".to_string(),
                };
                idx += 1;
            }
        }
    }
    let srcs: Vec<&str> = toks[idx..]
        .iter()
        .filter(|s| {
            let s = s.trim_end_matches(')');
            s.ends_with(".c") || s.ends_with(".cpp") || s.ends_with(".cc") || s.ends_with(".cxx")
        })
        .cloned()
        .collect();
    if srcs.is_empty() {
        report.note(format!("cmake target '{}': no sources found in command", name));
        return None;
    }
    // sources glob: common dir of sources or explicit list
    let mut body = String::new();
    body.push_str("language = c\n");
    body.push_str(&format!("type = {}\n", real_ty));
    // Emit explicit file list (comma-separated) - poly accepts globs too
    let src_list: Vec<String> = srcs.iter().map(|s| s.trim_end_matches(')').to_string()).collect();
    body.push_str(&format!("sources = {}\n", ini_array(&src_list)));
    Some((sanitize(&name), body))
}
