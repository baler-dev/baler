//! What `baler init` checks about the directory it is run in. Only
//! paths and names are looked at, never the contents of a file.

use anyhow::{bail, Context, Result};

use std::fs;
use std::path::{Component, Path};

/// Checks `--src` and returns it as a `/`-separated path relative to the
/// project.
pub(super) fn explicit_src(project_root: &Path, raw: &str) -> Result<String> {
    let rel = Path::new(raw);
    let inside = !rel.is_absolute()
        && rel
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if !inside {
        bail!("`--src` must be a path inside the project, relative to it (got '{raw}').");
    }
    let parts: Vec<&str> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(p) => p.to_str(),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        bail!("`--src` can't be the project root. A module's source lives in a subdirectory.");
    }
    if !project_root.join(rel).is_dir() {
        bail!("`--src` '{raw}' is not a directory.");
    }
    Ok(parts.join("/"))
}

/// Whether `dir` has nothing in it but hidden entries (`.git`, `.Rproj.user`,
/// ...). Only names are looked at, never contents.
pub(super) fn is_fresh(dir: &Path) -> Result<bool> {
    let entries = fs::read_dir(dir)
        .with_context(|| format!("Failed to read directory: {}", dir.display()))?;
    for entry in entries {
        if !entry?.file_name().to_string_lossy().starts_with('.') {
            return Ok(false);
        }
    }
    Ok(true)
}
