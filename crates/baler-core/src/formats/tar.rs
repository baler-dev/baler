use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use flate2::{write::GzEncoder, Compression};
use tar::Builder;

/// Bundle a module into a `.tar.gz` archive.
///
/// Archive structure:
/// ``` text
/// tstk_0.1.0/
/// ├── tstk/
/// │   ├── __init__.R
/// │   ├── decomp/
/// │   └── ...
/// ├── baler.toml
/// └── baler.lock        (only if the project has one)
/// ```
///
/// `baler.toml` and `baler.lock` are shipped as-is, the actual source
/// files, not a generated re-encoding of them. There used to be a
/// generated `manifest.json` here instead; it duplicated `baler.toml`
/// for no reason a source bundle needs (an sdist-style archive ships
/// its real manifest directly, the same way a Python sdist ships
/// `pyproject.toml` and a Julia package tarball ships `Project.toml`),
/// carried a timestamp nobody asked for, and its one genuinely
/// generated field (native artifact metadata: target_triple/r_version/
/// source_hash) was dead code, nothing downstream ever read it.
pub fn bundle(
    src_path: &Path,
    project_root: &Path,
    output_path: &Path,
    name: &str,
    version: &str,
    exclude: &[PathBuf],
    force_include: &[PathBuf],
) -> Result<()> {
    let spec = BundleSpec {
        src_path,
        project_root,
        output_path,
        name,
        version,
        exclude,
        force_include,
    };
    let planned = plan(&spec, None)?;
    let file = File::create(output_path)
        .with_context(|| format!("Failed to create: {}", output_path.display()))?;
    write_archive(file, &planned, &[])
}

/// Everything needed to write one bundle.
pub struct BundleSpec<'a> {
    pub src_path: &'a Path,
    pub project_root: &'a Path,
    pub output_path: &'a Path,
    pub name: &'a str,
    pub version: &'a str,
    pub exclude: &'a [PathBuf],
    pub force_include: &'a [PathBuf],
}

/// Directory name for one platform's binaries under `.lib/`, built from
/// the target triple and R `major.minor` that `.abi.json` records, e.g.
/// `x86_64-pc-linux-gnu_r4.4`. Anything outside `A-Za-z0-9._-` becomes
/// `_`, so the tag is always a single safe path component.
pub fn platform_tag(target_triple: &str, r_version: &str) -> String {
    let clean = |text: &str| -> String {
        text.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
            .collect()
    };
    format!("{}_r{}", clean(target_triple), clean(r_version))
}

/// Bundle like `bundle`, but store the compiled binaries under
/// `.lib/<tag>/` and keep the binaries of other platforms that an
/// earlier run left in the same `output_path`:
/// ``` text
/// tstk_0.1.0/tstk/src/.lib/x86_64-pc-linux-gnu_r4.4/cpp.so
/// tstk_0.1.0/tstk/src/.lib/aarch64-apple-darwin20_r4.4/cpp.so
/// ```
/// Running `baler bundle` on each platform against the same archive
/// builds one bundle that holds every platform's binary, and install
/// keeps only the one that fits the machine.
///
/// An existing archive is only reused when everything outside `.lib/`
/// is byte for byte the same as what is being bundled now, otherwise
/// binaries from other source would be shipped under this version, and
/// the call fails without touching the archive. An archive that holds
/// no other platform's binary is simply replaced. The new archive is
/// written next to `output_path` and renamed over it, so a failure
/// leaves the old archive in place.
///
/// Returns the tags of every platform the archive now holds, sorted.
pub fn bundle_with_binary(spec: &BundleSpec, tag: &str) -> Result<Vec<String>> {
    let planned = plan(spec, Some(tag))?;
    let top = format!("{}_{}", spec.name, spec.version);

    let mut carried: Vec<CarriedEntry> = Vec::new();
    if spec.output_path.exists() {
        match read_previous(spec.output_path) {
            Ok(previous) => {
                let others: Vec<CarriedEntry> = previous
                    .binaries
                    .into_iter()
                    .filter(|entry| entry.tag != tag)
                    .collect();
                if !others.is_empty() {
                    if !same_sources(&planned, &previous.sources, &top)? {
                        let tags: BTreeSet<&str> = others.iter().map(|entry| entry.tag.as_str()).collect();
                        bail!(
                            "{} already holds binaries for {}, but the module's files differ from the ones it was bundled from, so those binaries may not belong to this source.\nTo start over and drop those binaries, delete {} and bundle again. To keep them, bundle the same files.",
                            spec.output_path.display(),
                            tags.into_iter().collect::<Vec<_>>().join(", "),
                            spec.output_path.display()
                        );
                    }
                    if previous.untagged > 0 {
                        eprintln!(
                            "warning: {} holds {} binary file(s) from an older baler that are not tied to a platform. They are not carried over.",
                            spec.output_path.display(),
                            previous.untagged
                        );
                    }
                    carried = others;
                }
            }
            Err(e) => eprintln!(
                "warning: {} exists but could not be read ({:#}). It is replaced.",
                spec.output_path.display(),
                e
            ),
        }
    }

    let mut tags: BTreeSet<String> = carried.iter().map(|entry| entry.tag.clone()).collect();
    tags.insert(tag.to_owned());

    replace_archive(spec.output_path, &planned, &carried)?;

    Ok(tags.into_iter().collect())
}

