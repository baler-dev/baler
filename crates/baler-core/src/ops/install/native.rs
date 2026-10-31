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
/// A bundle made with `binary = true` ships `.lib/` with the binaries.
/// A bundle made on several platforms has one `.lib/<platform>/` folder
/// per platform. Install keeps the folder that fits this machine, moves
/// its files up into `.lib/` and deletes the other folders. Each
/// artifact's `.abi.json` sidecar records the platform and R version it
/// was built for, and comparing that with this machine gives three cases:
///
/// - The binary fits (same platform, same R `major.minor`, and the same
///   source hash as the source that shipped next to it): it is used as
///   it is. No compiler and no `--install-deps` are needed.
/// - It does not fit but the source shipped (`keep_source = true`): it
///   is rebuilt here. Without `--install-deps` a note says the shipped
///   binaries will not work on this machine.
/// - It does not fit and there is no source: the install fails with a
///   message naming every platform the bundle holds and this machine's,
///   instead of leaving a binary that cannot load.
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
    // Only ask R what this machine is when the bundle shipped binaries.
    let lib_dirs = find_lib_dirs(module_path);
    let host = if lib_dirs.is_empty() {
        None
    } else {
        Some(host_abi()?)
    };

    // Binaries that shipped but cannot run here, from the platform
    // folders and from `.lib/` itself.
    let mut foreign: Vec<AbiInfo> = Vec::new();
    if let Some((host_triple, host_r_version)) = &host {
        for lib_dir in &lib_dirs {
            foreign.extend(select_platform_folder(lib_dir, host_triple, host_r_version)?);
        }
    }

    // A declared native dir that is not in the archive (bundled with
    // `keep_source = false`) has nothing to build from.
    let native_dirs: Vec<PathBuf> = toml
        .native_dirs_under(module_path)
        .into_iter()
        .filter(|dir| dir.is_dir())
        .collect();

    if native_dirs.is_empty() {
        return check_binary_only(name, &lib_dirs, &host, &foreign);
    }

    let mut by_lib_dir: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for native_dir in &native_dirs {
        let target_dir = native_dir.parent().unwrap_or(module_path.as_path());
        by_lib_dir
            .entry(target_dir.join(".lib"))
            .or_default()
            .push(native_dir.clone());
    }

    // A `.lib/` is reused only when every native dir feeding it has a
    // binary that fits. One that does not means the whole `.lib/` is
    // rebuilt, as it always was, since the artifacts there are cleared
    // together.
    let mut reusable: BTreeSet<PathBuf> = BTreeSet::new();
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
        if let (false, Some((host_triple, host_r_version))) = (foreign.is_empty(), &host) {
            println!(
                " [native] the binaries shipped with '{}' were built for {}, this machine is {} (R {}), so none of them will work here until it is built",
                name,
                describe(&foreign),
                host_triple,
                host_r_version
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

/// Every `.lib/` directory under `module_path`.
fn find_lib_dirs(module_path: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut walker = walkdir::WalkDir::new(module_path).into_iter();
    while let Some(entry) = walker.next() {
        let Ok(entry) = entry else { continue };
        if entry.file_type().is_dir() && entry.file_name().to_str() == Some(".lib") {
            found.push(entry.path().to_path_buf());
            walker.skip_current_dir();
        }
    }
    found
}

/// A bundle made on several platforms keeps each platform's binaries in
/// `lib_dir/<platform>/`. Moves the files of the folder that fits this
/// machine up into `lib_dir` and deletes every platform folder, so only
/// the binary that can run here stays on disk.
///
/// Returns what shipped but cannot run here: empty when a folder fit,
/// or when `lib_dir` has no platform folders at all (a bundle from an
/// older baler keeps its binaries directly in `lib_dir`).
fn select_platform_folder(lib_dir: &Path, host_triple: &str, host_r_version: &str) -> Result<Vec<AbiInfo>> {
    let mut folders: Vec<PathBuf> = std::fs::read_dir(lib_dir)
        .with_context(|| format!("Failed to read {}", lib_dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    if folders.is_empty() {
        return Ok(Vec::new());
    }
    folders.sort();

    let mut unusable = Vec::new();
    let mut chosen: Option<&PathBuf> = None;
    for folder in &folders {
        let shipped = read_abi_sidecars(folder)?;
        let Some((_, first)) = shipped.first() else { continue };
        if shipped.iter().all(|(_, info)| info.is_compatible(host_triple, host_r_version)) {
            if chosen.is_none() {
                chosen = Some(folder);
            }
        } else {
            unusable.push(first.clone());
        }
    }

    if let Some(folder) = chosen {
        for entry in std::fs::read_dir(folder)
            .with_context(|| format!("Failed to read {}", folder.display()))?
        {
            let from = entry
                .with_context(|| format!("Failed to read an entry of {}", folder.display()))?
                .path();
            let Some(file_name) = from.file_name() else { continue };
            let to = lib_dir.join(file_name);
            std::fs::rename(&from, &to)
                .with_context(|| format!("Failed to move {} to {}", from.display(), to.display()))?;
        }
        unusable.clear();
    }

    for folder in &folders {
        std::fs::remove_dir_all(folder)
            .with_context(|| format!("Failed to remove {}", folder.display()))?;
    }

    Ok(unusable)
}

/// An archive with no native source (bundled with `binary = true` and
/// `keep_source = false`) can only ever use the binaries it shipped. If
/// none of them were built for this platform and R version, fail now
/// with a clear message, rather than leave a binary that cannot load.
fn check_binary_only(
    name: &str,
    lib_dirs: &[PathBuf],
    host: &Option<(String, String)>,
    foreign: &[AbiInfo],
) -> Result<()> {
    let Some((host_triple, host_r_version)) = host else {
        return Ok(());
    };

    let mut unusable = foreign.to_vec();
    for lib_dir in lib_dirs {
        for (_, info) in read_abi_sidecars(lib_dir)? {
            if !info.is_compatible(host_triple, host_r_version) {
                unusable.push(info);
            }
        }
    }
    if unusable.is_empty() {
        return Ok(());
    }

    bail!(
        "'{name}' ships binaries for {} and no native source, but this machine is {host_triple} (R {host_r_version}), so none of them can be used here.\n\
         Ask the module's author to add a binary for this platform (run `baler bundle` on it against the same archive), \
         or to bundle with `keep_source = true` under [compiled-code] in baler.toml so other platforms can build it from source.",
        describe(&unusable)
    );
}

/// `x86_64-pc-linux-gnu (R 4.4), aarch64-apple-darwin20 (R 4.4)`, each
/// platform once.
fn describe(infos: &[AbiInfo]) -> String {
    let mut seen = BTreeSet::new();
    infos
        .iter()
        .map(|info| format!("{} (R {})", info.target_triple, info.r_version))
        .filter(|text| seen.insert(text.clone()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn remove_native_source(native_dir: &Path) -> Result<()> {
    std::fs::remove_dir_all(native_dir)
        .with_context(|| format!("Failed to remove native source at {}", native_dir.display()))
}
