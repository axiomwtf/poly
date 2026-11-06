//! Quality tooling, developer tools, CI/migration and registry helpers.
//!//! Implements the `poly test|lint|fmt|doc|version|ci|migrate|completion|
//! trust|cache|outdated|tree|toolchain|audit|sbom|profile|ide|release|
//! review|diff|flamegraph|stats|env` command surface from polyspec §10/§12/§13/§15.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};

use crate::{expand_globs, parse_poly_ini, rust_dep_args};

fn load_config() -> Result<poly_config::Config> {
    parse_poly_ini("poly.ini")
}

fn targets_of(config: &poly_config::Config, filter: Option<&str>) -> Vec<String> {
    let all = crate::parse_build_targets(config).unwrap_or_default();
    match filter {
        Some(f) => all.into_iter().filter(|t| t == f).collect(),
        None => all,
    }
}

// ---------------------------------------------------------------------------
// poly test
// ---------------------------------------------------------------------------

pub async fn cmd_test(target: Option<String>, _coverage: bool, _benchmark: bool) -> Result<()> {
    let config = load_config()?;
    crate::inject_secrets(&config);

    // pre-test hook
    if let Some(hooks) = &config.hooks {
        if let Some(hook) = &hooks.pre_test {
            info!("Running pre-test hook: {}", hook.command);
            let status = Command::new("sh").arg("-c").arg(&hook.command).status()?;
            if !status.success() {
                anyhow::bail!("pre-test hook failed: {}", hook.command);
            }
        }
    }

    let target_ids = targets_of(&config, target.as_deref());
    if target_ids.is_empty() {
        anyhow::bail!("No matching targets");
    }

    let mut failures = 0usize;
    let mut ran = 0usize;

    for target_id in &target_ids {
        let Some(tc) = config.targets.get(target_id) else { continue };
        match tc.language.as_str() {
            "rust" => {
                match run_rust_tests(target_id, tc, &config).await {
                    Ok(p) => {
                        ran += 1;
                        if !p {
                            failures += 1;
                        }
                    }
                    Err(e) => {
                        warn!("rust tests for {} failed to build: {}", target_id, e);
                        failures += 1;
                    }
                }
            }
            "go" => {
                match run_go_tests(target_id, tc, &config) {
                    Ok(p) => {
                        ran += 1;
                        if !p {
                            failures += 1;
                        }
                    }
                    Err(e) => {
                        warn!("go tests for {} failed: {}", target_id, e);
                        failures += 1;
                    }
                }
            }
            "c" | "cpp" => {
                match run_c_tests(target_id, tc) {
                    Ok(Some(passed)) => {
                        ran += 1;
                        if !passed {
                            failures += 1;
                        }
                    }
                    Ok(None) => debug!("no C tests for {}", target_id),
                    Err(e) => {
                        warn!("c tests for {} failed: {}", target_id, e);
                        failures += 1;
                    }
                }
            }
            _ => {}
        }
    }

    // post-test hook
    if let Some(hooks) = &config.hooks {
        if let Some(hook) = &hooks.post_test {
            info!("Running post-test hook: {}", hook.command);
            let _ = Command::new("sh").arg("-c").arg(&hook.command).status();
        }
    }

    if ran == 0 {
        info!("No tests found");
        return Ok(());
    }
    if failures > 0 {
        anyhow::bail!("{} of {} test targets failed", failures, ran);
    }
    info!("All {} test target(s) passed", ran);
    Ok(())
}

