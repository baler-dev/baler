use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use flate2::{write::GzEncoder, Compression};
use tar::Builder;

use crate::baler_toml::{first_target, BinaryTarget, HEAD_LEN, LIB_DIR, PLATFORMS_DIR};

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
    let file = File::create(output_path)
        .with_context(|| format!("Failed to create: {}", output_path.display()))?;

    let enc = GzEncoder::new(file, Compression::default());
    let mut archive = Builder::new(enc);

    let top = format!("{}_{}", name, version);

    for entry in all_files(src_path, exclude, force_include) {
        let rel = entry
            .strip_prefix(src_path)
            .with_context(|| format!("Failed to strip prefix from {}", entry.display()))?;

        let tar_name = format!(
            "{}/{}/{}",
            top,
            name,
            rel.to_string_lossy().replace('\\', "/")
        );

        archive
            .append_path_with_name(&entry, &tar_name)
            .with_context(|| format!("Failed to add to archive: {tar_name}"))?;
    }

    // baler.toml and baler.lock land at the archive root, a sibling of
    // {name}/ — never inside the module's own namespace, so a module
    // file that happens to be named either of those can't collide
    // with them.
    let toml_path = project_root.join("baler.toml");
    let toml_tar_name = format!("{}/baler.toml", top);
    archive
        .append_path_with_name(&toml_path, &toml_tar_name)
        .with_context(|| format!("Failed to add baler.toml to archive: {toml_tar_name}"))?;

    let lock_path = project_root.join(crate::lockfile::LOCK_FILE_NAME);
    if lock_path.exists() {
        let lock_tar_name = format!("{}/baler.lock", top);
        archive
            .append_path_with_name(&lock_path, &lock_tar_name)
            .with_context(|| format!("Failed to add baler.lock to archive: {lock_tar_name}"))?;
    }

    archive.finish().context("Failed to finalize tar.gz archive")?;
    Ok(())
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

/// A `.lib/` directory found inside an archive, read without unpacking.
pub struct ShippedLib {
    /// The directory inside the module, for example `rcalc/.lib`.
    pub lib_dir: PathBuf,
    /// The platform each set of binaries in it was built for, read from
    /// the files' own headers: the files directly in `.lib/` first, then
    /// one entry per `.lib/.platforms/<name>/` directory. A set whose
    /// files are none of ELF, Mach-O or PE is left out.
    pub targets: Vec<BinaryTarget>,
}

/// Every `.lib/` directory in the archive, so an install can check the
/// binaries fit this machine before it replaces anything.
pub fn read_shipped_libs(tar_path: &Path) -> Result<Vec<ShippedLib>> {
    let file = File::open(tar_path)
        .with_context(|| format!("Failed to open: {}", tar_path.display()))?;

    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    let mut libs: BTreeMap<PathBuf, BTreeMap<String, Vec<Vec<u8>>>> = BTreeMap::new();

    for entry in archive.entries().context("Failed to read tar.gz entries")? {
        let mut entry = entry.context("Failed to read tar.gz entry")?;
        let raw_path = entry.path()?.to_path_buf();
        let stripped = strip_top_level(&raw_path)?;
        let Some((lib_dir, rest)) = lib_split(&stripped) else { continue };
        let sets = libs.entry(lib_dir).or_default();

        if !entry.header().entry_type().is_file() {
            continue;
        }
        let Some(set) = set_name(&rest) else { continue };
        let mut head = Vec::new();
        (&mut entry)
            .take(HEAD_LEN as u64)
            .read_to_end(&mut head)
            .with_context(|| format!("Failed to read {} from archive", stripped.display()))?;
        sets.entry(set).or_default().push(head);
    }

    Ok(libs
        .into_iter()
        .map(|(lib_dir, sets)| ShippedLib {
            lib_dir,
            targets: sets.values().filter_map(|heads| first_target(heads)).collect(),
        })
        .collect())
}

/// Which set of binaries a path inside `.lib/` belongs to: `""` for a file
/// directly in `.lib/`, the directory name for `.platforms/<name>/<file>`.
fn set_name(rest: &Path) -> Option<String> {
    let parts: Vec<String> = rest
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    match parts.as_slice() {
        [_file] => Some(String::new()),
        [dir, name, _file] if dir == PLATFORMS_DIR => Some(name.clone()),
        _ => None,
    }
}

/// One file of an archive, read whole.
struct Member {
    path: PathBuf,
    mode: u32,
    mtime: u64,
    bytes: Vec<u8>,
}

/// One set of binaries for one platform.
struct Group<'a> {
    lib_dir: PathBuf,
    /// The platform, as a directory name, for example `macos-aarch64`.
    name: String,
    files: Vec<&'a Member>,
}

