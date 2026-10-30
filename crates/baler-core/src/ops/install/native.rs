use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use baler_native::toolchain::{host_abi, read_abi_sidecars, AbiInfo};

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
/// A bundle made with `binary = true` ships `.lib/` with each artifact's
/// `.abi.json` sidecar, which records the platform and R version it was
/// built for. Three cases follow from comparing that with this machine:
///
/// - The binary fits (same platform, same R `major.minor`, and the same
///   source hash as the source that shipped next to it): it is used as it
///   is. No compiler and no `--install-deps` are needed.
/// - It does not fit but the source shipped too (`keep_source = true`):
///   it is rebuilt here, as before. Without `--install-deps` a note says
///   the shipped binary will not work on this machine.
/// - It does not fit and there is no source: the install fails with a
///   message naming both platforms, instead of leaving a binary that
///   cannot load.
///
/// A binary with no sidecar has no recorded platform, so it cannot be
/// checked and is left alone.
///
/// Building is gated behind `install_deps`: `[compiled-code].build_deps`
/// are resolved and installed here, separately from
/// `[dependencies]` and skipping `baler.lock`, since they're
/// compile-time-only, not a runtime contract.
pub(super) fn build_native_if_present(
    module_path: &PathBuf,
    name: &str,
    install_deps: bool,
    toml: &BalerToml,
) -> Result<()> {
    // A declared native dir that is not in the archive (bundled with
    // `keep_source = false`) has nothing to build from.
    let native_dirs: Vec<PathBuf> = toml
        .native_dirs_under(module_path)
        .into_iter()
        .filter(|dir| dir.is_dir())
        .collect();

    if native_dirs.is_empty() {
        return check_binary_only(module_path, name);
    }

    let mut by_lib_dir: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for native_dir in &native_dirs {
        let target_dir = native_dir.parent().unwrap_or(module_path.as_path());
        by_lib_dir
            .entry(target_dir.join(".lib"))
            .or_default()
            .push(native_dir.clone());
    }

    // Only ask R what this machine is when there is a shipped binary to
    // compare it with.
    let host = if by_lib_dir.keys().any(|lib_dir| lib_dir.is_dir()) {
        Some(host_abi()?)
    } else {
        None
    };

    // A `.lib/` is reused only when every native dir feeding it has a
    // binary that fits. One that does not means the whole `.lib/` is
    // rebuilt, as it always was, since the artifacts there are cleared
    // together.
    let mut reusable: BTreeSet<PathBuf> = BTreeSet::new();
    let mut foreign: Vec<AbiInfo> = Vec::new();
    if let Some((host_triple, host_r_version)) = &host {
        for (lib_dir, dirs) in &by_lib_dir {
            let shipped = read_abi_sidecars(lib_dir)?;
            if shipped.is_empty() {
                continue;
            }
            let mut all_fit = true;
            for native_dir in dirs {
                let binary_name = crate::ops::compile::binary_name(native_dir, name);
                let found = shipped.iter().find(|(artifact, _)| {
                    artifact.file_stem().and_then(|stem| stem.to_str()) == Some(binary_name)
                });
                let fits = match found {
                    Some((_, info)) if info.is_compatible(host_triple, host_r_version) => {
                        info.source_hash == baler_native::cache::source_hash(native_dir)?
                    }
                    Some((_, info)) => {
                        foreign.push(info.clone());
                        false
                    }
                    None => false,
                };
                all_fit &= fits;
            }
            if all_fit {
                reusable.insert(lib_dir.clone());
            }
        }
    }

    // What fits is used whatever `install_deps` says, it needs no compiler.
    if !reusable.is_empty() {
        for lib_dir in &reusable {
            for native_dir in &by_lib_dir[lib_dir] {
                remove_native_source(native_dir)?;
            }
        }
        println!(" [native] {}: using the binary that shipped for this platform", name);
    }

    let pending: Vec<(&PathBuf, &Vec<PathBuf>)> = by_lib_dir
        .iter()
        .filter(|(lib_dir, _)| !reusable.contains(*lib_dir))
        .collect();
    if pending.is_empty() {
        return Ok(());
    }

    if !install_deps {
        println!(
            " [native] {} has compiled code, build with: baler install --install-deps",
            name
        );
        if let (Some(found), Some((host_triple, host_r_version))) = (foreign.first(), &host) {
            println!(
                " [native] the binary shipped with '{}' was built for {} (R {}), this machine is {} (R {}), so it will not work here until it is built",
                name, found.target_triple, found.r_version, host_triple, host_r_version
            );
        }
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

    for (lib_dir, dirs) in pending {
        if lib_dir.exists() {
            std::fs::remove_dir_all(lib_dir)
                .with_context(|| format!("Failed to clear {}", lib_dir.display()))?;
        }

        for native_dir in dirs {
            let target_dir = native_dir.parent().unwrap_or(module_path.as_path());
            let binary_name = crate::ops::compile::binary_name(native_dir, name);

            println!("Building native code for '{}' ({})...", name, native_dir.display());
            let outcome = baler_native::build(target_dir, native_dir, binary_name, name)
                .with_context(|| format!("Failed to build native code for '{}' at {}", name, native_dir.display()))?;

            println!(
                " built: {} ({})",
                outcome.artifact_path.display(),
                if outcome.from_cache { "cached" } else { "compiled" }
            );

            remove_native_source(native_dir)?;
        }
    }

    Ok(())
}

/// An archive with no native source (bundled with `binary = true` and
/// `keep_source = false`) can only ever use the binary it shipped. If
/// that was built for another platform or R version, fail now with a
/// clear message, rather than leave a binary that cannot load.
fn check_binary_only(module_path: &Path, name: &str) -> Result<()> {
    let mut shipped: Vec<(PathBuf, AbiInfo)> = Vec::new();
    for entry in walkdir::WalkDir::new(module_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir() && e.file_name().to_str() == Some(".lib"))
    {
        shipped.extend(read_abi_sidecars(entry.path())?);
    }
    if shipped.is_empty() {
        return Ok(());
    }

    let (host_triple, host_r_version) = host_abi()?;
    if let Some((artifact, info)) = shipped
        .iter()
        .find(|(_, info)| !info.is_compatible(&host_triple, &host_r_version))
    {
        let file = artifact.file_name().unwrap_or_default().to_string_lossy();
        bail!(
            "'{name}' ships {file} built for {} (R {}) and no native source, \
             but this machine is {host_triple} (R {host_r_version}), so it cannot be used here.\n\
             Ask the module's author to bundle it with `keep_source = true` under \
             [compiled-code] in baler.toml so other platforms can build it from source, \
             or to publish a bundle built on this platform.",
            info.target_triple,
            info.r_version
        );
    }
    Ok(())
}

fn remove_native_source(native_dir: &Path) -> Result<()> {
    std::fs::remove_dir_all(native_dir)
        .with_context(|| format!("Failed to remove native source at {}", native_dir.display()))
}