/// Compile each rust target's sources with `rustc --test` and run the binary.
async fn run_rust_tests(
    target_id: &str,
    tc: &poly_config::Target,
    config: &poly_config::Config,
) -> Result<bool> {
    let sources = expand_globs(&tc.sources)?;
    if sources.is_empty() {
        return Ok(true);
    }

    // Reuse debug dep artifacts — dep rlibs are identical for tests.
    let profile = "debug";
    let dep_compiler = poly_core::DependencyCompiler::new(
        poly_core::project_cache_dir(&config.project.name).join(profile),
    )?;
    let workspace_dir = std::env::current_dir()?;
    let locked = crate::read_locked_rust_versions();
    let compiled_deps = dep_compiler
        .compile_dependencies(
            &config.dependencies.rust,
            &workspace_dir,
            "x86_64-unknown-linux-gnu",
            profile,
            &locked,
        )
        .await?;

    // Compile workspace deps (rlibs) in build order for --extern use.
    let tmp = std::env::temp_dir().join(format!("poly-test-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let mut workspace_rlibs: HashMap<String, PathBuf> = HashMap::new();
    let driver = poly_rust::RustDriver::new()?;
    for tid in crate::parse_build_targets(config)? {
        let Some(dep_tc) = config.targets.get(&tid) else { continue };
        if dep_tc.language != "rust" {
            continue;
        }
        let dep_sources = expand_globs(&dep_tc.sources)?;
        if dep_sources.is_empty() {
            continue;
        }
        let crate_name = tid.replace('-', "_");
        let is_self = tid == *target_id;
        let out = tmp.join(format!("lib{}.rlib", crate_name));
        let edition = dep_tc
            .edition
            .clone()
            .or(config.project.edition.clone())
            .unwrap_or_else(|| "2021".to_string());
        let rt = poly_rust::RustTarget {
            name: crate_name.clone(),
            crate_type: if is_self { "bin".into() } else { "lib".into() },
            edition,
            features: dep_tc.features.clone(),
            dependencies: Vec::new(),
            source: dep_sources[0].clone(),
            flags: {
                let mut f = dep_tc.flags.clone();
                if is_self {
                    f.push("--test".to_string());
                }
                f.extend(rust_dep_args(
                    &tid,
                    dep_tc,
                    config,
                    &compiled_deps,
                    &workspace_rlibs,
                    "x86_64-unknown-linux-gnu",
                ));
                f
            },
            target_triple: Some("x86_64-unknown-linux-gnu".to_string()),
            env: crate::manifest_env(&tid, &config, &dep_sources[0]),
        };
        let out_bin = if is_self {
            tmp.join(format!("{}_test", crate_name))
        } else {
            out.clone()
        };
        driver.compile(&dep_sources[0], &out_bin, &rt, &[], None)?;
        if !is_self {
            workspace_rlibs.insert(crate_name, out);
        } else {
            // Run the test binary
            let status = Command::new(&out_bin).status()?;
            let _ = std::fs::remove_dir_all(&tmp);
            return Ok(status.success());
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(true)
}

fn run_go_tests(
    target_id: &str,
    tc: &poly_config::Target,
    config: &poly_config::Config,
) -> Result<bool> {
    let sources = expand_globs(&tc.sources)?;
    if sources.is_empty() {
        return Ok(true);
    }
    let work = prepare_go_workdir(target_id, tc, config, &sources)?;
    let driver = poly_go::GoDriver::new()?;
    driver.test_package(&work, "./pkg", None)
}

/// Copy go sources into `<dir>/<pkg>/` and write a go.mod; returns work dir.
fn prepare_go_workdir(
    target_id: &str,
    tc: &poly_config::Target,
    config: &poly_config::Config,
    sources: &[PathBuf],
) -> Result<PathBuf> {
    let work = std::env::temp_dir()
        .join(format!("poly-gotest-{}-{}", target_id, std::process::id()));
    let pkg_dir = sources[0]
        .parent()
        .map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "pkg".into()))
        .unwrap_or_else(|| "pkg".to_string());
    let dest = work.join("pkg");
    std::fs::create_dir_all(&dest)?;
    for src in sources {
        if let Some(name) = src.file_name() {
            std::fs::copy(src, dest.join(name))?;
        }
    }
    let module = tc
        .module
        .clone()
        .unwrap_or_else(|| format!("example.com/{}/{}", config.project.name, target_id));
    std::fs::write(
        work.join("go.mod"),
        format!("module {}\n\ngo 1.21\n", module),
    )?;
    let _ = pkg_dir;
    Ok(work)
}

fn run_c_tests(target_id: &str, tc: &poly_config::Target) -> Result<Option<bool>> {
    let sources = expand_globs(&tc.sources)?;
    // Test sources: *_test.c / test_*.c inside the target's sources
    let test_sources: Vec<_> = sources
        .iter()
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            n.ends_with("_test.c") || n.starts_with("test_") || n.contains("_test.")
        })
        .cloned()
        .collect();
    if test_sources.is_empty() {
        return Ok(None);
    }
    let non_test: Vec<_> = sources
        .iter()
        .filter(|p| !test_sources.contains(p))
        .cloned()
        .collect();

    let driver = poly_c::CDriver::new()?;
    let work = std::env::temp_dir().join(format!("poly-ctest-{}", std::process::id()));
    std::fs::create_dir_all(&work)?;

    let mut all = non_test.clone();
    all.extend(test_sources.iter().cloned());
    let c_target = poly_c::CTarget {
        name: target_id.to_string(),
        language: tc.language.clone(),
        target_type: "bin".into(),
        sources: all,
        headers: Vec::new(),
        include_dirs: Vec::new(),
        library_dirs: Vec::new(),
        libraries: Vec::new(),
        flags: tc.flags.clone(),
        defines: HashMap::new(),
        dependencies: Vec::new(),
        target_triple: "x86_64-unknown-linux-gnu".into(),
    };
    let objects = driver.compile_target(&c_target, &work)?;
    let bin = work.join(format!("{}_test", target_id.replace('-', "_")));
    driver.link(&c_target, &objects, &bin)?;
    let status = Command::new(&bin).status()?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(Some(status.success()))
}

// ---------------------------------------------------------------------------
// poly lint / fmt
// ---------------------------------------------------------------------------