/// Carry the binaries of an earlier bundle of the same module over into a
/// freshly made one, writing the combined archive to `output`.
///
/// `fresh` is what `baler bundle` just built on this machine. It stays as
/// it is, so its binary is in `.lib/`, where an older baler looks. Every
/// platform `existing` holds that `fresh` doesn't goes to
/// `.lib/.platforms/<os>-<cpu>/`, named after what each binary's own
/// header says. Install picks the set that fits the machine and moves it
/// into `.lib/`, so nothing is compiled there. A platform in both comes
/// from `fresh`.
///
/// Returns `false`, and writes nothing, when `existing` holds different
/// source: its binaries belong to code that is no longer there.
pub fn add_platforms(fresh: &Path, existing: &Path, output: &Path) -> Result<bool> {
    if output == fresh || output == existing {
        bail!("The combined archive can't overwrite one of its inputs.");
    }

    let fresh_members = read_members(fresh)?;
    let old_members = read_members(existing)?;
    if source_files(&fresh_members) != source_files(&old_members) {
        return Ok(false);
    }

    let mut kept: BTreeMap<(PathBuf, String), Vec<&Member>> = BTreeMap::new();
    for group in groups(&fresh_members, fresh)? {
        kept.insert((group.lib_dir.clone(), group.name.clone()), group.files);
    }

    let mut added: Vec<(PathBuf, &Member)> = Vec::new();
    for group in groups(&old_members, existing)? {
        let key = (group.lib_dir.clone(), group.name.clone());
        if kept.contains_key(&key) {
            continue;
        }
        for member in &group.files {
            let Some(file) = member.path.file_name() else { continue };
            let at = group.lib_dir.join(PLATFORMS_DIR).join(&group.name).join(file);
            added.push((at, *member));
        }
        kept.insert(key, group.files);
    }

    let file = File::create(output)
        .with_context(|| format!("Failed to create: {}", output.display()))?;
    let enc = GzEncoder::new(file, Compression::default());
    let mut archive = Builder::new(enc);

    for member in &fresh_members {
        append_bytes(&mut archive, &member.path, &member.bytes, member.mode, member.mtime)?;
    }
    for (path, member) in &added {
        append_bytes(&mut archive, path, &member.bytes, member.mode, member.mtime)?;
    }

    archive.finish().context("Failed to finalize tar.gz archive")?;
    Ok(true)
}

fn read_members(tar_path: &Path) -> Result<Vec<Member>> {
    let file = File::open(tar_path)
        .with_context(|| format!("Failed to open: {}", tar_path.display()))?;

    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    let mut members = Vec::new();
    for entry in archive.entries().context("Failed to read tar.gz entries")? {
        let mut entry = entry.context("Failed to read tar.gz entry")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.to_path_buf();
        let mode = entry.header().mode().unwrap_or(0o644);
        let mtime = entry.header().mtime().unwrap_or(0);
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).with_context(|| {
            format!("Failed to read {} from {}", path.display(), tar_path.display())
        })?;
        members.push(Member { path, mode, mtime, bytes });
    }
    Ok(members)
}

/// Every set of binaries in an archive, each named after the platform its
/// files' headers say they were built for.
fn groups<'a>(members: &'a [Member], origin: &Path) -> Result<Vec<Group<'a>>> {
    let mut sets: BTreeMap<(PathBuf, String), Vec<&Member>> = BTreeMap::new();
    for member in members {
        let Some((lib_dir, rest)) = lib_split(&member.path) else { continue };
        let Some(set) = set_name(&rest) else { continue };
        sets.entry((lib_dir, set)).or_default().push(member);
    }

    let mut found = Vec::new();
    for ((lib_dir, _), files) in sets {
        let heads: Vec<Vec<u8>> = files
            .iter()
            .map(|member| member.bytes[..member.bytes.len().min(HEAD_LEN)].to_vec())
            .collect();
        let Some(target) = first_target(&heads) else {
            bail!(
                "{} has files in {} that are not a recognisable ELF, Mach-O or PE binary, so \
                 the platform they were built for is unknown. Bundle it on the platform it is \
                 for, then merge.",
                origin.display(),
                slash(&lib_dir)
            );
        };
        let Some(name) = target.dir_name() else {
            bail!(
                "{} has a binary in {} for a CPU baler doesn't know ({}).",
                origin.display(),
                slash(&lib_dir),
                target.describe()
            );
        };
        found.push(Group { lib_dir, name, files });
    }
    Ok(found)
}

/// Everything in an archive except `.lib/` contents, by path.
///
/// A text file is read with CRLF as LF. A Git checkout on Windows turns
/// every line ending into CRLF, so the same commit bundled on Windows and
/// on Linux differs byte for byte, and would never count as the same
/// source. A file with a NUL byte is a binary and is compared as it is.
fn source_files(members: &[Member]) -> BTreeMap<String, Vec<u8>> {
    members
        .iter()
        .filter(|member| lib_split(&member.path).is_none())
        .map(|member| (slash(&member.path), lf_text(&member.bytes)))
        .collect()
}

fn lf_text(bytes: &[u8]) -> Vec<u8> {
    if bytes.contains(&0) {
        return bytes.to_vec();
    }
    let mut text = Vec::with_capacity(bytes.len());
    for (at, byte) in bytes.iter().enumerate() {
        if *byte == b'\r' && bytes.get(at + 1) == Some(&b'\n') {
            continue;
        }
        text.push(*byte);
    }
    text
}

fn append_bytes(
    archive: &mut Builder<GzEncoder<File>>,
    path: &Path,
    bytes: &[u8],
    mode: u32,
    mtime: u64,
) -> Result<()> {
    let name = slash(path);
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(mode);
    header.set_mtime(mtime);
    header.set_cksum();
    archive
        .append_data(&mut header, &name, bytes)
        .with_context(|| format!("Failed to add to archive: {name}"))
}

/// Splits a path at its `.lib` component into the path through `.lib`
/// and what is inside it. `None` when the path isn't inside a `.lib`.
fn lib_split(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let components: Vec<_> = path.components().collect();
    let at = components.iter().position(|c| c.as_os_str() == LIB_DIR)?;
    let lib_dir: PathBuf = components[..=at].iter().collect();
    let rest: PathBuf = components[at + 1..].iter().collect();
    Some((lib_dir, rest))
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
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
