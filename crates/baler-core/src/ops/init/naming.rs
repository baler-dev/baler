//! The module name `baler init` uses when none is given, and the rules
//! a name has to follow.

use anyhow::{bail, Result};

use std::fs;
use std::path::Path;

/// The project directory's name as a module name: a trailing `-proj`
/// (what `baler init <name>` appends) is dropped.
pub(super) fn project_dir_name(project_root: &Path) -> Option<String> {
    let abs = fs::canonicalize(project_root).ok()?;
    let raw = abs.file_name()?.to_str()?;
    let trimmed = raw.strip_suffix("-proj").filter(|s| !s.is_empty()).unwrap_or(raw);
    Some(trimmed.to_owned())
}

fn is_valid_module_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(super) fn validate_module_name(name: &str) -> Result<()> {
    if is_valid_module_name(name) {
        Ok(())
    } else {
        bail!(
            "'{name}' is not a valid module name: use letters, digits and underscores, \
             starting with a letter. Pass `--name <name>` to choose one."
        )
    }
}
