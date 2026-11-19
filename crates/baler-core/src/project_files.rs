//! The files a module ships next to its source: a license and a README.
//!
//! They live at the project root, beside `baler.toml`. `baler bundle`
//! puts them at the archive root, and `baler install` keeps them in the
//! module's `.dist-info` folder, where `baler show` finds them again.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

/// Names that count as a license file, in lower case. Compared without
/// regard to case, so `License.md` is found on a case-sensitive
/// filesystem as well.
const LICENSE_NAMES: [&str; 9] = [
    "license",
    "license.md",
    "license.txt",
    "licence",
    "licence.md",
    "licence.txt",
    "copying",
    "copying.md",
    "copying.txt",
];

/// True for a file name that holds a license: `LICENSE`, `LICENSE.md`,
/// `LICENSE.txt`, their `LICENCE` spelling, and `COPYING` with the same
/// endings.
pub fn is_license_file_name(name: &str) -> bool {
    LICENSE_NAMES.contains(&name.to_ascii_lowercase().as_str())
}

/// True for a file name that holds a README: `README` with any ending,
/// in any case.
pub fn is_readme_file_name(name: &str) -> bool {
    Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case("readme"))
}

/// What was found at a project root.
#[derive(Debug, Default)]
pub struct ProjectFiles {
    /// License files, sorted by name.
    pub licenses: Vec<PathBuf>,
    /// The README, from `readme` in `baler.toml` or else found by name.
    pub readme: Option<PathBuf>,
    /// Why a declared README could not be used.
    pub problems: Vec<String>,
}

/// The `readme` that `baler.toml` declares, or `None`. It reads that one
/// key and nothing else, so it works on a file that is not yet valid.
pub fn declared_readme(project_root: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct Peek {
        project: PeekProject,
    }
    #[derive(Deserialize)]
    struct PeekProject {
        readme: Option<String>,
    }

    let text = std::fs::read_to_string(project_root.join("baler.toml")).ok()?;
    let peek: Peek = toml::from_str(&text).ok()?;
    peek.project.readme
}

/// Looks for the license files and the README in `project_root`.
///
/// With `declared_readme` set, that path is the README, and it has to be
/// a file inside the project: an absolute path or one with `..` is
/// refused, since a bundle must never read outside the project. Without
/// it, a file called `README` (with any ending) is used, preferring
/// `.md`, then `.txt`, then no ending.
pub fn find_project_files(project_root: &Path, declared_readme: Option<&str>) -> ProjectFiles {
    let mut found = ProjectFiles::default();

    let mut root_files: Vec<(String, PathBuf)> = match std::fs::read_dir(project_root) {
        Ok(entries) => entries
            .flatten()
            .filter(|entry| entry.path().is_file())
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_owned();
                Some((name, entry.path()))
            })
            .collect(),
        Err(_) => return found,
    };
    root_files.sort();

    found.licenses = root_files
        .iter()
        .filter(|(name, _)| is_license_file_name(name))
        .map(|(_, path)| path.clone())
        .collect();

    match declared_readme {
        Some(declared) => match inside_project(project_root, declared) {
            Some(path) if path.is_file() => found.readme = Some(path),
            Some(_) => found.problems.push(format!(
                "`readme = \"{declared}\"` in baler.toml does not point at a file."
            )),
            None => found.problems.push(format!(
                "`readme = \"{declared}\"` in baler.toml must be a path inside the project, \
                 without `..`."
            )),
        },
        None => {
            let rank = |name: &str| match Path::new(name)
                .extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("md") => 0,
                Some("txt") => 1,
                None => 2,
                Some(_) => 3,
            };
            found.readme = root_files
                .iter()
                .filter(|(name, _)| is_readme_file_name(name))
                .min_by_key(|(name, _)| (rank(name), name.clone()))
                .map(|(_, path)| path.clone());
        }
    }

    found
}

/// `relative` joined onto `project_root`, or `None` when it is absolute
/// or climbs out with `..`.
fn inside_project(project_root: &Path, relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative);
    let plain = relative
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
    if relative.as_os_str().is_empty() || !plain {
        return None;
    }
    Some(project_root.join(relative))
}

/// The name a README is stored under in a bundle and in `.dist-info`:
/// `README` plus the ending of the original, so `docs/intro.md` becomes
/// `README.md`. A fixed stem is what lets install find it again,
/// wherever the author kept it.
pub fn stored_readme_name(source: &Path) -> String {
    match source.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => format!("README.{ext}"),
        None => "README".to_owned(),
    }
}
