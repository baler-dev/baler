//! R packages that come from a GitHub repository.
//!
//! Everywhere else a package's source is the URL of a CRAN-like repository.
//! A GitHub package is written as a source id in the same place, so the
//! resolver, the lock and the installer carry it without a second field:
//!
//! ```text
//! git+https://github.com/eddelbuettel/bh                  declared, default branch
//! git+https://github.com/eddelbuettel/bh@master           declared, branch or tag
//! git+https://github.com/eddelbuettel/bh#5a1b9c...        declared, exact commit
//! git+https://github.com/eddelbuettel/bh@master#5a1b9c... locked: ref plus the commit
//! ```
//!
//! A declared id may name a ref or a commit, never needs either. Resolving
//! turns it into a locked id, which always carries the full commit. The
//! commit is what gets downloaded and installed, so a locked install gives
//! the same code every time, even when the branch has moved on.

use std::io::Read as _;
use std::path::Path;

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;

use crate::cran::packages::parse_dcf;
use crate::cran::universe::Index;

const PREFIX: &str = "git+";

/// True when `repo` is a GitHub source id instead of a repository URL.
pub fn is_git_id(repo: &str) -> bool {
    repo.starts_with(PREFIX)
}

/// Whether a lock's `locked` source is the one `declared` in baler.toml
/// names. For repositories that is plain equality. For GitHub packages the
/// declared id is compared without its commit, which only a lock has.
pub fn same_source(declared: &str, locked: &str) -> bool {
    match (is_git_id(declared), is_git_id(locked)) {
        (false, false) => declared == locked,
        (true, true) => match (GitId::parse(declared), GitId::parse(locked)) {
            (Ok(declared), Ok(locked)) => declared.same_source(&locked),
            _ => false,
        },
        _ => false,
    }
}

/// The id to install for a package whose lock entry is still valid: the
/// locked id, commit included. `None` when the package is not a GitHub one,
/// the lock names another source, or the lock has no commit to hold on to.
pub fn pinned_source(declared: &str, locked: &str) -> Option<String> {
    if !same_source(declared, locked) {
        return None;
    }
    let locked_id = GitId::parse(locked).ok()?;
    locked_id.sha.as_ref()?;
    Some(locked_id.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitId {
    pub url: String,
    /// A branch or a tag.
    pub git_ref: Option<String>,
    /// A commit, abbreviated when it was typed by hand in baler.toml.
    pub sha: Option<String>,
}

impl GitId {
    pub fn new(url: &str, git_ref: Option<&str>, sha: Option<&str>) -> Self {
        let url = url.trim().trim_end_matches('/');
        let url = url.strip_suffix(".git").unwrap_or(url);
        Self {
            url: url.to_owned(),
            git_ref: git_ref.map(str::to_owned),
            sha: sha.map(str::to_owned),
        }
    }

    pub fn parse(id: &str) -> Result<Self> {
        let Some(rest) = id.strip_prefix(PREFIX) else {
            bail!("'{id}' is not a GitHub package source");
        };
        let (rest, sha) = match rest.split_once('#') {
            Some((rest, sha)) => (rest, Some(sha)),
            None => (rest, None),
        };
        // The ref starts at the first '@' after the host, so a branch may
        // contain slashes (`feature/x`) but a URL never has to.
        let path_start = rest
            .find("://")
            .map(|at| at + 3)
            .with_context(|| format!("'{id}' has no URL in it"))?;
        let at = rest[path_start..].find('/').and_then(|slash| {
            let from = path_start + slash;
            rest[from..].find('@').map(|at| from + at)
        });
        let (url, git_ref) = match at {
            Some(at) => (&rest[..at], Some(&rest[at + 1..])),
            None => (rest, None),
        };
        if git_ref == Some("") || sha == Some("") {
            bail!("'{id}' has an empty ref or commit");
        }
        Ok(Self::new(url, git_ref, sha))
    }

    pub fn with_sha(&self, sha: &str) -> Self {
        Self { sha: Some(sha.to_owned()), ..self.clone() }
    }

    /// `(owner, repository)`. Only github.com is supported.
    pub fn owner_repo(&self) -> Result<(String, String)> {
        let rest = self
            .url
            .strip_prefix("https://github.com/")
            .or_else(|| self.url.strip_prefix("http://github.com/"))
            .with_context(|| {
                format!(
                    "R packages can come from github.com only, got '{}'. For another host, \
                     publish the package to a CRAN-like repository and use `repo`.",
                    self.url
                )
            })?;
        let mut parts = rest.split('/');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(owner), Some(repo), None) if !owner.is_empty() && !repo.is_empty() => {
                Ok((owner.to_owned(), repo.to_owned()))
            }
            _ => bail!("expected https://github.com/<owner>/<repo>, got '{}'", self.url),
        }
    }

    /// Whether `locked` is the same place as `self`, as declared in
    /// baler.toml: same repository, same ref, and a commit that agrees when
    /// baler.toml names one.
    pub fn same_source(&self, locked: &GitId) -> bool {
        if !self.url.eq_ignore_ascii_case(&locked.url) || self.git_ref != locked.git_ref {
            return false;
        }
        match (&self.sha, &locked.sha) {
            (Some(declared), Some(locked)) => {
                locked.starts_with(declared.as_str()) || declared.starts_with(locked.as_str())
            }
            (Some(_), None) => false,
            _ => true,
        }
    }

    fn sha_is_full(&self) -> bool {
        self.sha.as_ref().is_some_and(|sha| is_full_sha(sha))
    }
}

