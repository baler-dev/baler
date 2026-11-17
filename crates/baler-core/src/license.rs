//! Checks on a module's license: the `license` field in `baler.toml`,
//! and whether the project has a license file to go with it, the way an
//! R package pairs `License:` in `DESCRIPTION` with a `LICENSE` file.
//!
//! The field is an SPDX license expression: an identifier such as `MIT`
//! or `GPL-3.0-or-later`, or identifiers joined with `AND`, `OR` and
//! `WITH`. The list is at <https://spdx.org/licenses/>.

use crate::project_files::ProjectFiles;

/// What the generated `baler.toml` holds until someone picks a license.
pub const UNSET_LICENSE: &str = "Unknown";

/// Checks `expression` against the SPDX license list. `Err` holds the
/// reason, ready to put in a message.
pub fn validate_spdx(expression: &str) -> Result<(), String> {
    match spdx::Expression::parse(expression) {
        Ok(_) => Ok(()),
        Err(e) => Err(e.reason.to_string()),
    }
}

/// True when `license` says nothing: empty, or the `Unknown` that
/// `baler init` writes.
pub fn is_unset(license: &str) -> bool {
    let trimmed = license.trim();
    trimmed.is_empty() || trimmed.eq_ignore_ascii_case(UNSET_LICENSE)
}

/// What is wrong with a project's license, one line for each problem and
/// empty when there is nothing to say. `baler bundle` prints these as
/// warnings and carries on, so a project that predates the check still
/// bundles.
pub fn warnings(license: &str, files: &ProjectFiles) -> Vec<String> {
    let mut found = Vec::new();

    if is_unset(license) {
        found.push(format!(
            "`license` in baler.toml is \"{}\". Set it to an SPDX identifier such as \
             \"MIT\", \"Apache-2.0\" or \"GPL-3.0-or-later\".",
            license.trim()
        ));
    } else if let Err(reason) = validate_spdx(license.trim()) {
        found.push(format!(
            "`license = \"{}\"` in baler.toml is not a valid SPDX expression ({}). \
             Use an identifier from https://spdx.org/licenses/, such as \"MIT\" or \
             \"GPL-3.0-or-later\".",
            license, reason
        ));
    }

    if files.licenses.is_empty() {
        found.push(
            "No license file (LICENSE, LICENSE.md, LICENSE.txt or COPYING) at the project \
             root, so this bundle ships without a license text."
                .to_owned(),
        );
    }

    found
}
