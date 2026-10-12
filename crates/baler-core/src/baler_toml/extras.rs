use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::dependencies::{ModuleDep, PackageDep};

// ---- Extras ----

/// Shape of `[extras]`: everything `[dependencies]` holds (`packages`,
/// `baler`), plus `external`: tools used outside `box`'s runtime
/// entirely (Quarto for docs, a linter, a Python package a script
/// shells out to). `external` has no counterpart under
/// `[dependencies]` on purpose: nothing in it is ever loaded by
/// `box`, so it was never a runtime dependency to begin with, only
/// ever an `[extras]` one.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Extras {
    #[serde(flatten)]
    pub packages: BTreeMap<String, PackageDep>,
    pub baler: Option<BTreeMap<String, ModuleDep>>,
    pub external: Option<BTreeMap<String, ExternalToolDep>>,
}

// ---- ExternalToolDep ----

/// A `[extras.external]` entry: something baler neither fetches nor
/// installs itself, so it can only check for it and report. Which
/// kind of thing it is decides what "check" means:
///
/// - `cli` (the default): the key is a command name on PATH, the
///   version comes from `<tool> --version`, and `version` is a semver
///   requirement.
/// - `python`: the key is a Python distribution name (`scikit-learn`,
///   not `sklearn`), looked up in one resolved interpreter's package
///   metadata, and `version` is a PEP 440 specifier (`>=1.24,<2`).
///   `index` and `git` say where the package comes from when that
///   isn't PyPI. They only change the install hint baler prints,
///   because detection doesn't care where a package came from.
///
/// A bare string (`pandoc = ">=3.0"`) is a `cli` entry, so manifests
/// written before `type` existed keep parsing. Deserialization goes
/// through `RawExternalToolDep` because a missing `type` has to
/// default to `cli`, which `#[serde(tag = ...)]` can't express.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    try_from = "RawExternalToolDep"
)]
pub enum ExternalToolDep {
    Cli {
        version: String,
        /// Shown when the tool is missing, e.g. a download page.
        #[serde(skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
    Python {
        version: String,
        /// Interpreter to check instead of the auto-resolved one.
        #[serde(skip_serializing_if = "Option::is_none")]
        python: Option<String>,
        /// Private index URL. Install hint only.
        #[serde(skip_serializing_if = "Option::is_none")]
        index: Option<String>,
        /// Git URL. Install hint only. Conflicts with `index`.
        #[serde(skip_serializing_if = "Option::is_none")]
        git: Option<String>,
    },
}

impl ExternalToolDep {
    pub fn version(&self) -> &str {
        match self {
            ExternalToolDep::Cli { version, .. } | ExternalToolDep::Python { version, .. } => {
                version
            }
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            ExternalToolDep::Cli { .. } => "cli",
            ExternalToolDep::Python { .. } => "python",
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawExternalToolDep {
    Simple(String),
    Table(RawExternalTable),
}

/// Strict on purpose: a typo like `hnit = ...` or `tpye = "python"`
/// is an error instead of a silently ignored key.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExternalTable {
    #[serde(rename = "type")]
    kind: Option<String>,
    version: String,
    hint: Option<String>,
    python: Option<String>,
    index: Option<String>,
    git: Option<String>,
}

impl TryFrom<RawExternalToolDep> for ExternalToolDep {
    type Error = String;

    fn try_from(raw: RawExternalToolDep) -> Result<Self, String> {
        let t = match raw {
            RawExternalToolDep::Simple(version) => {
                return Ok(ExternalToolDep::Cli { version, hint: None });
            }
            RawExternalToolDep::Table(t) => t,
        };

        match t.kind.as_deref().unwrap_or("cli") {
            "cli" => {
                if t.python.is_some() || t.index.is_some() || t.git.is_some() {
                    return Err(
                        "`python`, `index` and `git` only apply to `type = \"python\"`".into(),
                    );
                }
                Ok(ExternalToolDep::Cli { version: t.version, hint: t.hint })
            }
            "python" => {
                if t.hint.is_some() {
                    return Err("`hint` only applies to `type = \"cli\"`".into());
                }
                if t.index.is_some() && t.git.is_some() {
                    return Err("`index` and `git` cannot be set together".into());
                }
                Ok(ExternalToolDep::Python {
                    version: t.version,
                    python: t.python,
                    index: t.index,
                    git: t.git,
                })
            }
            other => Err(format!(
                "unknown external type '{other}' (expected \"cli\" or \"python\")"
            )),
        }
    }
}
