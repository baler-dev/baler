mod archive;
mod github;
mod native;

use anyhow::{bail, Result};
use std::path::PathBuf;

pub use github::GitHubFetcher;

use archive::{install_from_dir, install_from_registry, install_from_tar};
use github::install_from_github;

/// Everything `install` needs, already checked for mutual exclusivity
/// at the CLI layer (see baler-cli's clap `conflicts_with`/`requires`
/// wiring). This module re-checks the combinations that matter for
/// correctness anyway, not just argument wiring, since a library
/// caller (or a test) can build this directly without going through
/// clap at all.
pub struct InstallRequest {
    /// A bare module name for a registry lookup — nothing else.
    /// Mirrors `cargo install <crate>` / `pip install <pkg>` exactly:
    /// the positional never means a local path here, `--path` does.
    pub source: Option<String>,
    /// Registry URL for a bare-name lookup (registries aren't
    /// implemented yet). Only meaningful with `source`.
    pub repo: Option<String>,
    /// Version constraint for a bare-name lookup. Only meaningful
    /// with `source`.
    pub version: Option<String>,
    /// Local directory or .tar.gz. Mirrors `cargo install --path`.
    pub path: Option<String>,
    /// A GitHub repository URL, e.g. `https://github.com/user/repo`.
    /// Named `--git` to match Cargo's flag, but the fetch underneath
    /// (see github.rs) only speaks GitHub's tarball API, not generic
    /// git — a GitLab or self-hosted URL will fail here today.
    pub git: Option<String>,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub rev: Option<String>,
    /// A direct tarball URL, e.g. a GitHub Release asset.
    pub url: Option<String>,
    /// Directory within a `--git` repo that holds the module, for a
    /// repo that holds more than one (mirrors rcalc's own src/
    /// layout). Only meaningful with `git` — `--git`'s URL names a
    /// whole repository, not one file, so it needs this to know where
    /// inside that repo to look. `--url` names one exact artifact
    /// directly, there's nothing to navigate, so it has no equivalent.
    pub module_dir: Option<String>,
}

enum InstallSource {
    Tar(PathBuf),
    Dir(PathBuf),
    GitHub { url: String, module_dir: Option<String>, git_ref: Option<String> },
    Url(String),
    Registry { name: String, repo: String, version: Option<String> },
}

pub fn run(req: InstallRequest, install_deps: bool) -> Result<()> {
    match resolve_source(req)? {
        InstallSource::Tar(path) => install_from_tar(&path, install_deps, None),
        InstallSource::Dir(path) => install_from_dir(&path, install_deps),
        InstallSource::GitHub { url, module_dir, git_ref } => {
            install_from_github(&url, module_dir.as_deref(), git_ref.as_deref(), install_deps)
        }
        InstallSource::Url(url) => {
            github::download_and_install(&url, install_deps)
        }
        InstallSource::Registry { name, repo, version } => {
            install_from_registry(&name, &repo, version.as_deref(), install_deps)
        }
    }
}

fn resolve_source(req: InstallRequest) -> Result<InstallSource> {
    let exclusive_count = [req.source.is_some(), req.path.is_some(), req.git.is_some(), req.url.is_some()]
        .into_iter()
        .filter(|set| *set)
        .count();

    if exclusive_count == 0 {
        bail!("Nothing to install. Pass a module name, --path <dir-or-tar.gz>, --git <url>, or --url <tarball>.");
    }
    if exclusive_count > 1 {
        bail!("A module source can only come from one place: a bare name, --path, --git, or --url, not more than one.");
    }

    if let Some(url) = req.url {
        if req.repo.is_some() || req.version.is_some() || req.branch.is_some() || req.tag.is_some()
            || req.rev.is_some() || req.module_dir.is_some()
        {
            bail!("--url fetches an already-bundled archive directly, --repo/--version/--branch/--tag/--rev/--module-dir don't apply to it.");
        }
        return Ok(InstallSource::Url(url));
    }

    if let Some(url) = req.git {
        if req.repo.is_some() || req.version.is_some() {
            bail!("--repo/--version apply to a registry lookup, not --git.");
        }
        let ref_count = [req.branch.is_some(), req.tag.is_some(), req.rev.is_some()]
            .into_iter()
            .filter(|set| *set)
            .count();
        if ref_count > 1 {
            bail!("--branch, --tag, and --rev are mutually exclusive, pick one ref.");
        }
        let git_ref = req.branch.or(req.tag).or(req.rev);
        return Ok(InstallSource::GitHub { url, module_dir: req.module_dir, git_ref });
    }

    if let Some(p) = req.path {
        if req.repo.is_some() || req.version.is_some() || req.branch.is_some() || req.tag.is_some()
            || req.rev.is_some() || req.module_dir.is_some()
        {
            bail!("--repo/--version/--branch/--tag/--rev/--module-dir don't apply to --path.");
        }
        let path = PathBuf::from(&p);
        if path.is_dir() {
            return Ok(InstallSource::Dir(path));
        }
        return match path.extension().and_then(|e| e.to_str()) {
            Some("gz") => Ok(InstallSource::Tar(path)),
            _ => bail!("--path expects a directory or a .tar.gz, got '{}'.", p),
        };
    }

    let s = req.source.expect("checked above: exactly one of source/path/git/url is set");
    if req.branch.is_some() || req.tag.is_some() || req.rev.is_some() || req.module_dir.is_some() {
        bail!("--branch/--tag/--rev/--module-dir only apply to --git, not a registry name.");
    }

    // Bare name: --repo is what turns this into a registry lookup.
    // Without it, the name is still reserved, just not resolvable to
    // anything yet, same as `cargo install <crate>` needing a
    // registry (crates.io, by default) to actually mean something.
    match req.repo {
        Some(repo_url) => Ok(InstallSource::Registry { name: s, repo: repo_url, version: req.version }),
        None => bail!(
            "'{}' looks like a module name. Use --repo <url> for a registry, \
             --path for a local module, --git <url>, or --url <tarball> instead.",
            s
        ),
    }
}