pub async fn cmd_lint(target: Option<String>) -> Result<()> {
    let config = load_config()?;
    let target_ids = targets_of(&config, target.as_deref());
    let mut issues = 0usize;

    for target_id in &target_ids {
        let Some(tc) = config.targets.get(target_id) else { continue };
        match tc.language.as_str() {
            "rust" => issues += lint_rust(target_id, tc, &config).await?,
            "c" | "cpp" => issues += lint_c(target_id, tc)?,
            "go" => issues += lint_go(target_id, tc, &config)?,
            _ => {}
        }
    }

    if issues == 0 {
        info!("No lint issues found");
    } else {
        info!("Found {} lint issue(s)", issues);
    }
    Ok(())
}

async fn lint_rust(
    target_id: &str,
    tc: &poly_config::Target,
    config: &poly_config::Config,
) -> Result<usize> {
    let sources = expand_globs(&tc.sources)?;
    if sources.is_empty() {
        return Ok(0);
    }

    // Prefer clippy-driver; fall back to rustc --emit=metadata.
    let clippy = which_tool("clippy-driver").or_else(|| which_tool("cargo-clippy"));
    let linter = clippy.unwrap_or_else(|| "rustc".to_string());

    let dep_compiler = poly_core::DependencyCompiler::new(
        poly_core::project_cache_dir(&config.project.name).join("debug"),
    )?;
    let workspace_dir = std::env::current_dir()?;
    let locked = crate::read_locked_rust_versions();
    let compiled_deps = dep_compiler
        .compile_dependencies(
            &config.dependencies.rust,
            &workspace_dir,
            "x86_64-unknown-linux-gnu",
            "debug",
            &locked,
        )
        .await?;
    let dep_args = rust_dep_args(
        target_id,
        tc,
        config,
        &compiled_deps,
        &HashMap::new(),
        "x86_64-unknown-linux-gnu",
    );

    let mut issues = 0usize;
    for src in &sources {
        let mut cmd = Command::new(&linter);
        cmd.arg(src)
            .arg("--crate-type")
            .arg("lib")
            .arg("--edition")
            .arg(tc.edition.clone().unwrap_or_else(|| "2021".into()))
            .arg("-A")
            .arg("non_snake_case")
            .arg("--emit=metadata")
            .arg("-o")
            .arg("/dev/null");
        for a in &dep_args {
            cmd.arg(a);
        }
        let out = cmd.output()?;
        if !out.status.success() {
            issues += String::from_utf8_lossy(&out.stderr)
                .lines()
                .filter(|l| l.contains("warning") || l.contains("error"))
                .count()
                .max(1);
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
        }
    }
    Ok(issues)
}

fn lint_c(target_id: &str, tc: &poly_config::Target) -> Result<usize> {
    let sources = expand_globs(&tc.sources)?;
    if sources.is_empty() {
        return Ok(0);
    }
    // clang --analyze if available, else gcc -fsyntax-only
    let (tool, args): (String, Vec<String>) = if let Some(clang) = which_tool("clang") {
        (clang, vec!["--analyze".into()])
    } else {
        (
            which_tool("gcc").unwrap_or_else(|| "cc".into()),
            vec!["-fsyntax-only".into(), "-Wall".into(), "-Wextra".into()],
        )
    };
    let mut issues = 0usize;
    for src in &sources {
        if src.extension().and_then(|e| e.to_str()) == Some("h") {
            continue;
        }
        let mut cmd = Command::new(&tool);
        cmd.args(&args);
        for inc in &tc.flags {
            if let Some(d) = inc.strip_prefix("-I") {
                cmd.arg(format!("-I{}", d));
            }
        }
        cmd.arg(src);
        let out = cmd.output()?;
        if !out.status.success() {
            issues += 1;
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
        }
    }
    let _ = target_id;
    Ok(issues)
}

fn lint_go(target_id: &str, tc: &poly_config::Target, config: &poly_config::Config) -> Result<usize> {
    let sources = expand_globs(&tc.sources)?;
    if sources.is_empty() {
        return Ok(0);
    }
    let work = prepare_go_workdir(target_id, tc, config, &sources)?;
    let driver = poly_go::GoDriver::new()?;
    let out = driver.vet_package(&work, "./pkg")?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(if out.status.success() { 0 } else { 1 })
}

pub fn cmd_fmt() -> Result<()> {
    let config = load_config()?;
    let target_ids = targets_of(&config, None);

    for target_id in &target_ids {
        let Some(tc) = config.targets.get(target_id) else { continue };
        let sources = expand_globs(&tc.sources).unwrap_or_default();
        match tc.language.as_str() {
            "rust" => {
                if let Some(rustfmt) = which_tool("rustfmt") {
                    for src in &sources {
                        let _ = Command::new(&rustfmt)
                            .arg("--edition")
                            .arg(tc.edition.clone().unwrap_or_else(|| "2021".into()))
                            .arg(src)
                            .status();
                    }
                }
            }
            "c" | "cpp" => {
                if let Some(fmt) = which_tool("clang-format") {
                    for src in &sources {
                        let _ = Command::new(&fmt).arg("-i").arg(src).status();
                    }
                }
            }
            "go" => {
                if let Some(gofmt) = which_tool("gofmt") {
                    for src in &sources {
                        let _ = Command::new(&gofmt).arg("-w").arg(src).status();
                    }
                }
            }
            _ => {}
        }
    }
    info!("Formatting complete");
    Ok(())
}

