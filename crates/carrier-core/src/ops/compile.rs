use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use crate::carrier_toml::CarrierToml;
use crate::cran::client::read_installed_version;
use crate::ops::resolve;
use crate::paths::resolve_r_lib_dir;
use crate::version::VersionSpec;

/// How `run` treats a module's cache and `.lib/` before compiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileMode {
    /// Compile normally, a cache hit may skip the compiler.
    Normal,
    /// Remove cached and compiled artifacts and stop, like
    /// `pkgbuild::clean_dll()`. Never compiles.
    Clean,
    /// Evict the cache first, then compile, so the build
    /// cannot be a cache hit.
    Rebuild,
}

/// Compile a module's native code in place, mirroring
/// `devtools::load_all()`'s convention of building `src/*.so`
/// right where the source lives, for fast dev-loop iteration.
/// Resolves `[native].build_deps` first, same as `install`.
pub fn run(project_root: &Path, mode: CompileMode) -> Result<Vec<CompiledArtifact>> {
    if !project_root.join("carrier.toml").exists() {
        bail!(
            "No carrier.toml found in {}. Is this a carrier module project?",
            project_root.display()
        );
    }

    let toml = CarrierToml::from_dir(project_root)?;
    let name = toml.module.name.clone();
    let native_dirs = toml.resolve_native_dirs(project_root)?;

    if native_dirs.is_empty() {
        return Ok(Vec::new());
    }

    if matches!(mode, CompileMode::Clean | CompileMode::Rebuild) {
        carrier_native::cache::clear_module_cache(&name)
            .with_context(|| format!("Failed to clear native build cache for '{}'", name))?;
    }

    if mode == CompileMode::Clean {
        let mut cleared_lib_dirs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for native_dir in &native_dirs {
            let target_dir = native_dir.parent().unwrap_or(project_root);
            let lib_dir = target_dir.join(".lib");
            if cleared_lib_dirs.insert(lib_dir.clone()) && lib_dir.exists() {
                std::fs::remove_dir_all(&lib_dir)
                    .with_context(|| format!("Failed to clear {}", lib_dir.display()))?;
            }
        }

        return Ok(Vec::new());
    }

    let build_deps = toml.native.as_ref()
        .and_then(|n| n.build_deps.clone())
        .filter(|deps| !deps.is_empty());

    if let Some(deps) = build_deps {
        let all_satisfied = resolve_r_lib_dir()
            .map(|r_lib| {
                deps.iter().all(|(pkg_name, dep)| {
                    let desc_path = r_lib.join(pkg_name).join("DESCRIPTION");
                    let Ok(installed) = read_installed_version(&desc_path) else { return false };
                    let Ok(spec) = VersionSpec::parse(dep.version()) else { return false };
                    spec.matches(installed.semver())
                })
            })
            .unwrap_or(false);

        if !all_satisfied {
            println!("Installing native build deps for '{}'...", name);
            let plan = resolve::resolve(&Some(deps), &None)?;
            resolve::print_plan(&plan);
            resolve::execute_plan(&plan, false, None)?;
        }
    }

    let mut cleared_lib_dirs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut compiled = Vec::new();
    for native_dir in &native_dirs {
        if !native_dir.is_dir() {
            bail!(
                "Configured native dir '{}' does not exist.",
                native_dir.display()
            );
        }

        let target_dir = native_dir.parent().unwrap_or(project_root);
        let lib_dir = target_dir.join(".lib");
        if cleared_lib_dirs.insert(lib_dir.clone()) && lib_dir.exists() {
            std::fs::remove_dir_all(&lib_dir)
                .with_context(|| format!("Failed to clear {}", lib_dir.display()))?;
        }

        let binary_name = binary_name(native_dir, &name);

        let outcome = carrier_native::build(target_dir, native_dir, binary_name, &name)
            .with_context(|| format!("Failed to compile native code for '{}' at {}", name, native_dir.display()))?;

        compiled.push(CompiledArtifact {
            native_dir: native_dir.clone(),
            artifact_path: outcome.artifact_path,
            target_triple: outcome.target_triple,
            r_version: outcome.r_version,
            source_hash: outcome.source_hash,
            from_cache: outcome.from_cache,
        });
    }

    Ok(compiled)
}

pub struct CompiledArtifact {
    pub native_dir: PathBuf,
    pub artifact_path: PathBuf,
    pub target_triple: String,
    pub r_version: String,
    pub source_hash: String,
    pub from_cache: bool,
}

/// Names the compiled artifact after the native dir's own folder
/// (`cpp`, `c`) so multiple native dirs sharing one parent don't
/// collide on the same output filename. A generic `src` carries no
/// disambiguating information though, just noise in `.lib/`, so falls
/// back to the module's own name instead in that one case.
pub fn binary_name<'a>(native_dir: &'a Path, module_name: &'a str) -> &'a str {
    match native_dir.file_name().and_then(|f| f.to_str()) {
        Some("src") | None => module_name,
        Some(folder) => folder,
    }
}