/// Combine the binaries of several bundles of one module, made on other
/// platforms, with the binary compiled here, into a single bundle at
/// `spec.output_path`:
/// ``` text
/// this platform + foo_1.0.0.tar.gz (linux) + foo_1.0.0.tar.gz (macos) -> foo_1.0.0.tar.gz (all three)
/// ```
///
/// The module's own files come from `spec`. When a binary was compiled
/// here, `spec.force_include` holds its `.lib/` directories and
/// `local_tag` names its platform, as for `bundle_with_binary`; both are
/// empty and `None` when nothing was compiled, and only the inputs'
/// binaries are combined.
///
/// Every file outside `.lib/` in each input must be byte for byte the
/// same as the module's files, so binaries built from different source
/// are never combined, and the call fails naming the input that differs.
/// A platform that appears in more than one input fails too, and so does
/// an input that holds `local_tag`, since there is no telling which
/// binary to keep. Nothing is written when the call fails, and
/// `spec.output_path` may be one of the inputs, because every input is
/// read before the output is replaced.
///
/// Returns the tags of every platform the new archive holds, sorted.
pub fn merge_bundles(spec: &BundleSpec, inputs: &[PathBuf], local_tag: Option<&str>) -> Result<Vec<String>> {
    if inputs.is_empty() {
        bail!("Nothing to merge, pass at least one archive.");
    }

    let planned = plan(spec, local_tag)?;
    let top = format!("{}_{}", spec.name, spec.version);

    let mut carried: Vec<CarriedEntry> = Vec::new();
    let mut first_seen: BTreeMap<String, usize> = BTreeMap::new();
    for (index, input) in inputs.iter().enumerate() {
        let previous = read_previous(input)
            .with_context(|| format!("Failed to read {}", input.display()))?;

        if !same_sources(&planned, &previous.sources, &top)? {
            bail!(
                "{} was bundled from files that differ from the module at {}, so its binaries may not belong to this source. Bundle it again from the current files.",
                input.display(),
                spec.project_root.display()
            );
        }
        if previous.untagged > 0 {
            eprintln!(
                "warning: {} holds {} binary file(s) from an older baler that are not tied to a platform. They are not carried over.",
                input.display(),
                previous.untagged
            );
        }

        for entry in previous.binaries {
            if Some(entry.tag.as_str()) == local_tag {
                bail!(
                    "{} holds a binary for {}, the platform being bundled here. Leave it out of the merge, or bundle on another platform.",
                    input.display(),
                    entry.tag
                );
            }
            match first_seen.get(&entry.tag) {
                Some(&earlier) if earlier != index => bail!(
                    "{} and {} both hold a binary for {}. Merge one of them only.",
                    inputs[earlier].display(),
                    input.display(),
                    entry.tag
                ),
                _ => {
                    first_seen.insert(entry.tag.clone(), index);
                    carried.push(entry);
                }
            }
        }
    }

    if carried.is_empty() {
        bail!("None of the archives holds a binary to merge.");
    }

    let mut tags: BTreeSet<String> = carried.iter().map(|entry| entry.tag.clone()).collect();
    if let Some(tag) = local_tag {
        tags.insert(tag.to_owned());
    }
    replace_archive(spec.output_path, &planned, &carried)?;

    Ok(tags.into_iter().collect())
}

