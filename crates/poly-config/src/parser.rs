//! poly.ini parser using simple line-based parsing

use crate::model::*;
use anyhow::{Context, Result};
use std::collections::HashMap;
use tracing::debug;

/// Parse a poly.ini file
pub fn parse_poly_ini(path: &str) -> Result<Config> {
    debug!("Parsing poly.ini from {}", path);

    let content = std::fs::read_to_string(path)
        .context(format!("Failed to read poly.ini from {}", path))?;

    let config = parse_ini_content(&content)?;

    debug!("Successfully parsed poly.ini");
    Ok(config)
}

fn parse_ini_content(content: &str) -> Result<Config> {
    let mut sections: Vec<(String, HashMap<String, String>)> = Vec::new();
    let mut current_section: Option<String> = None;
    let mut current_props: HashMap<String, String> = HashMap::new();

    for line in content.lines() {
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        // Check if it's a section header
        if line.starts_with('[') && line.ends_with(']') {
            // Save previous section
            if let Some(name) = current_section.take() {
                sections.push((name, std::mem::take(&mut current_props)));
            }
            current_section = Some(line[1..line.len()-1].to_string());
            continue;
        }

        // Parse key=value
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_string();
            let value = value.trim().to_string();
            current_props.insert(key, value);
        }
    }

    // Save last section
    if let Some(name) = current_section.take() {
        sections.push((name, current_props));
    }

    build_config(sections)
}

