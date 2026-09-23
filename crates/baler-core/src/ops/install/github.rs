use anyhow::{bail, Context, Result};
use std::fs::File;
use std::path::PathBuf;
use tempfile::TempDir;

use ::tar::Archive as TarArchive;
use crate::baler_toml::BalerToml;
use crate::lockfile;
use crate::ops::module_graph::ModuleFetcher;

use super::archive::install_from_tar;

/// Parses a bare GitHub repo URL (`https://github.com/user/repo`,
/// trailing slash or `.git` suffix tolerated) into (user, repo).
fn parse_github_repo_url(url: &str) -> Result<(String, String)> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .ok_or_else(|| anyhow::anyhow!("--git expects a github.com URL, got '{url}'"))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = rest.splitn(2, '/');
    let user = parts.next().filter(|s| !s.is_empty());
    let repo = parts.next().filter(|s| !s.is_empty());
    match (user, repo) {
        (Some(u), Some(r)) => Ok((u.to_owned(), r.to_owned())),
        _ => bail!("--git expects https://github.com/user/repo, got '{url}'"),
    }
}

#[cfg(feature = "network")]
pub(super) fn install_from_github(url: &str, module_dir: Option<&str>, git_ref: Option<&str>, install_deps: bool) -> Result<()> {
    let (user, repo) = parse_github_repo_url(url)?;
    let tarball_url = match git_ref {
        Some(r) => format!("https://api.github.com/repos/{}/{}/tarball/{}", user, repo, r),
        None => format!("https://api.github.com/repos/{}/{}/tarball", user, repo),
    };
    match git_ref {
        Some(r) => println!("Fetching {}/{}@{}...", user, repo, r),
        None => println!("Fetching {}/{} (default branch)...", user, repo),
    }
    fetch_and_install(&tarball_url, module_dir, install_deps)
}

#[cfg(not(feature = "network"))]
pub(super) fn install_from_github(_url: &str, _module_dir: Option<&str>, _git_ref: Option<&str>, _install_deps: bool) -> Result<()> {
    bail!(
        "GitHub install requires the 'network' feature.\n\
         Rebuild with: cargo build --features network"
    )
}

/// `--url`'s handler: it points at an already-bundled baler archive
/// (a GitHub Release asset produced by `baler bundle`, say), not a
/// raw source tree, unlike --git there's no repo to extract, navigate,
/// or re-bundle, so no --module-dir concept applies here either. Just
/// download and install directly, the same path a local
/// `--path foo.tar.gz` already takes.
///
/// Only `.tar.gz` is supported, the one format `baler bundle`
/// produces. Checked up front, same as `--path` already does, so a
/// `.zip`, an HTML redirect page, or anything else fails with a clear
/// message here instead of a raw gzip-decoding error surfacing later
/// deep inside `tar::read_toml`.
#[cfg(feature = "network")]
pub(super) fn download_and_install(url: &str, install_deps: bool) -> Result<()> {
    if !url.ends_with(".tar.gz") {
        bail!(
            "--url expects a .tar.gz archive, the format `baler bundle` produces, got '{url}'."
        );
    }

    let tmp = TempDir::new().context("Failed to create temp directory")?;
    let tarball_path = tmp.path().join("module.tar.gz");
    download_file(url, &tarball_path)
        .with_context(|| format!("Failed to download {url}"))?;
    install_from_tar(&tarball_path, install_deps, None)
}

#[cfg(not(feature = "network"))]
pub(super) fn download_and_install(_url: &str, _install_deps: bool) -> Result<()> {
    bail!(
        "Installing from a URL requires the 'network' feature.\n\
         Rebuild with: cargo build --features network"
    )
}

#[cfg(feature = "network")]
fn fetch_and_install(url: &str, module_dir: Option<&str>, install_deps: bool) -> Result<()> {
    let tmp = TempDir::new().context("Failed to create temp directory")?;
    let tarball_path = tmp.path().join("source.tar.gz");

    download_file(url, &tarball_path)
        .with_context(|| format!("Failed to download {url}"))?;

    let extract_dir = tmp.path().join("extracted");
    std::fs::create_dir_all(&extract_dir)
        .context("Failed to create extraction directory")?;

    extract_tarball(&tarball_path, &extract_dir)
        .context("Failed to extract tarball")?;

    let extracted_root = find_single_subdir(&extract_dir)
        .context("Could not find module directory in downloaded archive")?;

    let project_root = match module_dir {
        Some(dir) => extracted_root.join(dir),
        None => extracted_root,
    };

    if !project_root.exists() {
        match module_dir {
            Some(dir) => bail!("Directory '{}' not found in the downloaded archive", dir),
            None => bail!("Extracted archive root does not exist"),
        }
    }

    if !project_root.join("baler.toml").exists() {
        bail!(
            "No baler.toml found at {}. This source is not a baler module.",
            project_root.display()
        );
    }

    let lock = lockfile::read(&project_root).with_context(|| {
        format!("Failed to read baler.lock in {}", project_root.display())
    })?;

    let output_path = tmp.path().join("module.tar.gz");
    crate::ops::bundle::bundle_to(&project_root, &output_path)
        .context("Failed to bundle downloaded module")?;

    install_from_tar(&output_path, install_deps, lock.as_ref())
}