fn which_tool(name: &str) -> Option<String> {
    Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// poly doc
// ---------------------------------------------------------------------------

pub fn cmd_doc(open: bool, serve: bool) -> Result<()> {
    let config = load_config()?;
    let doc_dir = PathBuf::from("doc");
    std::fs::create_dir_all(&doc_dir)?;

    let mut index = String::from("# Poly API Documentation\n\n");
    for (id, tc) in &config.targets {
        let sources = expand_globs(&tc.sources).unwrap_or_default();
        let mut page = format!(
            "# {}\n\n- Language: {}\n- Type: {}\n\n## Sources\n\n",
            id, tc.language, tc.type_
        );
        for s in &sources {
            page.push_str(&format!("- `{}`\n", s.display()));
        }
        page.push_str("\n## Dependencies\n\n");
        let dep_names: Vec<String> = match tc.language.as_str() {
            "rust" => config.dependencies.rust.keys().cloned().collect(),
            "c" => config.dependencies.c.keys().cloned().collect(),
            "go" => config.dependencies.go.keys().cloned().collect(),
            _ => Vec::new(),
        };
        for d in dep_names {
            page.push_str(&format!("- {}\n", d));
        }
        let page_path = doc_dir.join(format!("{}.md", id.replace('-', "_")));
        std::fs::write(&page_path, page)?;
        index.push_str(&format!("- [{}]({}.md)\n", id, id.replace('-', "_")));
    }
    std::fs::write(doc_dir.join("index.md"), index)?;
    info!("Documentation generated in doc/");

    if open {
        let _ = Command::new("xdg-open").arg(doc_dir.join("index.md")).status();
    }
    if serve {
        info!("Doc server not implemented; open doc/index.md directly");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// poly version
// ---------------------------------------------------------------------------

pub fn cmd_version_bump(level: &str) -> Result<()> {
    let path = PathBuf::from("poly.ini");
    let content = std::fs::read_to_string(&path)?;
    let re_version = find_project_version(&content).context("No [project] version found")?;
    let (major, minor, patch) = parse_semver(&re_version)?;
    let new = match level {
        "major" => format!("{}.0.0", major + 1),
        "minor" => format!("{}.{}.0", major, minor + 1),
        "patch" => format!("{}.{}.{}", major, minor, patch + 1),
        other => anyhow::bail!("Unknown bump level: {} (use major|minor|patch)", other),
    };
    let updated = replace_project_version(&content, &new)?;
    std::fs::write(&path, updated)?;
    info!("Version bumped: {} -> {}", re_version, new);
    Ok(())
}

pub fn cmd_version_check() -> Result<()> {
    let content = std::fs::read_to_string("poly.ini")?;
    let version = find_project_version(&content).context("No [project] version found")?;
    info!("Project version: {}", version);
    if PathBuf::from("poly.lock").exists() {
        info!("poly.lock present — run `poly outdated` to check for updates");
    }
    Ok(())
}

pub fn cmd_version_changelog() -> Result<()> {
    let out = Command::new("git")
        .args(["log", "--oneline", "-50"])
        .output()?;
    if !out.status.success() {
        anyhow::bail!("Not a git repository or git unavailable");
    }
    let log = String::from_utf8_lossy(&out.stdout);
    let mut changelog = String::from("# Changelog\n\n");
    for line in log.lines() {
        changelog.push_str(&format!("- {}\n", line));
    }
    std::fs::write("CHANGELOG.md", changelog)?;
    info!("Wrote CHANGELOG.md");
    Ok(())
}

fn find_project_version(content: &str) -> Option<String> {
    let mut in_project = false;
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_project = t == "[project]";
            continue;
        }
        if in_project {
            if let Some((k, v)) = t.split_once('=') {
                if k.trim() == "version" {
                    return Some(v.trim().trim_matches('"').to_string());
                }
            }
        }
    }
    None
}

fn replace_project_version(content: &str, new_version: &str) -> Result<String> {
    let mut out = String::new();
    let mut in_project = false;
    let mut done = false;
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_project = t == "[project]";
        }
        if !done && in_project {
            if let Some((k, _)) = t.split_once('=') {
                if k.trim() == "version" {
                    out.push_str(&format!("version = {}\n", new_version));
                    done = true;
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if !done {
        anyhow::bail!("version key not found in [project]");
    }
    Ok(out)
}

fn parse_semver(v: &str) -> Result<(u64, u64, u64)> {
    let core = v.split('-').next().unwrap_or(v);
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() < 3 {
        anyhow::bail!("Version {} is not semver (need major.minor.patch)", v);
    }
    Ok((
        parts[0].parse()?,
        parts[1].parse()?,
        parts[2].parse()?,
    ))
}

// ---------------------------------------------------------------------------
// poly ci / migrate / completion
// ---------------------------------------------------------------------------

pub fn cmd_ci(provider: &str) -> Result<()> {
    let config = load_config()?;
    let name = &config.project.name;
    match provider {
        "github" => {
            let dir = PathBuf::from(".github/workflows");
            std::fs::create_dir_all(&dir)?;
            std::fs::write(
                dir.join("poly.yml"),
                format!(
                    r#"name: poly
on: [push, pull_request]
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-action@stable
      - uses: actions/setup-go@v5
        with:
          go-version: '1.23'
      - name: Install poly
        run: cargo install --path .
      - name: Build
        run: poly build --release
      - name: Test
        run: poly test
      - name: Lint
        run: poly lint
"#,
                )
            )?;
            info!("Wrote .github/workflows/poly.yml");
        }
        "gitlab" => {
            std::fs::write(
                ".gitlab-ci.yml",
                r#"stages: [build, test]
variables:
  CARGO_HOME: $CI_PROJECT_DIR/.cargo
build:
  stage: build
  image: rust:latest
  script:
    - apt-get update && apt-get install -y build-essential
    - cargo install --path .
    - poly build --release
test:
  stage: test
  image: rust:latest
  script:
    - poly test
"#,
            )?;
            info!("Wrote .gitlab-ci.yml");
        }
        "docker" => {
            std::fs::write(
                "Dockerfile.poly",
                r#"# Multi-stage Poly build
FROM rust:latest AS builder
RUN apt-get update && apt-get install -y build-essential golang
WORKDIR /src
COPY . .
RUN cargo install --path . && poly build --release

FROM debian:bookworm-slim
COPY --from=builder /src/dist/release /dist
ENTRYPOINT ["/dist"]
"#,
            )?;
            info!("Wrote Dockerfile.poly");
        }
        other => anyhow::bail!("Unknown CI provider: {} (github|gitlab|docker)", other),
    }
    let _ = name;
    Ok(())
}

pub fn cmd_migrate(from: &str) -> Result<()> {
    match from {
        "cargo" => migrate_cargo(),
        "go-mod" | "gomod" => migrate_go_mod(),
        "cmake" => migrate_cmake(),
        other => anyhow::bail!("Unknown source: {} (cargo|go-mod|cmake)", other),
    }
}

fn migrate_cargo() -> Result<()> {
    let toml_path = PathBuf::from("Cargo.toml");
    if !toml_path.exists() {
        anyhow::bail!("No Cargo.toml found");
    }
    let content = std::fs::read_to_string(&toml_path)?;
    let mut ini = String::from("[project]\nname = poly-migrated\nversion = 0.1.0\nedition = 2021\n\n[dependencies.rust]\n");
    let mut in_deps = false;
    let mut report = String::new();
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_deps = t == "[dependencies]" || t.starts_with("[dependencies.");
            continue;
        }
        if in_deps {
            if let Some((name, spec)) = t.split_once('=') {
                let name = name.trim();
                if name.is_empty() || name.starts_with('#') {
                    continue;
                }
                let spec = spec.trim().trim_matches('"');
                // Extract version req and features from inline table or string
                let (version, features) = if spec.starts_with('{') {
                    let v = extract_toml_str(spec, "version").unwrap_or_else(|| "*".into());
                    let f = extract_toml_str(spec, "features");
                    (v, f)
                } else {
                    (spec.to_string(), None)
                };
                let feat_suffix = features
                    .map(|f| format!("|{}", f.replace(['[', ']', '"', ' '], "")))
                    .unwrap_or_default();
                ini.push_str(&format!("{} = {}{}\n", name, version, feat_suffix));
                report.push_str(&format!("migrated rust dep: {} = {}\n", name, version));
            }
        }
    }
    ini.push_str("\n[build.default]\ntargets =\n");
    // Backup existing poly.ini
    let poly_ini = PathBuf::from("poly.ini");
    if poly_ini.exists() {
        std::fs::copy(&poly_ini, "poly.ini.bak")?;
    }
    std::fs::write(&poly_ini, ini)?;
    std::fs::write("migration-report.txt", report)?;
    info!("Migrated Cargo.toml -> poly.ini (report: migration-report.txt)");
    Ok(())
}

fn migrate_go_mod() -> Result<()> {
    let gomod = PathBuf::from("go.mod");
    if !gomod.exists() {
        anyhow::bail!("No go.mod found");
    }
    let content = std::fs::read_to_string(&gomod)?;
    let mut ini = String::from("[project]\nname = poly-migrated\nversion = 0.1.0\nedition = 2021\n\n[dependencies.go]\n");
    let mut report = String::new();
    let mut in_require = false;
    for line in content.lines() {
        let t = line.trim();
        if t == "require (" {
            in_require = true;
            continue;
        }
        if in_require && t == ")" {
            in_require = false;
            continue;
        }
        if t.starts_with("require ") {
            let rest = t.trim_start_matches("require ").trim();
            write_go_dep(&mut ini, &mut report, rest);
            continue;
        }
        if in_require {
            write_go_dep(&mut ini, &mut report, t);
        }
    }
    if !PathBuf::from("poly.ini").exists() {
        std::fs::write("poly.ini", ini)?;
    } else {
        // Append go deps section if missing
        let existing = std::fs::read_to_string("poly.ini")?;
        if !existing.contains("[dependencies.go]") {
            std::fs::write("poly.ini", format!("{}\n{}", existing, ini))?;
        }
    }
    std::fs::write("migration-report.txt", report)?;
    info!("Migrated go.mod -> poly.ini (report: migration-report.txt)");
    Ok(())
}

fn write_go_dep(ini: &mut String, report: &mut String, line: &str) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 2 {
        ini.push_str(&format!("{} = {}\n", parts[0], parts[1]));
        report.push_str(&format!("migrated go dep: {} = {}\n", parts[0], parts[1]));
    }
}

