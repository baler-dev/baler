use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::version::VersionSpec;

use super::author::Author;

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ModuleMeta {
    pub name: String,
    pub version: String,
    pub description: String,
    pub readme: Option<String>,
    pub authors: Vec<Author>,
    pub license: String,
    pub r_version: String,
    pub repository: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub src: Option<String>,
}

impl ModuleMeta {
    pub fn r_version_spec(&self) -> Result<VersionSpec> {
        VersionSpec::parse(&self.r_version)
    }

    /// `version` must be strict semver (`MAJOR.MINOR.PATCH`), since
    /// dependency ranges (`^`, `>=`, etc.) in another module's
    /// `[dependencies.baler]` entry resolve against it. An
    /// unvalidated `version` here doesn't fail where the mistake was
    /// made, it fails later, inside whoever consumes this module.
    pub fn semver(&self) -> Result<semver::Version> {
        semver::Version::parse(&self.version).map_err(|e| anyhow::anyhow!(
            "Invalid `version` '{}' in baler.toml: {}. \
             `version` must be a valid semver string (e.g. \"1.2.3\"), \
             since dependency ranges (^, >=, etc.) resolve against it.",
            self.version, e
        ))
    }
}
