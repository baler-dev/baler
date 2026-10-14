use anyhow::{bail, Result};
use baler_core::ops::lock::{LockOptions, UpdateScope};

pub struct LockArgs {
    pub path: String,
    /// `None`: no `--update`. `Some(empty)`: a bare `--update`, which
    /// re-resolves everything. `Some(names)`: `--update <pkg>...`, which
    /// re-resolves only those packages.
    pub update: Option<Vec<String>>,
    pub with_rver: bool,
    pub remove: bool,
    pub check: bool,
}

pub fn run(args: LockArgs) -> Result<()> {
    let update = match args.update {
        None => UpdateScope::Keep,
        Some(names) if names.is_empty() => UpdateScope::All,
        Some(names) => {
            for name in &names {
                check_package_name(name)?;
            }
            UpdateScope::Packages(names)
        }
    };

    let opts = LockOptions {
        update,
        with_r_version: args.with_rver,
        remove: args.remove,
        check: args.check,
    };
    baler_core::ops::lock::run_with(&args.path, &opts)
}

/// `--update` takes any number of values, so a path typed after it
/// (`baler lock --update .`) lands here as a "package". R package names
/// can't start with a dot or contain a slash, so anything that does is
/// almost certainly a path, and gets a pointed message instead of a
/// confusing "not in baler.lock".
fn check_package_name(name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('.') || name.chars().any(|c| c == '/' || c == '\\') {
        bail!(
            "'{name}' looks like a path, not an R package name. \
             Put the project path first: baler lock <path> --update <pkg>"
        );
    }
    Ok(())
}