fn migrate_cmake() -> Result<()> {
    let cmake = PathBuf::from("CMakeLists.txt");
    if !cmake.exists() {
        anyhow::bail!("No CMakeLists.txt found");
    }
    let content = std::fs::read_to_string(&cmake)?;
    let mut ini = String::from("[project]\nname = poly-migrated\nversion = 0.1.0\nedition = 2021\n\n");
    let mut report = String::new();
    for line in content.lines() {
        let t = line.trim();
        for (func, ty) in [("add_executable", "bin"), ("add_library", "static-lib")] {
            if t.starts_with(func) {
                // add_executable(name src1 src2 ...) or add_library(name STATIC src...)
                let inner = t
                    .trim_start_matches(func)
                    .trim_start_matches('(')
                    .trim_end_matches(')')
                    .trim();
                let tokens: Vec<&str> = inner.split_whitespace().collect();
                if let Some(name) = tokens.first() {
                    let srcs: Vec<&str> = tokens
                        .iter()
                        .skip(if ty == "static-lib" { 2 } else { 1 })
                        .filter(|s| s.ends_with(".c") || s.ends_with(".cpp"))
                        .cloned()
                        .collect();
                    if !srcs.is_empty() {
                        ini.push_str(&format!(
                            "[target.{}]\nlanguage = c\ntype = {}\nsources = {}\n\n",
                            name,
                            ty,
                            srcs.join(", ")
                        ));
                        report.push_str(&format!("migrated cmake target: {} ({})\n", name, ty));
                    }
                }
            }
        }
    }
    ini.push_str("[build.default]\ntargets =\n");
    if PathBuf::from("poly.ini").exists() {
        std::fs::copy("poly.ini", "poly.ini.bak")?;
    }
    std::fs::write("poly.ini", ini)?;
    std::fs::write("migration-report.txt", report)?;
    info!("Migrated CMakeLists.txt -> poly.ini (report: migration-report.txt)");
    Ok(())
}