impl std::fmt::Display for GitId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{PREFIX}{}", self.url)?;
        if let Some(git_ref) = &self.git_ref {
            write!(f, "@{git_ref}")?;
        }
        if let Some(sha) = &self.sha {
            write!(f, "#{sha}")?;
        }
        Ok(())
    }
}

fn is_full_sha(text: &str) -> bool {
    text.len() == 40 && text.chars().all(|c| c.is_ascii_hexdigit())
}

// ---- DESCRIPTION ----

/// The one-package index a GitHub repository stands for, read from its
/// DESCRIPTION. The record names `id` as its source, so what gets locked is
/// exactly the commit that was read.
pub fn index_from_description(text: &str, id: &GitId) -> Result<Index> {
    let mut index = parse_dcf(text.as_bytes())?;
    if index.len() != 1 {
        bail!("{id} has no readable DESCRIPTION with a Package and a Version field");
    }
    for record in index.values_mut() {
        record.source = Some(id.to_string());
    }
    Ok(index)
}

/// `description` with the `Remote*` fields R tooling (and baler) read back
/// to learn where a package was installed from. Existing ones are replaced.
pub fn with_remote_fields(description: &str, id: &GitId) -> Result<String> {
    let Some(sha) = &id.sha else {
        bail!("{id} has no commit, so there is nothing to record");
    };
    let (owner, repo) = id.owner_repo()?;
    let mut kept: Vec<&str> = description
        .lines()
        .filter(|line| !line.starts_with("Remote"))
        .collect();
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    let mut text = kept.join("\n");
    text.push('\n');
    text.push_str("RemoteType: github\n");
    text.push_str("RemoteHost: api.github.com\n");
    text.push_str(&format!("RemoteRepo: {repo}\n"));
    text.push_str(&format!("RemoteUsername: {owner}\n"));
    text.push_str(&format!("RemoteRef: {}\n", id.git_ref.as_deref().unwrap_or("HEAD")));
    text.push_str(&format!("RemoteSha: {sha}\n"));
    Ok(text)
}

/// The commit an installed package records in its DESCRIPTION, if any.
pub fn remote_sha(description: &str) -> Option<String> {
    description
        .lines()
        .find_map(|line| line.strip_prefix("RemoteSha:"))
        .map(|sha| sha.trim().to_owned())
        .filter(|sha| !sha.is_empty())
}

/// Whether the package installed at `desc_path` came from where `repo` says:
/// the same commit for a GitHub source, and not from GitHub at all for a
/// repository.
pub fn installed_origin_matches(desc_path: &Path, repo: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(desc_path) else {
        return false;
    };
    let installed = remote_sha(&text);
    if !is_git_id(repo) {
        return installed.is_none();
    }
    let wanted = GitId::parse(repo).ok().and_then(|id| id.sha);
    match (installed, wanted) {
        (Some(installed), Some(wanted)) => installed.starts_with(&wanted),
        _ => false,
    }
}

// ---- network ----

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent("baler")
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .context("Failed to build HTTP client")
}

