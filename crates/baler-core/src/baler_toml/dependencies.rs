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

#[derive(Debug, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum ModuleDep {
    Simple(String),
    Extended { version: String, source: Option<String> },
}

impl ModuleDep {
    pub fn version(&self) -> &str {
        match self {
            ModuleDep::Simple(v) => v,
            ModuleDep::Extended { version, .. } => version,
        }
    }

    pub fn source(&self) -> Option<&str> {
        match self {
            ModuleDep::Simple(_) => None,
            ModuleDep::Extended { source, .. } => source.as_deref(),
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