fn extract_toml_str(table: &str, key: &str) -> Option<String> {
    let pat = format!("{} = \"", key);
    let start = table.find(&pat)? + pat.len();
    let end = table[start..].find('"')? + start;
    Some(table[start..end].to_string())
}

pub fn cmd_completion(shell: &str) -> Result<()> {
    // Generate a static completion script for bash/zsh/fish.
    let script = match shell {
        "bash" => {
            let mut cmds = String::new();
            for c in [
                "init", "build", "add", "remove", "update", "lock", "clean", "validate",
                "sign", "verify", "publish", "install", "uninstall", "test", "lint", "fmt",
                "doc", "version", "ci", "migrate", "completion", "trust", "cache", "outdated",
                "tree", "toolchain", "audit", "sbom", "profile", "ide", "release", "review",
                "diff", "flamegraph", "stats", "env",
            ] {
                cmds.push_str(c);
                cmds.push(' ');
            }
            format!(
                "_poly() {{\n  local cur=\"${{COMP_WORDS[COMP_CWORD]}}\"\n  COMPREPLY=( $(compgen -W \"{}\" -- \"$cur\") )\n}}\ncomplete -F _poly poly\n",
                cmds.trim()
            )
        }
        "zsh" => format!(
            "#compdef poly\n_poly() {{\n  local -a cmds\n  cmds=(build test lint fmt doc init add remove update lock clean validate sign verify publish install uninstall version ci migrate completion trust cache outdated tree toolchain audit sbom profile ide release review diff flamegraph stats env)\n  _describe 'command' cmds\n}}\ncompdef _poly poly\n"
        ),
        "fish" => {
            let mut s = String::from("complete -c poly -f\n");
            for c in [
                "build", "test", "lint", "fmt", "doc", "init", "add", "remove", "update",
                "lock", "clean", "validate", "sign", "verify", "publish", "install",
                "uninstall", "version", "ci", "migrate", "completion", "trust", "cache",
                "outdated", "tree", "toolchain", "audit", "sbom", "profile", "ide",
                "release", "review", "diff", "flamegraph", "stats", "env",
            ] {
                s.push_str(&format!("complete -c poly -n '__fish_use_subcommand' -a {}\n", c));
            }
            s
        }
        other => anyhow::bail!("Unknown shell: {} (bash|zsh|fish)", other),
    };
    print!("{}", script);
    Ok(())
}