/// Writes the archive next to `output_path` and renames it into place,
/// so a failure leaves whatever was at `output_path` as it was.
fn replace_archive(output_path: &Path, planned: &[Planned], carried: &[CarriedEntry]) -> Result<()> {
    let file_name = output_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("bundle.tar.gz");
    let partial = output_path.with_file_name(format!("{}.partial", file_name));
    let file = File::create(&partial)
        .with_context(|| format!("Failed to create: {}", partial.display()))?;
    if let Err(e) = write_archive(file, planned, carried) {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }
    std::fs::rename(&partial, output_path)
        .with_context(|| format!("Failed to move {} to {}", partial.display(), output_path.display()))
}

const LIB_DIR: &str = ".lib";

/// One file to write: where it is on disk and its name in the archive.
/// `binary` marks files under a forced-in `.lib/`.
struct Planned {
    disk: PathBuf,
    tar_name: String,
    binary: bool,
}

/// A binary from an earlier archive, kept as it was written.
struct CarriedEntry {
    tag: String,
    tar_name: String,
    header: tar::Header,
    bytes: Vec<u8>,
}

/// What an earlier archive at the same path holds: every file outside
/// `.lib/` by its path inside the top directory, the binaries stored
/// under `.lib/<tag>/`, and a count of files directly under `.lib/`
/// (written by a baler that did not tag binaries by platform).
struct Previous {
    sources: BTreeMap<String, Vec<u8>>,
    binaries: Vec<CarriedEntry>,
    untagged: usize,
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// The files of a bundle in the order they are written. With a `tag`,
/// files under a forced-in `.lib/` get the tag as an extra directory.
fn plan(spec: &BundleSpec, tag: Option<&str>) -> Result<Vec<Planned>> {
    let top = format!("{}_{}", spec.name, spec.version);
    let mut planned = Vec::new();

    for entry in all_files(spec.src_path, spec.exclude, spec.force_include) {
        let rel = entry
            .strip_prefix(spec.src_path)
            .with_context(|| format!("Failed to strip prefix from {}", entry.display()))?;
        let lib_dir = spec.force_include.iter().find(|dir| entry.starts_with(dir));

        let inner = match (tag, lib_dir) {
            (Some(tag), Some(lib_dir)) => {
                let lib_rel = lib_dir
                    .strip_prefix(spec.src_path)
                    .with_context(|| format!("Failed to strip prefix from {}", lib_dir.display()))?;
                let inside = entry
                    .strip_prefix(lib_dir)
                    .with_context(|| format!("Failed to strip prefix from {}", entry.display()))?;
                format!("{}/{}/{}", slash(lib_rel), tag, slash(inside))
            }
            _ => slash(rel),
        };

        let binary = lib_dir.is_some();
        planned.push(Planned {
            disk: entry,
            tar_name: format!("{}/{}/{}", top, spec.name, inner),
            binary,
        });
    }

    // baler.toml and baler.lock land at the archive root, a sibling of
    // {name}/, never inside the module's own namespace, so a module
    // file that happens to be named either of those can't collide
    // with them.
    planned.push(Planned {
        disk: spec.project_root.join("baler.toml"),
        tar_name: format!("{}/baler.toml", top),
        binary: false,
    });

    let lock_path = spec.project_root.join(crate::lockfile::LOCK_FILE_NAME);
    if lock_path.exists() {
        planned.push(Planned {
            disk: lock_path,
            tar_name: format!("{}/baler.lock", top),
            binary: false,
        });
    }

    Ok(planned)
}

fn write_archive(file: File, planned: &[Planned], carried: &[CarriedEntry]) -> Result<()> {
    let enc = GzEncoder::new(file, Compression::default());
    let mut archive = Builder::new(enc);

    for item in planned {
        archive
            .append_path_with_name(&item.disk, &item.tar_name)
            .with_context(|| format!("Failed to add to archive: {}", item.tar_name))?;
    }

    for entry in carried {
        let mut header = entry.header.clone();
        archive
            .append_data(&mut header, &entry.tar_name, &entry.bytes[..])
            .with_context(|| format!("Failed to carry over: {}", entry.tar_name))?;
    }

    let enc = archive.into_inner().context("Failed to finalize tar archive")?;
    enc.finish().context("Failed to finalize gzip stream")?;
    Ok(())
}

fn read_previous(path: &Path) -> Result<Previous> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open: {}", path.display()))?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    let mut previous = Previous {
        sources: BTreeMap::new(),
        binaries: Vec::new(),
        untagged: 0,
    };

    for entry in archive.entries().context("Failed to read tar.gz entries")? {
        let mut entry = entry.context("Failed to read tar.gz entry")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let raw = entry.path().context("Failed to get entry path")?.to_path_buf();
        let stripped = strip_top_level(&raw)?;
        let parts: Vec<String> = stripped
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();

        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .with_context(|| format!("Failed to read {} from archive", raw.display()))?;

        match parts.iter().position(|part| part == LIB_DIR) {
            None => {
                previous.sources.insert(slash(&stripped), bytes);
            }
            Some(i) if parts.len() > i + 2 => previous.binaries.push(CarriedEntry {
                tag: parts[i + 1].clone(),
                tar_name: slash(&raw),
                header: entry.header().clone(),
                bytes,
            }),
            Some(_) => previous.untagged += 1,
        }
    }

    Ok(previous)
}

