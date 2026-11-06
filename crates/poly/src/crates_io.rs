//! Registry token storage and crate naming helpers.
//!
//! `poly login`/`poly logout` manage API tokens in
//! `~/.config/poly/credentials.toml`. `crate_name_for` derives the crates.io
//! package name for a publish target (used for package metadata).

use anyhow::{Context, Result};
use poly_config::Config;
use std::path::PathBuf;

/// crates.io requires `[package] name` in Cargo.toml to match the publish
/// metadata name.
///
/// Naming rules:
/// - `[publish] crateName` set + multiple Rust targets →
///   `{crateName}-{suffix}` where suffix is the target id with the
///   `{project}-` prefix stripped (poly-tools → axiom-poly-tools).
/// - `[publish] crateName` set + single target → `crateName`.
/// - No override → the target id itself (poly-config, my-app, ...).
pub fn crate_name_for(config: &Config, target_id: &str) -> String {
    let rust_targets: Vec<String> = publish_target_specs(config)
        .into_iter()
        .filter(|t| t.starts_with("rust:"))
        .collect();
    let multi = rust_targets.len() > 1;

    if let Some(base) = config
        .publish
        .as_ref()
        .and_then(|p| p.crate_name.as_ref())
    {
        let base = base.replace('_', "-");
        if !multi {
            return base;
        }
        let prefix = format!("{}-", config.project.name.replace('_', "-"));
        let suffix = target_id
            .strip_prefix(&prefix)
            .unwrap_or(target_id)
            .replace('_', "-");
        return format!("{}-{}", base, suffix);
    }
    target_id.replace('_', "-")
}

/// The `lang:target` specs publish will act on: [publish] targets, falling
/// back to [build.default] targets.
fn publish_target_specs(config: &Config) -> Vec<String> {
    if let Some(p) = &config.publish {
        if !p.targets.is_empty() {
            return p.targets.clone();
        }
    }
    config.build.default.targets.clone()
}

/// Path to Poly's own credentials store: ~/.config/poly/credentials.toml
pub fn credentials_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("poly").join("credentials.toml"))
}

/// Save a crates.io API token to Poly's credentials file.
pub fn save_token(token: &str, registry: &str) -> Result<PathBuf> {
    let path = credentials_path().context("Cannot determine config directory")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Preserve other sections if the file exists.
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    let section = format!("[registries.{}]", registry);
    let entry = format!("token = {:?}\n", token);
    if let Some(start) = text.find(&section) {
        let rest = &text[start..];
        let end = rest[1..]
            .find("\n[")
            .map(|i| start + 1 + i)
            .unwrap_or(text.len());
        text.replace_range(start..end, &format!("{}{}", section, entry));
    } else {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!("\n{}{}", section, entry));
    }
    std::fs::write(&path, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

/// Remove a registry's token from Poly's credentials file.
pub fn clear_token(registry: &str) -> Result<bool> {
    let Some(path) = credentials_path() else {
        return Ok(false);
    };
    if !path.is_file() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(&path)?;
    let section = format!("[registries.{}]", registry);
    if !text.contains(&section) {
        return Ok(false);
    }
    // Remove the whole [registries.X] section (up to next '[' header or EOF).
    let start = text.find(&section).unwrap();
    let rest = &text[start + section.len()..];
    let end = rest
        .find("\n[")
        .map(|i| start + section.len() + i + 1)
        .unwrap_or(text.len());
    let mut new_text = String::with_capacity(text.len());
    new_text.push_str(&text[..start]);
    new_text.push_str(&text[end..]);
    std::fs::write(&path, new_text)?;
    Ok(true)
}