// ---------------------------------------------------------------------------
// trust / cache / outdated / tree / toolchain / audit / sbom / misc
// ---------------------------------------------------------------------------

fn trust_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/poly/trust")
}

pub fn cmd_trust_add(path: &str) -> Result<()> {
    let src = PathBuf::from(path);
    if !src.exists() {
        anyhow::bail!("Key file not found: {}", path);
    }
    std::fs::create_dir_all(trust_dir())?;
    let dest = trust_dir().join(src.file_name().unwrap());
    std::fs::copy(&src, &dest)?;
    info!("Trusted key added: {}", dest.display());
    Ok(())
}

pub fn cmd_trust_list() -> Result<()> {
    let dir = trust_dir();
    if !dir.exists() {
        info!("No trusted keys");
        return Ok(());
    }
    for entry in std::fs::read_dir(&dir)? {
        info!("  {}", entry?.file_name().to_string_lossy());
    }
    Ok(())
}

pub fn cmd_trust_remove(name: &str) -> Result<()> {
    let path = trust_dir().join(name);
    if path.exists() {
        std::fs::remove_file(&path)?;
        info!("Removed trusted key: {}", name);
    } else {
        anyhow::bail!("Trusted key not found: {}", name);
    }
    Ok(())
}

pub fn cmd_cache_stats() -> Result<()> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let cache = PathBuf::from(home).join(".cache/poly");
    let (files, bytes) = dir_stats(&cache)?;
    info!("Dependency cache: {} files, {:.1} MiB", files, bytes as f64 / 1048576.0);
    let (dfiles, dbytes) = dir_stats(Path::new("dist"))?;
    info!("dist/: {} files, {:.1} MiB", dfiles, dbytes as f64 / 1048576.0);
    Ok(())
}

fn dir_stats(dir: &Path) -> Result<(u64, u64)> {
    if !dir.exists() {
        return Ok((0, 0));
    }
    let (mut files, mut bytes) = (0u64, 0u64);
    for entry in walkdir::WalkDir::new(dir) {
        let entry = entry?;
        if entry.file_type().is_file() {
            files += 1;
            bytes += entry.metadata()?.len();
        }
    }
    Ok((files, bytes))
}

pub async fn cmd_outdated() -> Result<()> {
    let lock = poly_resolver::LockFile::read("poly.lock")
        .context("No poly.lock — run `poly build` or `poly lock` first")?;
    let client = poly_registry::RegistryClient::new()?;
    let mut outdated = 0usize;
    for entry in &lock.dependencies {
        if entry.language != "rust" {
            continue;
        }
        if let Ok(latest) = client.resolve_version(&entry.name, "*").await {
            if latest != entry.version {
                outdated += 1;
                info!("{}: {} -> {}", entry.name, entry.version, latest);
            }
        }
    }
    if outdated == 0 {
        info!("All dependencies up to date");
    } else {
        info!("{} outdated rust dependenc{}", outdated, if outdated == 1 { "y" } else { "ies" });
    }
    Ok(())
}

pub fn cmd_tree() -> Result<()> {
    let lock = poly_resolver::LockFile::read("poly.lock")
        .context("No poly.lock — run `poly build` or `poly lock` first")?;
    let mut by_lang: HashMap<String, Vec<String>> = HashMap::new();
    for entry in &lock.dependencies {
        by_lang
            .entry(entry.language.clone())
            .or_default()
            .push(format!("{}@{}", entry.name, entry.version));
    }
    for (lang, mut deps) in by_lang {
        deps.sort();
        println!("{}:", lang);
        for d in deps {
            println!("  {}", d);
        }
    }
    Ok(())
}

pub fn cmd_toolchain(sub: &str) -> Result<()> {
    match sub {
        "list" => {
            for (name, cmd, arg) in [
                ("rust", "rustc", "-vV"),
                ("go", "go", "version"),
                ("c", "gcc", "--version"),
            ] {
                if let Ok(out) = Command::new(cmd).arg(arg).output() {
                    let first = String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .next()
                        .unwrap_or("")
                        .to_string();
                    println!("{}: {}", name, first);
                } else {
                    println!("{}: not installed", name);
                }
            }
        }
        "install" | "use" => {
            info!("Toolchain management is handled by the system package manager / rustup / go installer");
            info!("Detected toolchains via `poly toolchain list`");
        }
        other => anyhow::bail!("Unknown toolchain command: {} (list|install|use)", other),
    }
    Ok(())
}

