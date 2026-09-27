use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::dependencies::{ModuleDep, PackageDep};

// ---- Extras ----

/// Shape of `[extras]`: everything `[dependencies]` holds (`packages`,
/// `baler`), plus `external`: CLI tools/binaries used outside `box`'s
/// runtime entirely (Quarto for docs, a linter, a formatter).
/// `external` has no counterpart under `[dependencies]` on purpose:
/// nothing in it is ever loaded by `box`, so it was never a runtime
/// dependency to begin with, only ever a `[extras]` one.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Extras {
    #[serde(flatten)]
    pub packages: BTreeMap<String, PackageDep>,
    pub baler: Option<BTreeMap<String, ModuleDep>>,
    pub external: Option<BTreeMap<String, ExternalToolDep>>,
}

// ---- ExternalToolDep ----

/// A `[extras.external]` entry: some CLI tool baler doesn't fetch or
/// install itself. There's no CRAN mirror or GitHub tarball
/// convention for an arbitrary system binary the way there is for R
/// packages and box modules, so baler can only check for one and
/// report on it, the same relationship `[project].r_version` already
/// has to R itself.
///
/// `Extended`'s flattened `extra` map is what keeps this tool-agnostic.
/// Quarto might need nothing past a version, a linter might need a
/// config path, some other tool three unrelated fields entirely.
/// baler's own schema shouldn't have to grow a field for every tool's
/// particular needs. baler itself only ever reads `version`; whatever
/// else an entry carries is between whoever wrote it and whatever
/// eventually consumes it.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum ExternalToolDep {
    Simple(String),
    Extended {
        version: String,
        #[serde(flatten)]
        extra: BTreeMap<String, serde_json::Value>,
    },
}

impl ExternalToolDep {
    pub fn version(&self) -> &str {
        match self {
            ExternalToolDep::Simple(v) => v,
            ExternalToolDep::Extended { version, .. } => version,
        }
    }
}
