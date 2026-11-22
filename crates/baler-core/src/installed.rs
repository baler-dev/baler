//! What is installed, read back from the install directory.
//!
//! A module `tstk` at version 0.2.0 is two things on disk, side by side
//! in the install directory:
//!
//! ``` text
//! tstk/                   the module's source, what `box::use()` loads
//! tstk-0.2.0.dist-info/   what baler kept about it: baler.toml, and
//!                         baler.lock, the license and the README when
//!                         the bundle had them
//! ```
//!
//! The module directory says a module is installed. The `.dist-info`
//! folder says which version it is, and what it declared.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{bail, Context, Result};

use crate::baler_toml::BalerToml;
use crate::project_files::{is_license_file_name, is_readme_file_name};

const DIST_INFO_SUFFIX: &str = ".dist-info";

/// A module name is one plain path component. Anything else, such as
/// `.`, `..` or `a/b`, would make `<install_dir>/<name>` point at the
/// install directory itself or outside it, and a removal there would
/// delete more than one module.
pub fn validate_module_name(name: &str) -> Result<()> {
    let mut parts = Path::new(name).components();
    let single = matches!(
        (parts.next(), parts.next()),
        (Some(std::path::Component::Normal(part)), None) if part == std::ffi::OsStr::new(name)
    );
    if !single {
        bail!(
            "'{}' is not a module name. Give the name of an installed module, \
             as `baler list` shows it.",
            name
        );
    }
    Ok(())
}

/// `<name>-<version>.dist-info`.
pub fn dist_info_dir_name(name: &str, version: &str) -> String {
    format!("{name}-{version}{DIST_INFO_SUFFIX}")
}

/// Splits `<name>-<version>.dist-info` into the name and the version.
///
/// Both can hold a `-`: a module may be called `foo-bar`, and a version
/// may carry a pre-release tag, as in `0.3.0-dev`. So the divide is the
/// first `-` after which what remains is a valid semver version. That
/// reads `foo-bar-0.3.0-dev` as `foo-bar` and `0.3.0-dev`, and never as
/// `foo` and `bar-0.3.0-dev`, which is not a version.
fn parse_dist_info_dir_name(dir_name: &str) -> Option<(&str, &str)> {
    let stem = dir_name.strip_suffix(DIST_INFO_SUFFIX)?;
    for (at, _) in stem.match_indices('-') {
        let (name, version) = (&stem[..at], &stem[at + 1..]);
        if !name.is_empty() && semver::Version::parse(version).is_ok() {
            return Some((name, version));
        }
    }
    None
}

/// Every `.dist-info` folder in `install_dir` that belongs to `name`,
/// whatever its version.
pub fn dist_info_dirs_for(install_dir: &Path, name: &str) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for (dir_name, path) in dist_info_dirs(install_dir)? {
        if parse_dist_info_dir_name(&dir_name).is_some_and(|(owner, _)| owner == name) {
            found.push(path);
        }
    }
    Ok(found)
}

/// Deletes every `.dist-info` folder of `name`, whatever its version, and
/// says how many there were. Install uses it so an upgrade leaves only the
/// new version's folder, and remove uses it so nothing is left behind.
pub fn remove_dist_infos(install_dir: &Path, name: &str) -> Result<usize> {
    let dirs = dist_info_dirs_for(install_dir, name)?;
    for dir in &dirs {
        std::fs::remove_dir_all(dir)
            .with_context(|| format!("Failed to remove: {}", dir.display()))?;
    }
    Ok(dirs.len())
}

/// `(directory name, path)` of every `.dist-info` folder in `install_dir`.
/// An install directory that does not exist yet holds none.
fn dist_info_dirs(install_dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let entries = match std::fs::read_dir(install_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to read: {}", install_dir.display()))
        }
    };

    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(dir_name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if path.is_dir() && dir_name.ends_with(DIST_INFO_SUFFIX) {
            dirs.push((dir_name, path));
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// One installed module, as its `.dist-info` folder describes it.
#[derive(Debug)]
pub struct InstalledModule {
    pub name: String,
    pub version: String,
    /// The `baler.toml` the module was installed with.
    pub toml: BalerToml,
    /// `<install_dir>/<name>`.
    pub module_dir: PathBuf,
    pub dist_info: PathBuf,
}

impl InstalledModule {
    /// The license files kept with the module, sorted by name.
    pub fn license_files(&self) -> Vec<PathBuf> {
        self.files_in_dist_info(is_license_file_name)
    }

    /// The README kept with the module.
    pub fn readme_file(&self) -> Option<PathBuf> {
        self.files_in_dist_info(is_readme_file_name).into_iter().next()
    }

    /// The `baler.lock` the module came with, if it had one.
    pub fn lock_file(&self) -> Option<PathBuf> {
        let path = self.dist_info.join(crate::lockfile::LOCK_FILE_NAME);
        path.is_file().then_some(path)
    }

    /// Whether compiled code is installed with the module: a `.lib/`
    /// folder anywhere in its source.
    pub fn has_compiled_code(&self) -> bool {
        walkdir::WalkDir::new(&self.module_dir)
            .into_iter()
            .flatten()
            .any(|entry| entry.file_type().is_dir() && entry.file_name() == ".lib")
    }

    fn files_in_dist_info(&self, wanted: fn(&str) -> bool) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(&self.dist_info) else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| entry.path().is_file())
            .filter(|entry| entry.file_name().to_str().is_some_and(wanted))
            .map(|entry| entry.path())
            .collect();
        files.sort();
        files
    }
}

