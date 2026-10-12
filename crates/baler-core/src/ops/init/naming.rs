//! The module name `baler init` uses when none is given.

use std::fs;
use std::path::Path;

/// The project directory's own name, used as is. Whether it suits as a
/// module name is the user's call: they can change `name` in `baler.toml`
/// or pass `--name`, and `box::use()` can import a non-syntactic name with
/// backticks.
pub(super) fn project_dir_name(project_root: &Path) -> Option<String> {
    let abs = fs::canonicalize(project_root).ok()?;
    abs.file_name()?.to_str().map(str::to_owned)
}
