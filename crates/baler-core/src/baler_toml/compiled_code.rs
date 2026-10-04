use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::dependencies::PackageDep;

// ---- NativePath ----

/// `[compiled-code].path` accepts either a single string or an array,
/// so a module with one compiled-code dir doesn't have to write
/// `path = ["cpp/"]` just to satisfy a Vec-only field, and a module
/// with several doesn't have to pick one arbitrarily.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum NativePath {
    Single(String),
    Multiple(Vec<String>),
}

impl NativePath {
    pub fn as_paths(&self) -> Vec<&str> {
        match self {
            NativePath::Single(p) => vec![p.as_str()],
            NativePath::Multiple(ps) => ps.iter().map(String::as_str).collect(),
        }
    }
}

// ---- CompiledCode ----
/// Declares where a module's compiled code lives, and its build-time-
/// only R package deps (e.g. `Rcpp`), whose headers a `Makevars` needs
/// to find via `system.file()` before `R CMD SHLIB` can run.
///
/// `path` is relative to the module's own source directory (whatever
/// `resolve_src_dir()` resolves to), the same base `src` in
/// `[project]` already uses, not the project root `baler.toml` lives
/// in. `path = ["cpp", "extra/src"]` in a module's own `baler.toml`
/// means exactly what it looks like: two dirs nested under that
/// module's source tree.
///
/// `path` is optional and exists purely as an override. When omitted,
/// `resolve_native_dirs()` scans the module's whole source tree for
/// compiled-code dirs instead of assuming one is where it must live.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CompiledCode {
    pub path: Option<NativePath>,
    pub build_deps: Option<BTreeMap<String, PackageDep>>,
}
