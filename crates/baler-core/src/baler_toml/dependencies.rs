use anyhow::{bail, Result};

use crate::cran::client::git::GitId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DEFAULT_CRAN_MIRROR: &str = "https://cloud.r-project.org";

// ---- PackageDep ----

/// An R package entry. A bare string is a version constraint, looked up on
/// CRAN. A table with `version` and `repo` names another CRAN-like
/// repository. A table with `git` names a GitHub repository, see
/// `PackageGitSpec`.
///
/// Untagged, so the variants are tried in order, and `Git` has to stay
/// ahead of `Extended`. `Extended` ignores keys it does not know, so
/// `{ version = ">=1.0", git = "..." }` would match it and the `git` key
/// would be dropped without a word.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum PackageDep {
    Simple(String),
    Git(PackageGitSpec),
    Extended { version: String, repo: Option<String> },
}

/// The table form of an R package that comes from GitHub, written with the
/// same keys a module dependency takes. Unknown keys are rejected rather
/// than silently dropped, so a misspelled `tag` fails loudly. Which keys
/// may combine is checked by `PackageGitSpec::check`.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PackageGitSpec {
    /// A github.com repository URL.
    pub git: String,
    /// Optional, defaults to `"*"`. Meant to be checked against the
    /// package's own DESCRIPTION.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// Never valid next to `git`. It is accepted here only so that
    /// `check` can say a package comes from one place, instead of the
    /// entry failing to match any shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

impl PackageGitSpec {
    /// The `branch`, `tag` or `rev`, whichever is set.
    pub fn git_ref(&self) -> Option<&str> {
        self.branch
            .as_deref()
            .or(self.tag.as_deref())
            .or(self.rev.as_deref())
    }

    /// The source id this entry stands for in the resolver and in
    /// `baler.lock`, see `cran::client::git`. A `rev` is a commit, a branch
    /// or tag is a ref.
    pub fn id(&self) -> String {
        let (git_ref, sha) = match (&self.rev, self.git_ref()) {
            (Some(rev), _) => (None, Some(rev.as_str())),
            (None, git_ref) => (git_ref, None),
        };
        GitId::new(&self.git, git_ref, sha).to_string()
    }

    /// An error means the keys contradict each other.
    pub fn check(&self) -> Result<()> {
        if self.repo.is_some() {
            bail!("`git` and `repo` are mutually exclusive, a package comes from one place.");
        }
        let ref_count = [&self.branch, &self.tag, &self.rev]
            .iter()
            .filter(|r| r.is_some())
            .count();
        if ref_count > 1 {
            bail!("`branch`, `tag`, and `rev` are mutually exclusive, pick one ref.");
        }
        Ok(())
    }
}

impl PackageDep {
    pub fn version(&self) -> &str {
        match self {
            PackageDep::Simple(v) => v,
            PackageDep::Git(spec) => spec.version.as_deref().unwrap_or("*"),
            PackageDep::Extended { version, .. } => version,
        }
    }

    /// The GitHub source this entry names, when it names one.
    pub fn git(&self) -> Option<&PackageGitSpec> {
        match self {
            PackageDep::Git(spec) => Some(spec),
            _ => None,
        }
    }

    /// Fails on an entry whose keys contradict each other.
    pub fn check(&self) -> Result<()> {
        match self {
            PackageDep::Git(spec) => spec.check(),
            _ => Ok(()),
        }
    }

    /// The repository this entry names, or `None` when it names none and the
    /// default CRAN is meant. Dependents that do not care where a package
    /// comes from must not clash with one that does. A GitHub package names
    /// its repository URL.
    pub fn explicit_repo(&self) -> Option<&str> {
        match self {
            PackageDep::Simple(_) => None,
            PackageDep::Git(spec) => Some(&spec.git),
            PackageDep::Extended { repo, .. } => repo.as_deref(),
        }
    }

    /// What the resolver and the lock call this entry's source: the
    /// repository URL, or for a GitHub package its source id.
    pub fn repo_id(&self) -> String {
        match self {
            PackageDep::Git(spec) => spec.id(),
            other => other.repo().to_owned(),
        }
    }