/// The modules in an install directory, plus what could not be read.
#[derive(Debug, Default)]
pub struct Listing {
    /// Sorted by name.
    pub modules: Vec<InstalledModule>,
    /// One line for each module that is there but unreadable, for the
    /// caller to show. They are left out of `modules`.
    pub problems: Vec<String>,
}

/// Every module installed in `install_dir`.
///
/// A module counts as installed when its directory is there. A
/// `.dist-info` folder without one is left over from a removal made by
/// baler 0.1.0, which did not delete it, and is ignored. When several
/// versions of one module have a folder, which a 0.1.0 upgrade also left
/// behind, the most recently written one is used.
pub fn list(install_dir: &Path) -> Result<Listing> {
    list_where(install_dir, |_| true)
}

/// Like [`list`], but only the modules whose name `wanted` accepts. A
/// module it turns down is never read, so a broken record of a module the
/// caller did not ask about is not reported either.
pub fn list_where(install_dir: &Path, wanted: impl Fn(&str) -> bool) -> Result<Listing> {
    let mut by_name: BTreeMap<String, Vec<(PathBuf, String)>> = BTreeMap::new();
    for (dir_name, path) in dist_info_dirs(install_dir)? {
        if let Some((name, version)) = parse_dist_info_dir_name(&dir_name) {
            by_name
                .entry(name.to_owned())
                .or_default()
                .push((path, version.to_owned()));
        }
    }

    let mut listing = Listing::default();
    for (name, candidates) in by_name {
        if !wanted(&name) {
            continue;
        }
        let module_dir = install_dir.join(&name);
        if !module_dir.is_dir() {
            continue;
        }
        let Some((dist_info, _)) = newest(candidates) else {
            continue;
        };
        match read_installed(&name, module_dir, dist_info) {
            Ok(module) => listing.modules.push(module),
            Err(e) => listing.problems.push(format!("{name}: {e:#}")),
        }
    }
    Ok(listing)
}

/// The installed module called `name`.
pub fn find(install_dir: &Path, name: &str) -> Result<InstalledModule> {
    validate_module_name(name)?;
    let module_dir = install_dir.join(name);
    if !module_dir.is_dir() {
        bail!(
            "Module '{}' is not installed ({} not found).",
            name,
            module_dir.display()
        );
    }

    let candidates: Vec<(PathBuf, String)> = dist_info_dirs_for(install_dir, name)?
        .into_iter()
        .filter_map(|path| {
            let dir_name = path.file_name()?.to_str()?.to_owned();
            let (_, version) = parse_dist_info_dir_name(&dir_name)?;
            Some((path, version.to_owned()))
        })
        .collect();

    match newest(candidates) {
        Some((dist_info, _)) => read_installed(name, module_dir, dist_info),
        None => bail!(
            "'{}' has a directory in {} but no {} folder, so baler has no record of it. \
             It was not installed by baler.",
            name,
            install_dir.display(),
            dist_info_dir_name(name, "<version>")
        ),
    }
}

/// The candidate whose `baler.toml` was written last, with the higher
/// version breaking a tie.
fn newest(candidates: Vec<(PathBuf, String)>) -> Option<(PathBuf, String)> {
    let written = |dist_info: &Path| -> SystemTime {
        std::fs::metadata(dist_info.join("baler.toml"))
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH)
    };
    let version = |text: &str| semver::Version::parse(text).ok();

    candidates.into_iter().max_by(|(a_path, a_version), (b_path, b_version)| {
        written(a_path)
            .cmp(&written(b_path))
            .then_with(|| version(a_version).cmp(&version(b_version)))
    })
}

fn read_installed(name: &str, module_dir: PathBuf, dist_info: PathBuf) -> Result<InstalledModule> {
    let toml_path = dist_info.join("baler.toml");
    let text = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("Could not read {}", toml_path.display()))?;
    let toml: BalerToml = toml::from_str(&text)
        .with_context(|| format!("Could not parse {}", toml_path.display()))?;

    Ok(InstalledModule {
        name: name.to_owned(),
        version: toml.project.version.clone(),
        toml,
        module_dir,
        dist_info,
    })
}