/// True when every non-binary file about to be written matches, byte
/// for byte, the file of the same path in the earlier archive, and the
/// earlier archive has no other such file.
fn same_sources(planned: &[Planned], previous: &BTreeMap<String, Vec<u8>>, top: &str) -> Result<bool> {
    let prefix = format!("{}/", top);
    let mut seen = BTreeSet::new();
    for item in planned.iter().filter(|item| !item.binary) {
        let key = item.tar_name.strip_prefix(&prefix).unwrap_or(&item.tar_name).to_owned();
        let current = std::fs::read(&item.disk)
            .with_context(|| format!("Failed to read {}", item.disk.display()))?;
        match previous.get(&key) {
            Some(old) if *old == current => {}
            _ => return Ok(false),
        }
        seen.insert(key);
    }
    Ok(seen.len() == previous.len())
}

/// Unpack a `.tar.gz` baler archive into the install directory.
///
/// Strips the top-level `{name}_{version}/` prefix so the result is:
/// ``` text
/// <install_dir>/tstk/
///     __init__.R
///     decomp/
///     ...
/// <install_dir>/tstk-0.1.0.dist-info/
///     baler.toml
///     baler.lock       (only if the archive shipped one)
/// ```
pub fn unpack(tar_path: &Path, install_dir: &Path, name: &str, version: &str) -> Result<()> {
    let file = File::open(tar_path)
        .with_context(|| format!("Failed to open: {}", tar_path.display()))?;

    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    let dist_info_dir = install_dir.join(format!("{}-{}.dist-info", name, version));
    std::fs::create_dir_all(&dist_info_dir)
        .with_context(|| format!("Failed to create dist-info dir: {}", dist_info_dir.display()))?;

    for entry in archive.entries().context("Failed to read tar.gz entries")? {
        let mut entry = entry.context("Failed to read tar.gz entry")?;
        let raw_path = entry.path()
            .context("Failed to get entry path")?
            .to_path_buf();

        // Strip top-level {name}_{version}/ prefix
        let stripped = strip_top_level(&raw_path)?;

        if stripped == Path::new("") || stripped == Path::new(".") {
            continue;
        }

        // Only the reserved root-level baler.toml/baler.lock go into
        // .dist-info. Matching by full path (not basename) means a
        // module file that happens to be named either — however deep
        // — is never mistaken for it and misrouted. Multiple modules
        // share one install_dir, so these can't land loose at
        // install_dir's own root, each module's copy would overwrite
        // the last one's.
        let dest = if stripped == Path::new("baler.toml") {
            dist_info_dir.join("baler.toml")
        } else if stripped == Path::new("baler.lock") {
            dist_info_dir.join("baler.lock")
        } else {
            install_dir.join(&stripped)
        };

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create dir: {}", parent.display()))?;
        }

        entry
            .unpack(&dest)
            .with_context(|| format!("Failed to unpack: {}", dest.display()))?;
    }

    Ok(())
}

