use anyhow::{bail, Context, Result};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::baler_toml::{first_target, BalerToml, BinaryTarget, HEAD_LEN, LIB_DIR, PLATFORMS_DIR};
use crate::ops::{compile, resolve};

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
/// `[dependencies]` and skipping `baler.lock`, since they're
/// compile-time-only, not a runtime contract.
pub(super) fn build_native_if_present(
    module_path: &PathBuf,
    name: &str,
    install_deps: bool,
    toml: &BalerToml,
) -> Result<()> {
    let native_dirs = toml.native_dirs_under(module_path);

    if native_dirs.is_empty() {
        return use_shipped_binary(module_path, name);
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

/// The message for a bundle whose binaries don't fit this machine and
/// which has no native source to build from. Also used by the check
/// `install_from_tar` makes before it touches the installed copy.
pub(super) fn unusable_binary_message(name: &str, found: &str, host: &str) -> String {
    format!(
        "'{name}' ships compiled code for {found}, but this machine is {host}.\n\
         The bundle has no native source to build from, so it can't be used here.\n\
         Ask the author for a bundle that includes {host}: bundle on that platform, then \
         merge it in."
    )
}

/// The install of a bundle that carries binaries and no native source.
fn use_shipped_binary(module_path: &Path, name: &str) -> Result<()> {
    match activate_shipped_binaries(module_path)? {
        Shipped::Usable { host } => println!(" [native] {} ships a binary for {}, using it", name, host),
        Shipped::Foreign { host, found } => bail!("{}", unusable_binary_message(name, &found, &host)),
        Shipped::Absent => {}
    }
    Ok(())
}

enum Shipped {
    /// The module ships no compiled binary.
    Absent,
    /// A binary that fits is now in `.lib/`.
    Usable { host: String },
    /// A binary was shipped, none of it fits, and it has been deleted.
    Foreign { host: String, found: String },
}

/// One set of compiled files in a `.lib/`: the files directly inside it,
/// or the files of one `.platforms/<name>/` directory.
struct BinarySet {
    /// `None` is `.lib/` itself.
    dir: Option<PathBuf>,
    files: Vec<PathBuf>,
    /// `None` when none of the files is a known binary format.
    target: Option<BinaryTarget>,
}

/// Looks at every `.lib/` in the installed module and leaves behind
/// only binaries that fit this machine.
fn activate_shipped_binaries(module_path: &Path) -> Result<Shipped> {
    let lib_dirs = find_lib_dirs(module_path);
    if lib_dirs.is_empty() {
        return Ok(Shipped::Absent);
    }

    let host = compile::host_target()?;
    let mut activated = false;
    let mut found_elsewhere: Vec<String> = Vec::new();

    for lib_dir in lib_dirs {
        let sets = binary_sets(&lib_dir)?;
        if sets.is_empty() {
            continue;
        }

        // A platform baler doesn't recognise can't be judged, so what is
        // there is left alone, as it always was.
        let Some(host) = &host else {
            activated = true;
            continue;
        };

        // A file that is no known binary format can't be judged either,
        // and counts as fitting.
        let chosen = sets
            .iter()
            .position(|set| set.target.as_ref().map_or(true, |target| target.fits(host)));

        match chosen {
            Some(at) => {
                use_set(&lib_dir, &sets[at])?;
                activated = true;
            }
            None => {
                // Left in place, a binary that can't load would only fail
                // later at box::use(), with an error that doesn't point here.
                std::fs::remove_dir_all(&lib_dir)
                    .with_context(|| format!("Failed to remove {}", lib_dir.display()))?;
                found_elsewhere.extend(sets.iter().filter_map(|set| set.target.as_ref().map(BinaryTarget::describe)));
            }
        }
    }

    let host_name = host.as_ref().map(BinaryTarget::describe);
    Ok(match host_name {
        Some(host) if !found_elsewhere.is_empty() => Shipped::Foreign { host, found: found_elsewhere.join(", ") },
        host if activated => Shipped::Usable { host: host.unwrap_or_else(|| "this machine".to_owned()) },
        _ => Shipped::Absent,
    })
}

/// Every `.lib` directory under the module, without looking inside one.
fn find_lib_dirs(module_path: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut walk = walkdir::WalkDir::new(module_path).into_iter();
    while let Some(entry) = walk.next() {
        let Ok(entry) = entry else { continue };
        if entry.file_type().is_dir() && entry.file_name() == LIB_DIR {
            found.push(entry.into_path());
            walk.skip_current_dir();
        }
    }
    found
}

fn files_in(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("Failed to read {}", dir.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}

fn read_heads(files: &[PathBuf]) -> Result<Vec<Vec<u8>>> {
    files
        .iter()
        .map(|file| {
            let mut head = Vec::with_capacity(HEAD_LEN);
            std::fs::File::open(file)
                .and_then(|open| open.take(HEAD_LEN as u64).read_to_end(&mut head))
                .with_context(|| format!("Failed to read {}", file.display()))?;
            Ok(head)
        })
        .collect()
}

/// The legacy slot (`.lib/` itself) first, then each `.platforms/<name>/`.
fn binary_sets(lib_dir: &Path) -> Result<Vec<BinarySet>> {
    let mut sets = Vec::new();

    let files = files_in(lib_dir)?;
    if !files.is_empty() {
        let target = first_target(&read_heads(&files)?);
        sets.push(BinarySet { dir: None, files, target });
    }

    let platforms = lib_dir.join(PLATFORMS_DIR);
    if platforms.is_dir() {
        let mut dirs = Vec::new();
        for entry in std::fs::read_dir(&platforms)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                dirs.push(entry.path());
            }
        }
        dirs.sort();
        for dir in dirs {
            let files = files_in(&dir)?;
            if files.is_empty() {
                continue;
            }
            let target = first_target(&read_heads(&files)?);
            sets.push(BinarySet { dir: Some(dir), files, target });
        }
    }

    Ok(sets)
}

/// Makes `.lib/` hold exactly `set`, which is where `box::file()` looks.
/// The other platforms' files go.
fn use_set(lib_dir: &Path, set: &BinarySet) -> Result<()> {
    if set.dir.is_some() {
        for file in files_in(lib_dir)? {
            std::fs::remove_file(&file).with_context(|| format!("Failed to remove {}", file.display()))?;
        }
        for file in &set.files {
            let Some(name) = file.file_name() else { continue };
            std::fs::rename(file, lib_dir.join(name))
                .with_context(|| format!("Failed to move {} into {}", file.display(), lib_dir.display()))?;
        }
    }

    let platforms = lib_dir.join(PLATFORMS_DIR);
    if platforms.exists() {
        std::fs::remove_dir_all(&platforms)
            .with_context(|| format!("Failed to remove {}", platforms.display()))?;
    }
    Ok(())
}
