mod archive;
mod github;
mod native;

use anyhow::{bail, Result};
use std::path::PathBuf;

pub use github::GitHubFetcher;

use archive::{install_from_dir, install_from_registry, install_from_tar};
use github::install_from_github;

/// Everything `install` needs. This mirrors CLI mutual-exclusivity, but
/// re-checked here since a library caller can build this without clap.
pub struct InstallRequest {
    /// A bare registry name, a local path (any of `./x`, `../x`, an
    /// absolute path, a separator, or a `.tar.gz`), or `gh:user/repo`.
    /// Judged by appearance only. `--path`/`--git` are explicit alternatives.
    pub source: Option<String>,
    /// Registry URL for a bare-name lookup (not implemented yet).
    pub repo: Option<String>,
    /// Version constraint for a bare-name lookup.
    pub version: Option<String>,
    /// Local directory or .tar.gz. This inspired by `cargo install --path`.
    pub path: Option<String>,
    /// A GitHub repo URL. Fetches via GitHub's tarball API only, not
    /// generic git. A GitLab/self-hosted URL won't work.
    pub git: Option<String>,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub rev: Option<String>,
    /// A direct tarball URL, e.g. a GitHub Release asset.
    pub url: Option<String>,
    /// Subdir within a `--git` repo holding the module, for repos with
    /// more than one. No equivalent for `--url`, which names one artifact.
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

/// pip's `_looks_like_path` heuristic: appearance only, never the
/// filesystem. Anything else is reserved for a future registry lookup.
fn looks_like_local_path(s: &str) -> bool {
    std::path::Path::new(s).is_absolute()
        || s.starts_with("./") || s.starts_with(".\\")
        || s.starts_with("../") || s.starts_with("..\\")
        || s.contains('/') || s.contains('\\')
        || s.ends_with(".tar.gz")
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

    // `gh:user/repo` shorthand for --git; checked before the path sniff
    // below since it also contains a `/`.
    if let Some(rest) = s.strip_prefix("gh:") {
        if req.repo.is_some() {
            bail!("--repo doesn't apply to gh: sources, it's already a direct GitHub reference.");
        }
        let ref_count = [req.branch.is_some(), req.tag.is_some(), req.rev.is_some()]
            .into_iter()
            .filter(|set| *set)
            .count();
        if ref_count > 1 {
            bail!("--branch, --tag, and --rev are mutually exclusive, pick one ref.");
        }
        let (user, repo) = rest
            .split_once('/')
            .filter(|(u, r)| !u.is_empty() && !r.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Expected gh:username/repo, got '{}'.", s))?;
        let url = format!("https://github.com/{user}/{repo}");
        let git_ref = req.branch.or(req.tag).or(req.rev);
        return Ok(InstallSource::GitHub { url, module_dir: req.module_dir, git_ref });
    }

    // Local path, judged by appearance only (see looks_like_local_path).
    if looks_like_local_path(&s) {
        if req.repo.is_some() {
            bail!("--repo doesn't apply to local paths.");
        }
        if req.branch.is_some() || req.tag.is_some() || req.rev.is_some() || req.module_dir.is_some() {
            bail!("--branch/--tag/--rev/--module-dir only apply to --git, not a local path.");
        }
        let path = PathBuf::from(&s);
        if path.is_dir() {
            return Ok(InstallSource::Dir(path));
        }
        return match path.extension().and_then(|e| e.to_str()) {
            Some("gz") if s.ends_with(".tar.gz") => Ok(InstallSource::Tar(path)),
            _ => bail!("Expected a directory, .tar.gz, or gh:username/repo, got '{}'.", s),
        };
    }

    if req.branch.is_some() || req.tag.is_some() || req.rev.is_some() || req.module_dir.is_some() {
        bail!("--branch/--tag/--rev/--module-dir only apply to --git, not a registry name.");
    }

    // Bare name: --repo turns it into a registry lookup; without it,
    // the name is just reserved, not resolvable to anything yet.
    match req.repo {
        Some(repo_url) => Ok(InstallSource::Registry { name: s, repo: repo_url, version: req.version }),
        None => bail!(
            "'{}' looks like a module name. Use --repo <url> for a registry, \
             --path for a local module, --git <url>, or --url <tarball> instead.",
            s
        ),
    }
}
