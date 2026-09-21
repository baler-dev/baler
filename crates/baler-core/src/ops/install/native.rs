use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::baler_toml::BalerToml;
use crate::ops::resolve;

/// Compiles a module's native code, if it has any, right after
/// unpacking. `module_path` is the module's own flat installed
/// directory (`<install_dir>/<name>`), which after unpack already *is*
/// the module's source dir, there is no project root wrapping it.
///
/// `toml` is the `baler.toml` shipped inside the archive and read back
/// by `tar::read_toml`. Declared native dirs come from
/// `toml.native_dirs_under(module_path)`, which honours
/// `[compiled-code].path` if the author set one, falling back to a
/// filesystem scan otherwise, same resolution `bundle` used to decide
/// what to ship. There used to be a `declared_dirs` list baked into a
/// generated manifest.json for this; recomputing it from the shipped
/// baler.toml gives the identical answer without needing a generated
/// copy to stay in sync with it.
///
/// Gated behind `install_deps`: `[compiled-code].build_deps` are
/// resolved and installed here, separately from
/// `[project.dependencies]` and skipping `baler.lock`, since they're
/// compile-time-only, not a runtime contract.
pub(super) fn build_native_if_present(
    module_path: &PathBuf,
    name: &str,
    install_deps: bool,
    toml: &BalerToml,
) -> Result<()> {
    let native_dirs = toml.native_dirs_under(module_path);

    if native_dirs.is_empty() {
        return Ok(());
    }

    if !install_deps {
        println!(
            " [native] {} has compiled code, build with: baler install --install-deps",
            name
        );
        return Ok(());
    }

    let build_deps = toml.compiled_code.as_ref()
        .and_then(|n| n.build_deps.clone())
        .filter(|deps| !deps.is_empty());

    if let Some(deps) = build_deps {
        println!("  Installing native build deps for '{}'...", name);
        let plan = resolve::resolve(&Some(deps), &None)?;
        resolve::print_plan(&plan);
        resolve::execute_plan(&plan, false, None)?;
    }

    let mut cleared_lib_dirs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    for native_dir in &native_dirs {
        let target_dir = native_dir.parent().unwrap_or(module_path.as_path());
        let lib_dir = target_dir.join(".lib");
        if cleared_lib_dirs.insert(lib_dir.clone()) && lib_dir.exists() {
            std::fs::remove_dir_all(&lib_dir)
                .with_context(|| format!("Failed to clear {}", lib_dir.display()))?;
        }

        let binary_name = crate::ops::compile::binary_name(native_dir, name);

        println!("Building native code for '{}' ({})...", name, native_dir.display());
        let outcome = baler_native::build(target_dir, native_dir, binary_name, name)
            .with_context(|| format!("Failed to build native code for '{}' at {}", name, native_dir.display()))?;

        println!(
            " built: {} ({})",
            outcome.artifact_path.display(),
            if outcome.from_cache { "cached" } else { "compiled" }
        );

        std::fs::remove_dir_all(native_dir)
            .with_context(|| format!("Failed to remove native source at {}", native_dir.display()))?;
    }

    Ok(())
}