/// A token raises GitHub's API limit from 60 requests an hour and opens
/// private repositories.
fn token() -> Option<String> {
    ["GITHUB_TOKEN", "GH_TOKEN"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .filter(|token| !token.is_empty())
}

fn get(url: &str, accept: Option<&str>) -> Result<reqwest::blocking::Response> {
    let mut request = client()?.get(url);
    if let Some(accept) = accept {
        request = request.header("Accept", accept);
    }
    if let Some(token) = token() {
        request = request.bearer_auth(token);
    }
    let response = request.send().with_context(|| format!("Failed to reach {url}"))?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    match status.as_u16() {
        404 => bail!(
            "{url} was not found. Check the repository and the ref, and set GITHUB_TOKEN if \
             the repository is private."
        ),
        403 | 429 => bail!(
            "GitHub refused {url} (HTTP {status}), most likely its rate limit. Set \
             GITHUB_TOKEN to raise it."
        ),
        _ => bail!("HTTP {status} from {url}"),
    }
}

/// The full commit a ref, an abbreviated commit, or the default branch
/// points at right now.
fn resolve_sha(id: &GitId) -> Result<String> {
    let (owner, repo) = id.owner_repo()?;
    let what = id.sha.as_deref().or(id.git_ref.as_deref()).unwrap_or("HEAD");
    let url = format!("https://api.github.com/repos/{owner}/{repo}/commits/{what}");
    let sha = get(&url, Some("application/vnd.github.sha"))?
        .text()
        .with_context(|| format!("Failed to read the answer of {url}"))?;
    let sha = sha.trim().to_owned();
    if !is_full_sha(&sha) {
        bail!("GitHub did not answer {url} with a commit");
    }
    Ok(sha)
}

fn fetch_description(id: &GitId) -> Result<String> {
    let (owner, repo) = id.owner_repo()?;
    let sha = id.sha.as_deref().context("the commit is not known yet")?;
    let url = format!("https://raw.githubusercontent.com/{owner}/{repo}/{sha}/DESCRIPTION");
    get(&url, None)
        .with_context(|| {
            format!(
                "Could not read DESCRIPTION at the top of {} ({sha}). An R package has to \
                 be at the root of its repository, packages in a subdirectory are not \
                 supported yet.",
                id.url
            )
        })?
        .text()
        .with_context(|| format!("Failed to read the body of {url}"))
}

/// The index of a GitHub source: its one package, at the commit it resolves
/// to now, or at the commit the id already names.
pub fn fetch_index(source: &str) -> Result<Index> {
    let id = GitId::parse(source)?;
    id.owner_repo()?;
    let id = if id.sha_is_full() {
        id
    } else {
        let sha = resolve_sha(&id)?;
        id.with_sha(&sha)
    };
    let text = fetch_description(&id)?;
    index_from_description(&text, &id)
}

/// Download `source` at its commit, record where it came from in its
/// DESCRIPTION, and `R CMD INSTALL` it into `lib_path`.
pub fn install(pkg: &str, source: &str, lib_path: &Path) -> Result<()> {
    let id = GitId::parse(source)?;
    if !id.sha_is_full() {
        bail!("{source} has no full commit, resolve it before installing");
    }
    let sha = id.sha.clone().unwrap_or_default();
    let (owner, repo) = id.owner_repo()?;

    let url = format!("https://api.github.com/repos/{owner}/{repo}/tarball/{sha}");
    let bytes = get(&url, None)?
        .bytes()
        .with_context(|| format!("Failed to download {url}"))?;

    let work = tempfile::TempDir::new().context("Failed to create a temp dir")?;
    tar::Archive::new(GzDecoder::new(bytes.as_ref()))
        .unpack(work.path())
        .with_context(|| format!("Failed to unpack {url}"))?;
    let root = single_subdir(work.path())?;

    let desc_path = root.join("DESCRIPTION");
    let mut description = String::new();
    std::fs::File::open(&desc_path)
        .with_context(|| format!("{source} has no DESCRIPTION at its root"))?
        .read_to_string(&mut description)
        .context("DESCRIPTION is not valid UTF-8")?;
    let named = parse_dcf(description.as_bytes())?;
    if !named.contains_key(pkg) {
        bail!("{source} is not an R package named '{pkg}'");
    }
    std::fs::write(&desc_path, with_remote_fields(&description, &id)?)
        .context("Failed to record where the package came from")?;

    std::fs::create_dir_all(lib_path)
        .with_context(|| format!("Failed to create lib dir: {}", lib_path.display()))?;
    let lib_arg = format!(
        "--library={}",
        lib_path.to_str().context("lib_path contains invalid UTF-8")?
    );
    let status = std::process::Command::new("R")
        .args(["CMD", "INSTALL", "--no-multiarch", "--no-docs", "--no-help", &lib_arg])
        .arg(&root)
        .status()
        .with_context(|| format!("Failed to run R CMD INSTALL for {pkg}, is R on PATH?"))?;
    if !status.success() {
        bail!("R CMD INSTALL failed for {pkg} from {source} (exit: {status})");
    }
    Ok(())
}

/// GitHub tarballs hold one directory, `<owner>-<repo>-<short commit>`.
fn single_subdir(dir: &Path) -> Result<std::path::PathBuf> {
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(dir).context("Failed to read the unpacked tarball")? {
        let path = entry?.path();
        if path.is_dir() {
            dirs.push(path);
        }
    }
    match dirs.len() {
        1 => Ok(dirs.remove(0)),
        count => bail!("expected one directory in the tarball, found {count}"),
    }
}