    pub fn repo(&self) -> &str {
        match self {
            PackageDep::Simple(_) => DEFAULT_CRAN_MIRROR,
            PackageDep::Git(spec) => &spec.git,
            PackageDep::Extended { repo, .. } => {
                repo.as_deref().unwrap_or(DEFAULT_CRAN_MIRROR)
            }
        }
    }
}

// ---- ModuleDep ----

/// A `[dependencies.baler]` entry. A bare string is only a version
/// constraint, which would mean a registry lookup (not implemented
/// yet). The table form names where the module comes from, using the
/// same keys `baler install` takes as flags.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum ModuleDep {
    Simple(String),
    Extended(ModuleDepSpec),
}

/// The table form of a module dependency. Unknown keys are rejected
/// rather than silently dropped, so a misspelled `tag` fails loudly.
/// Which keys may combine is checked by `ModuleDep::source()`.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ModuleDepSpec {
    /// Optional, defaults to `"*"`. Still checked against the fetched
    /// module's own `baler.toml` version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// A github.com repository URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// Directory inside the `git` repo that holds the module.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_dir: Option<String>,
    /// A direct link to an already-bundled `.tar.gz`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Where a module dependency is fetched from, after `ModuleDepSpec`'s
/// keys have been checked against each other. `branch`, `tag`, and
/// `rev` all end up as one `git_ref`, since a GitHub tarball request
/// treats them the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleSource {
    Git { url: String, git_ref: Option<String>, module_dir: Option<String> },
    Url(String),
}

impl std::fmt::Display for ModuleSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModuleSource::Git { url, git_ref, module_dir } => {
                write!(f, "{url}")?;
                if let Some(r) = git_ref { write!(f, "@{r}")?; }
                if let Some(d) = module_dir { write!(f, " ({d})")?; }
                Ok(())
            }
            ModuleSource::Url(u) => write!(f, "{u}"),
        }
    }
}

impl ModuleDep {
    pub fn version(&self) -> &str {
        match self {
            ModuleDep::Simple(v) => v,
            ModuleDep::Extended(spec) => spec.version.as_deref().unwrap_or("*"),
        }
    }

    /// `Ok(None)` means no source was declared (a bare version string,
    /// or a table with only `version`). An error means the keys
    /// contradict each other. These are the same rules `baler install`
    /// applies to its flags.
    pub fn source(&self) -> Result<Option<ModuleSource>> {
        let ModuleDep::Extended(s) = self else { return Ok(None) };

        let ref_count = [&s.branch, &s.tag, &s.rev].iter().filter(|r| r.is_some()).count();
        if ref_count > 1 {
            bail!("`branch`, `tag`, and `rev` are mutually exclusive, pick one ref.");
        }
        let git_only_keys = ref_count > 0 || s.module_dir.is_some();

        match (&s.git, &s.url) {
            (Some(_), Some(_)) => {
                bail!("`git` and `url` are mutually exclusive, a module comes from one place.")
            }
            (Some(url), None) => Ok(Some(ModuleSource::Git {
                url: url.clone(),
                git_ref: s.branch.clone().or_else(|| s.tag.clone()).or_else(|| s.rev.clone()),
                module_dir: s.module_dir.clone(),
            })),
            (None, Some(url)) => {
                if git_only_keys {
                    bail!("`branch`, `tag`, `rev`, and `module_dir` only apply to `git`, not `url`.");
                }
                Ok(Some(ModuleSource::Url(url.clone())))
            }
            (None, None) => {
                if git_only_keys {
                    bail!("`branch`, `tag`, `rev`, and `module_dir` need a `git` source to apply to.");
                }
                Ok(None)
            }
        }
    }
}

// ---- Dependencies ----

/// Shape of `[dependencies]`: only things `{box}` actually loads at
/// runtime, meaning R packages and other box modules. Plain keys are R
/// packages, resolved off CRAN (or `repo`, if set); `baler` is box
/// modules, told apart from a package entry by table membership
/// rather than a `type`/`mode` field on each entry (an R package
/// literally named `baler` therefore can't be a bare key here, an
/// accepted tradeoff). An external tool (a linter, Quarto) is never
/// loaded by `box`, so it has no place in this table at all. See
/// `Extras::external` (in `extras.rs`), which is where it belongs
/// instead.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Dependencies {
    #[serde(flatten)]
    pub packages: BTreeMap<String, PackageDep>,
    pub baler: Option<BTreeMap<String, ModuleDep>>,
}