fn build_config(sections: Vec<(String, HashMap<String, String>)>) -> Result<Config> {
    let mut project = None;
    let mut workspace = None;
    let mut targets = HashMap::new();
    let mut rust_deps = HashMap::new();
    let mut c_deps = HashMap::new();
    let mut go_deps = HashMap::new();
    let mut build_profiles = HashMap::new();
    let mut default_profile = None;
    let mut publish = None;
    let mut vfsx = None;
    let mut lint = None;
    let mut format = None;
    let mut cache = None;
    let mut toolchain = None;
    let mut cross = None;
    let mut plugins = None;
    let mut hooks = None;
    let mut secrets = None;

    for (section_name, props) in sections {
        match section_name.as_str() {
            "project" => {
                project = Some(Project {
                    name: props.get("name").unwrap_or(&String::new()).clone(),
                    version: props.get("version").unwrap_or(&String::new()).clone(),
                    edition: props.get("edition").cloned(),
                });
            }
            "workspace" => {
                workspace = Some(Workspace {
                    members: parse_array(props.get("members")),
                    shared_deps: parse_array(props.get("sharedDeps")),
                });
            }
            name if name.starts_with("target.") => {
                let target_name = name.strip_prefix("target.").unwrap();
                let target = Target {
                    language: props.get("language").unwrap_or(&String::new()).clone(),
                    type_: props.get("type").unwrap_or(&String::new()).clone(),
                    sources: parse_array(props.get("sources")),
                    compiler: props.get("compiler").cloned(),
                    flags: parse_array(props.get("flags")),
                    edition: props.get("edition").cloned(),
                    features: parse_array(props.get("features")),
                    module: props.get("module").cloned(),
                    cgo: props.get("cgo").and_then(|v| v.parse().ok()),
                };
                targets.insert(target_name.to_string(), target);
            }
            "dependencies.rust" => {
                rust_deps = props.clone();
            }
            "dependencies.c" => {
                c_deps = props.clone();
            }
            "dependencies.go" => {
                go_deps = props.clone();
            }
            "build.default" => {
                default_profile = Some(BuildProfile {
                    targets: parse_array(props.get("targets")),
                    rust_flags: parse_array(props.get("rust.flags")),
                    c_flags: parse_array(props.get("c.flags")),
                    go_flags: parse_array(props.get("go.flags")),
                });
            }
            name if name.starts_with("build.profiles.") => {
                let profile_name = name.strip_prefix("build.profiles.").unwrap();
                let profile = BuildProfile {
                    targets: parse_array(props.get("targets")),
                    rust_flags: parse_array(props.get("rust.flags")),
                    c_flags: parse_array(props.get("c.flags")),
                    go_flags: parse_array(props.get("go.flags")),
                };
                build_profiles.insert(profile_name.to_string(), profile);
            }
            "publish" => {
                publish = Some(PublishConfig {
                    targets: parse_array(props.get("targets")),
                    platforms: parse_array(props.get("platforms")),
                    include_docs: props.get("includeDocs")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(false),
                });
            }
            "vfsx" => {
                vfsx = Some(VfsxConfig {
                    sign: props.get("sign")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(false),
                    keyfile: props.get("keyfile").unwrap_or(&String::new()).clone(),
                    verify: props.get("verify")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(false),
                    trust_store: props.get("trustStore").unwrap_or(&String::new()).clone(),
                });
            }
            "lint" => {
                lint = Some(LintConfig {
                    rust: props.get("rust").unwrap_or(&String::new()).clone(),
                    c: props.get("c").unwrap_or(&String::new()).clone(),
                    go: props.get("go").unwrap_or(&String::new()).clone(),
                    run_on_build: props.get("runOnBuild")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(false),
                });
            }
            "format" => {
                format = Some(FormatConfig {
                    rust: props.get("rust").unwrap_or(&String::new()).clone(),
                    c: props.get("c").unwrap_or(&String::new()).clone(),
                    go: props.get("go").unwrap_or(&String::new()).clone(),
                });
            }
            "cache" => {
                cache = Some(CacheConfig {
                    local: props.get("local").unwrap_or(&String::new()).clone(),
                    remote: props.get("remote").cloned(),
                    enabled: props.get("enabled")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(false),
                });
            }
            "toolchain" => {
                toolchain = Some(ToolchainConfig {
                    rust: ToolchainSpec {
                        version: props.get("rust.version").unwrap_or(&String::new()).clone(),
                        channel: props.get("rust.channel").cloned(),
                        compiler: None,
                    },
                    c: ToolchainSpec {
                        version: props.get("c.version").unwrap_or(&String::new()).clone(),
                        channel: None,
                        compiler: props.get("c.compiler").cloned(),
                    },
                    go: ToolchainSpec {
                        version: props.get("go.version").unwrap_or(&String::new()).clone(),
                        channel: None,
                        compiler: None,
                    },
                });
            }
            "cross" => {
                cross = Some(CrossConfig {
                    targets: parse_array(props.get("targets")),
                    sysroot: props.get("sysroot").unwrap_or(&String::new()).clone(),
                });
            }
            "plugins" => {
                plugins = Some(props.clone());
            }
            "hooks" => {
                hooks = Some(HooksConfig {
                    pre_build: props.get("preBuild").map(|cmd| Hook {
                        command: cmd.clone(),
                    }),
                    post_build: props.get("postBuild").map(|cmd| Hook {
                        command: cmd.clone(),
                    }),
                    pre_test: props.get("preTest").map(|cmd| Hook {
                        command: cmd.clone(),
                    }),
                    post_test: props.get("postTest").map(|cmd| Hook {
                        command: cmd.clone(),
                    }),
                });
            }
            "secrets" => {
                secrets = Some(props.clone());
            }
            _ => {}
        }
    }

    let project = project.context("Missing [project] section")?;
    let default_profile = default_profile.context("Missing [build.default] section")?;

    let mut profiles = HashMap::new();
    profiles.insert("default".to_string(), default_profile.clone());
    for (name, profile) in build_profiles {
        profiles.insert(name, profile);
    }

    let build = BuildConfig {
        default: default_profile,
        profiles,
    };

    let dependencies = Dependencies {
        rust: rust_deps,
        c: c_deps,
        go: go_deps,
    };

    Ok(Config {
        project,
        workspace,
        targets,
        dependencies,
        build,
        publish,
        vfsx,
        lint,
        format,
        cache,
        toolchain,
        cross,
        plugins,
        hooks,
        secrets,
    })
}

fn parse_array(value: Option<&String>) -> Vec<String> {
    match value {
        Some(v) => v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        None => Vec::new(),
    }
}