/// A parsed `[project.dependencies.baler]` entry's `source`. Unlike
/// `--git`/`--module-dir` at the CLI, this still parses an
/// embedded `/tree/<ref>/<subpath>` out of one URL string, since
/// that's the shape `baler.toml`'s own `source` field uses. A TOML
/// string field is naturally single-valued; the CLI has room for a
/// separate flag a config file doesn't. Two different conventions for
/// two different contexts, not an inconsistency introduced by
/// accident.
fn parse_module_dep_source(source: &str) -> Result<(String, Option<String>, Option<String>)> {
    let rest = source
        .strip_prefix("https://github.com/")
        .or_else(|| source.strip_prefix("http://github.com/"))
        .ok_or_else(|| anyhow::anyhow!("Unsupported module source '{source}', only github.com URLs can be fetched right now."))?;

    let mut parts = rest.splitn(2, '/');
    let user = parts.next().filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Invalid GitHub source '{source}'"))?;
    let remainder = parts.next().unwrap_or("");
    let mut repo_and_rest = remainder.splitn(2, '/');
    let repo = repo_and_rest.next().filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Invalid GitHub source '{source}'"))?;
    let after_repo = repo_and_rest.next();

    let (git_ref, subpath) = match after_repo {
        Some(after) => match after.strip_prefix("tree/") {
            Some(tree_rest) => {
                let mut segments = tree_rest.splitn(2, '/');
                let git_ref = segments.next().filter(|s| !s.is_empty()).map(str::to_owned);
                let subpath = segments.next().filter(|s| !s.is_empty()).map(str::to_owned);
                (git_ref, subpath)
            }
            None => (None, if after.is_empty() { None } else { Some(after.to_owned()) }),
        },
        None => (None, None),
    };

    let repo_url = format!("https://github.com/{user}/{repo}");
    Ok((repo_url, subpath, git_ref))
}

/// The real ModuleFetcher used by resolve_transitive() outside of
/// tests, for `[project.dependencies.baler]` entries whose `source`
/// is a GitHub URL. Only understands github.com URLs today.
#[cfg(feature = "network")]
pub struct GitHubFetcher;

#[cfg(feature = "network")]
impl ModuleFetcher for GitHubFetcher {
    fn fetch(&self, source: &str) -> Result<BalerToml> {
        let (repo_url, subdir, git_ref) = parse_module_dep_source(source)?;
        let (user, repo) = parse_github_repo_url(&repo_url)?;

        let tarball_url = match &git_ref {
            Some(r) => format!("https://api.github.com/repos/{}/{}/tarball/{}", user, repo, r),
            None => format!("https://api.github.com/repos/{}/{}/tarball", user, repo),
        };

        let tmp = TempDir::new().context("Failed to create temp directory")?;
        let tarball_path = tmp.path().join("repo.tar.gz");
        download_file(&tarball_url, &tarball_path)
            .with_context(|| format!("Failed to download module source '{source}'"))?;

        let extract_dir = tmp.path().join("extracted");
        std::fs::create_dir_all(&extract_dir)
            .context("Failed to create extraction directory")?;
        extract_tarball(&tarball_path, &extract_dir)
            .context("Failed to extract tarball")?;

        let extracted_root = find_single_subdir(&extract_dir)
            .context("Could not find module directory in downloaded archive")?;

        let project_root = match &subdir {
            Some(sub) => extracted_root.join(sub),
            None => extracted_root,
        };

        BalerToml::from_dir(&project_root)
            .with_context(|| format!("Module source '{source}' does not contain a valid baler.toml"))
    }
}

#[cfg(not(feature = "network"))]
pub struct GitHubFetcher;

#[cfg(not(feature = "network"))]
impl ModuleFetcher for GitHubFetcher {
    fn fetch(&self, _source: &str) -> Result<BalerToml> {
        bail!(
            "GitHub module fetching requires the 'network' feature.\n\
             Rebuild with: cargo build --features network"
        )
    }
}

#[cfg(feature = "network")]
fn download_file(url: &str, dest: &PathBuf) -> Result<()> {
    let response = reqwest::blocking::Client::new()
        .get(url)
        .header("User-Agent", "baler")
        .send()
        .with_context(|| format!("HTTP request failed: {url}"))?;

    if !response.status().is_success() {
        bail!("HTTP {} from {}", response.status(), url);
    }

    let bytes = response.bytes().context("Failed to read response bytes")?;
    std::fs::write(dest, &bytes)
        .with_context(|| format!("Failed to write to {}", dest.display()))?;

    Ok(())
}

fn extract_tarball(tarball_path: &PathBuf, dest: &PathBuf) -> Result<()> {
    let file = File::open(tarball_path)
        .with_context(|| format!("Failed to open: {}", tarball_path.display()))?;

    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = TarArchive::new(gz);

    archive
        .unpack(dest)
        .with_context(|| format!("Failed to unpack to {}", dest.display()))?;

    Ok(())
}

fn find_single_subdir(dir: &PathBuf) -> Result<PathBuf> {
    let entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("Failed to read: {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();

    match entries.len() {
        0 => bail!("Extracted archive is empty"),
        1 => Ok(entries.into_iter().next().unwrap().path()),
        _ => bail!("Expected one top-level directory in archive, found multiple"),
    }
}
