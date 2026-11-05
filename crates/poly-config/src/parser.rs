//! poly.ini parser using nom

use crate::model::*;
use anyhow::{Context, Result};
use nom::{
    bytes::complete::{take_until, take_while1},
    character::complete::{char, line_ending, multispace0, space0},
    combinator::{map, opt},
    multi::many0,
    sequence::delimited,
    IResult,
};
use std::collections::HashMap;
use tracing::debug;

/// Parse a poly.ini file
pub fn parse_poly_ini(path: &str) -> Result<Config> {
    debug!("Parsing poly.ini from {}", path);

    let content = std::fs::read_to_string(path)
        .context(format!("Failed to read poly.ini from {}", path))?;

    let (_, ini) = parse_ini(&content)
        .map_err(|e| anyhow::anyhow!("Failed to parse poly.ini: {:?}", e))?;

    // Build config from parsed INI
    let config = build_config(ini)?;

    debug!("Successfully parsed poly.ini");
    Ok(config)
}

#[derive(Debug, Clone)]
struct IniSection {
    name: String,
    properties: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct Ini {
    sections: Vec<IniSection>,
}

fn parse_ini(input: &str) -> IResult<&str, Ini> {
    let (input, sections) = many0(parse_section)(input)?;
    Ok((input, Ini { sections }))
}

fn parse_section(input: &str) -> IResult<&str, IniSection> {
    let (input, _) = multispace0(input)?;
    let (input, name) = parse_section_name(input)?;
    let (input, _) = multispace0(input)?;
    let (input, properties) = many0(parse_property)(input)?;
    let (input, _) = multispace0(input)?;

    Ok((
        input,
        IniSection {
            name,
            properties: properties.into_iter().collect(),
        },
    ))
}

fn parse_section_name(input: &str) -> IResult<&str, String> {
    delimited(
        char('['),
        map(
            take_while1(|c: char| c.is_alphanumeric() || c == '.' || c == '_'),
            |s: &str| s.to_string(),
        ),
        char(']'),
    )(input)
}

fn parse_property(input: &str) -> IResult<&str, (String, String)> {
    let (input, key) = parse_key(input)?;
    let (input, _) = space0(input)?;
    let (input, _) = char('=')(input)?;
    let (input, _) = space0(input)?;
    let (input, value) = parse_value(input)?;
    let (input, _) = opt(line_ending)(input)?;

    Ok((input, (key, value)))
}

fn parse_key(input: &str) -> IResult<&str, String> {
    map(
        take_while1(|c: char| c.is_alphanumeric() || c == '_'),
        |s: &str| s.to_string(),
    )(input)
}

fn parse_value(input: &str) -> IResult<&str, String> {
    let (input, value) = take_until("\n")(input)?;
    Ok((input, value.trim().to_string()))
}

fn build_config(ini: Ini) -> Result<Config> {
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

    for section in &ini.sections {
        match section.name.as_str() {
            "project" => {
                project = Some(Project {
                    name: section.properties.get("name").unwrap_or(&String::new()).clone(),
                    version: section.properties.get("version").unwrap_or(&String::new()).clone(),
                    edition: section.properties.get("edition").cloned(),
                });
            }
            "workspace" => {
                workspace = Some(Workspace {
                    members: parse_array(section.properties.get("members")),
                    shared_deps: parse_array(section.properties.get("sharedDeps")),
                });
            }
            name if name.starts_with("target.") => {
                let target_name = name.strip_prefix("target.").unwrap();
                let target = Target {
                    language: section.properties.get("language").unwrap_or(&String::new()).clone(),
                    type_: section.properties.get("type").unwrap_or(&String::new()).clone(),
                    sources: parse_array(section.properties.get("sources")),
                    compiler: section.properties.get("compiler").cloned(),
                    flags: parse_array(section.properties.get("flags")),
                    edition: section.properties.get("edition").cloned(),
                    features: parse_array(section.properties.get("features")),
                    module: section.properties.get("module").cloned(),
                    cgo: section.properties.get("cgo").and_then(|v| v.parse().ok()),
                };
                targets.insert(target_name.to_string(), target);
            }
            "dependencies.rust" => {
                rust_deps = section.properties.clone();
            }
            "dependencies.c" => {
                c_deps = section.properties.clone();
            }
            "dependencies.go" => {
                go_deps = section.properties.clone();
            }
            "build.default" => {
                default_profile = Some(BuildProfile {
                    targets: parse_array(section.properties.get("targets")),
                    rust_flags: parse_array(section.properties.get("rust.flags")),
                    c_flags: parse_array(section.properties.get("c.flags")),
                    go_flags: parse_array(section.properties.get("go.flags")),
                });
            }
            name if name.starts_with("build.profiles.") => {
                let profile_name = name.strip_prefix("build.profiles.").unwrap();
                let profile = BuildProfile {
                    targets: parse_array(section.properties.get("targets")),
                    rust_flags: parse_array(section.properties.get("rust.flags")),
                    c_flags: parse_array(section.properties.get("c.flags")),
                    go_flags: parse_array(section.properties.get("go.flags")),
                };
                build_profiles.insert(profile_name.to_string(), profile);
            }
            "publish" => {
                publish = Some(PublishConfig {
                    targets: parse_array(section.properties.get("targets")),
                    platforms: parse_array(section.properties.get("platforms")),
                    include_docs: section.properties.get("includeDocs")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(false),
                });
            }
            "vfsx" => {
                vfsx = Some(VfsxConfig {
                    sign: section.properties.get("sign")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(false),
                    keyfile: section.properties.get("keyfile").unwrap_or(&String::new()).clone(),
                    verify: section.properties.get("verify")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(false),
                    trust_store: section.properties.get("trustStore").unwrap_or(&String::new()).clone(),
                });
            }
            "lint" => {
                lint = Some(LintConfig {
                    rust: section.properties.get("rust").unwrap_or(&String::new()).clone(),
                    c: section.properties.get("c").unwrap_or(&String::new()).clone(),
                    go: section.properties.get("go").unwrap_or(&String::new()).clone(),
                    run_on_build: section.properties.get("runOnBuild")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(false),
                });
            }
            "format" => {
                format = Some(FormatConfig {
                    rust: section.properties.get("rust").unwrap_or(&String::new()).clone(),
                    c: section.properties.get("c").unwrap_or(&String::new()).clone(),
                    go: section.properties.get("go").unwrap_or(&String::new()).clone(),
                });
            }
            "cache" => {
                cache = Some(CacheConfig {
                    local: section.properties.get("local").unwrap_or(&String::new()).clone(),
                    remote: section.properties.get("remote").cloned(),
                    enabled: section.properties.get("enabled")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(false),
                });
            }
            "toolchain" => {
                toolchain = Some(ToolchainConfig {
                    rust: ToolchainSpec {
                        version: section.properties.get("rust.version").unwrap_or(&String::new()).clone(),
                        channel: section.properties.get("rust.channel").cloned(),
                        compiler: None,
                    },
                    c: ToolchainSpec {
                        version: section.properties.get("c.version").unwrap_or(&String::new()).clone(),
                        channel: None,
                        compiler: section.properties.get("c.compiler").cloned(),
                    },
                    go: ToolchainSpec {
                        version: section.properties.get("go.version").unwrap_or(&String::new()).clone(),
                        channel: None,
                        compiler: None,
                    },
                });
            }
            "cross" => {
                cross = Some(CrossConfig {
                    targets: parse_array(section.properties.get("targets")),
                    sysroot: section.properties.get("sysroot").unwrap_or(&String::new()).clone(),
                });
            }
            "plugins" => {
                plugins = Some(section.properties.clone());
            }
            "hooks" => {
                hooks = Some(HooksConfig {
                    pre_build: section.properties.get("preBuild").map(|cmd| Hook {
                        command: cmd.clone(),
                    }),
                    post_build: section.properties.get("postBuild").map(|cmd| Hook {
                        command: cmd.clone(),
                    }),
                    pre_test: section.properties.get("preTest").map(|cmd| Hook {
                        command: cmd.clone(),
                    }),
                    post_test: section.properties.get("postTest").map(|cmd| Hook {
                        command: cmd.clone(),
                    }),
                });
            }
            "secrets" => {
                secrets = Some(section.properties.clone());
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

    // Validate camelCase keys
    validate_camel_case(&ini)?;

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

/// Validate that all keys are camelCase
fn validate_camel_case(ini: &Ini) -> Result<()> {
    for section in &ini.sections {
        for key in section.properties.keys() {
            if !is_camel_case(key) {
                let expected = to_camel_case(key);
                tracing::warn!(
                    "poly.ini: key '{}' in section '{}' is not camelCase, use '{}'",
                    key,
                    section.name,
                    expected
                );
            }
        }
    }
    Ok(())
}

/// Check if a string is camelCase
fn is_camel_case(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }

    let chars: Vec<char> = s.chars().collect();

    // First character must be lowercase
    if !chars[0].is_lowercase() {
        return false;
    }

    // No hyphens, underscores, or dots allowed in keys
    for c in chars {
        if c == '-' || c == '_' || c == '.' {
            return false;
        }
    }

    true
}

/// Convert a string to camelCase
fn to_camel_case(s: &str) -> String {
    let parts: Vec<&str> = s.split(['-', '_', '.']).collect();
    let mut result = String::new();

    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            result.push_str(&part.to_lowercase());
        } else {
            let mut chars = part.chars();
            if let Some(first) = chars.next() {
                result.push(first.to_uppercase().next().unwrap());
                result.extend(chars);
            }
        }
    }

    result
}