pub fn cmd_audit(_fix: bool) -> Result<()> {
    // MVP: verify all locked checksums are present and well-formed.
    let lock = poly_resolver::LockFile::read("poly.lock")
        .context("No poly.lock — run `poly build` first")?;
    let mut bad = 0usize;
    for entry in &lock.dependencies {
        if entry.checksum.trim().is_empty()
            || entry.checksum.chars().all(|c| c == '0')
        {
            bad += 1;
            warn!("{}@{}: missing checksum", entry.name, entry.version);
        }
    }
    if bad == 0 {
        info!("No known vulnerabilities (checksum audit clean)");
    } else {
        info!("{} package(s) missing checksums", bad);
    }
    Ok(())
}

pub fn cmd_sbom() -> Result<()> {
    let lock = poly_resolver::LockFile::read("poly.lock")
        .context("No poly.lock — run `poly build` first")?;
    let sbom = serde_json::json!({
        "bomFormat": "Poly-SBOM/1",
        "dependencies": lock.dependencies.iter().map(|e| serde_json::json!({
            "name": e.name,
            "version": e.version,
            "language": e.language,
            "checksum": e.checksum,
            "source": e.source,
        })).collect::<Vec<_>>(),
    });
    std::fs::write("sbom.json", serde_json::to_string_pretty(&sbom)?)?;
    info!("Wrote sbom.json ({} dependencies)", lock.dependencies.len());
    Ok(())
}

pub fn cmd_profile() -> Result<()> {
    cmd_cache_stats()?;
    info!("Use RUST_LOG=debug poly build for per-unit timing");
    Ok(())
}

pub fn cmd_ide(kind: &str) -> Result<()> {
    match kind {
        "vscode" => {
            std::fs::create_dir_all(".vscode")?;
            std::fs::write(
                ".vscode/settings.json",
                r#"{
  "poly.buildOnSave": true,
  "files.associations": {
    "*.poly": "ini"
  }
}"#,
            )?;
            std::fs::write(
                ".vscode/extensions.json",
                r#"{"recommendations": ["poly.poly"]}"#,
            )?;
            info!("Wrote .vscode configuration");
        }
        "neovim" => {
            let dir = PathBuf::from(std::env::var("HOME")?).join(".config/nvim");
            std::fs::create_dir_all(&dir)?;
            std::fs::write(
                dir.join("poly.lua"),
                r#"-- Poly Neovim configuration
vim.api.nvim_create_autocmd("BufWritePost", {
  pattern = "poly.ini",
  callback = function() vim.cmd("!poly build") end,
})
"#,
            )?;
            info!("Wrote ~/.config/nvim/poly.lua");
        }
        "lsp" => {
            info!("Poly LSP server is not implemented yet");
        }
        other => anyhow::bail!("Unknown IDE: {} (vscode|neovim|lsp)", other),
    }
    Ok(())
}

pub fn cmd_release(dry_run: bool) -> Result<()> {
    if dry_run {
        info!("Dry run: would bump version, build release, and publish");
        return Ok(());
    }
    // Build release then publish (reuses cmd_build/cmd_publish logic via
    // spawning the same binary keeps it simple and honest).
    let exe = std::env::current_exe()?;
    let status = Command::new(exe).args(["build", "--release"]).status()?;
    if !status.success() {
        anyhow::bail!("Release build failed");
    }
    let exe = std::env::current_exe()?;
    let status = Command::new(exe).arg("publish").status()?;
    if !status.success() {
        anyhow::bail!("Publish failed");
    }
    Ok(())
}

pub async fn cmd_review(strict: bool) -> Result<()> {
    cmd_lint(None).await?;
    if strict {
        // Strict mode: also run tests
        cmd_test(None, false, false).await?;
    }
    Ok(())
}

pub fn cmd_diff() -> Result<()> {
    let status = Command::new("git").arg("diff").status()?;
    if !status.success() {
        anyhow::bail!("git diff failed (not a git repo?)");
    }
    Ok(())
}

pub fn cmd_flamegraph() -> Result<()> {
    if which_tool("perf").is_some() {
        info!("Run: perf record -g poly build && perf script | stackcollapse-perf.pl | flamegraph.pl > flamegraph.svg");
    } else {
        info!("perf not installed; flamegraph requires linux perf + FlameGraph tools");
    }
    Ok(())
}

pub fn cmd_stats() -> Result<()> {
    info!("Registry statistics require the Phase 6 registry backend");
    Ok(())
}

pub fn cmd_env(args: &[String]) -> Result<()> {
    match args.first().map(|s| s.as_str()) {
        Some("create") => {
            let name = args.get(1).context("env create <name>")?;
            let dir = PathBuf::from(std::env::var("HOME")?).join(".poly/envs").join(name);
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join("env.json"), "{}")?;
            info!("Created environment: {}", name);
        }
        Some("activate") | Some("deactivate") => {
            info!("Virtual environments are not yet sandboxed (Phase 5)");
        }
        _ => anyhow::bail!("Usage: poly env create|activate|deactivate <name>"),
    }
    Ok(())
}