/// Read the `baler.toml` embedded in a `.tar.gz` without unpacking the
/// rest of the archive.
pub fn read_toml(tar_path: &Path) -> Result<crate::baler_toml::BalerToml> {
    let contents = read_embedded_file(tar_path, "baler.toml")?.with_context(|| {
        format!(
            "No baler.toml found in {}. Is this a valid baler package?",
            tar_path.display()
        )
    })?;

    toml::from_str(&contents)
        .with_context(|| format!("Failed to parse baler.toml embedded in {}", tar_path.display()))
}

/// Read the `baler.lock` embedded in a `.tar.gz`, if the archive
/// shipped one. `Ok(None)` means the module was bundled without a
/// lock, not an error, same meaning `lockfile::read` gives a missing
/// file on disk.
pub fn read_lock(tar_path: &Path) -> Result<Option<crate::lockfile::BalerLock>> {
    match read_embedded_file(tar_path, "baler.lock")? {
        // Same parse, and same format-version check, as a lock read from
        // disk, so an embedded lock from a newer baler is refused with the
        // same clear message instead of being half-understood.
        Some(contents) => {
            let origin = format!("baler.lock embedded in {}", tar_path.display());
            Ok(Some(crate::lockfile::parse(&contents, &origin)?))
        }
        None => Ok(None),
    }
}

/// Read one root-level file out of a `.tar.gz` by its stripped path,
/// without unpacking the rest of the archive. Shared by `read_toml`
/// and `read_lock`, the only difference between them is which name
/// they look for and how the result gets parsed.
fn read_embedded_file(tar_path: &Path, name: &str) -> Result<Option<String>> {
    let file = File::open(tar_path)
        .with_context(|| format!("Failed to open: {}", tar_path.display()))?;

    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    for entry in archive.entries().context("Failed to read tar.gz entries")? {
        let mut entry = entry.context("Failed to read tar.gz entry")?;
        let raw_path = entry.path()?.to_path_buf();
        let stripped = strip_top_level(&raw_path)?;

        if stripped == Path::new(name) {
            let mut s = String::new();
            std::io::Read::read_to_string(&mut entry, &mut s)
                .with_context(|| format!("Failed to read {} from archive", name))?;
            return Ok(Some(s));
        }
    }

    Ok(None)
}

pub fn collect_files(base: &Path) -> Result<Vec<String>> {
    all_files(base, &[], &[])
        .iter()
        .map(|p| {
            p.strip_prefix(base)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .with_context(|| format!("Failed to strip prefix from {}", p.display()))
        })
        .collect()
}

fn strip_top_level(path: &Path) -> Result<PathBuf> {
    let mut components = path.components();
    components.next();
    Ok(components.as_path().to_path_buf())
}

fn all_files(base: &Path, exclude: &[PathBuf], force_include: &[PathBuf]) -> Vec<PathBuf> {
    walkdir::WalkDir::new(base)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| !exclude.iter().any(|ex| e.path().starts_with(ex)))
        .filter(|e| {
            let forced = force_include.iter().any(|f| e.path().starts_with(f));
            forced || e.path()
                .strip_prefix(base)
                .unwrap_or(e.path())
                .components()
                .filter_map(|c| {
                    let s = c.as_os_str().to_string_lossy();
                    if s == "." || s == ".." { None } else { Some(s.starts_with('.')) }
                })
                .all(|is_hidden| !is_hidden)
        })
        .map(|e| e.path().to_owned())
        .collect()
}
