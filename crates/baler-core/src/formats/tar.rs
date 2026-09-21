use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
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
        Some(contents) => {
            let lock: crate::lockfile::BalerLock = toml::from_str(&contents).with_context(|| {
                format!("Failed to parse baler.lock embedded in {}", tar_path.display())
            })?;
            Ok(Some(lock))
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
