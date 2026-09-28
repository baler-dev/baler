use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DEFAULT_CRAN_MIRROR: &str = "https://cloud.r-project.org";

// ---- PackageDep ----

#[derive(Debug, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum PackageDep {
    Simple(String),
    Extended { version: String, repo: Option<String> },
}

impl PackageDep {
    pub fn version(&self) -> &str {
        match self {
            PackageDep::Simple(v) => v,
            PackageDep::Extended { version, .. } => version,
        }
    }

    pub fn repo(&self) -> &str {
        match self {
            PackageDep::Simple(_) => DEFAULT_CRAN_MIRROR,
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
