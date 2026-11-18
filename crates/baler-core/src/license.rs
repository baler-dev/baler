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

/// The deprecated SPDX identifiers in `expression`, such as `GPL-3.0`.
///
/// The strict parser refuses these with a bare "deprecated" reason. The lax
/// parser still reads them, so it is used here to name each one.
fn deprecated_ids(expression: &str) -> Vec<&'static str> {
    let Ok(parsed) = spdx::Expression::parse_mode(expression, spdx::ParseMode::LAX) else {
        return Vec::new();
    };
    let mut names: Vec<&'static str> = parsed
        .requirements()
        .filter_map(|req| req.req.license.id())
        .filter(|id| id.is_deprecated())
        .map(|id| id.name)
        .collect();
    names.dedup();
    names
}

/// How to fix a deprecated identifier. The GPL family was split into
/// `-only` and `-or-later`, which is the one replacement that can be named
/// without looking it up. Anything else is pointed at the list.
fn deprecated_advice(id: &str) -> String {
    let split = ["GPL-", "LGPL-", "AGPL-", "GFDL-"];
    if split.iter().any(|family| id.starts_with(family)) {
        format!(
            "Replace `{id}` with \"{id}-only\" or \"{id}-or-later\", whichever your \
             license says."
        )
    } else {
        format!("Replace `{id}`, see https://spdx.org/licenses/ for its replacement.")
    }
}

/// What is wrong with a project's license, one line for each problem and
/// empty when there is nothing to say. `baler bundle` prints these as
/// warnings and carries on, so a project that predates the check still
/// bundles.
///
/// What gets checked is the relationship between the `license` field and the
/// license file, the same way `Cargo`, PEP 639, and `npm` treat them. A license is
/// either declared or it isn’t, and the text is expected to travel with the
/// code. We never try to match the file contents against the declared
/// identifier. Usually packaging tools don’t make that guess at build time.
///
/// Summary:  
/// 
/// - When a license field is unset and has no file, nothing is declared (one message).
/// - When a license field is unset but has a file present, `{baler}` point out the file and suggest setting the field.
/// - As long as the license field has a valid SPDX id but has no file, 
///      the package is shipping without the license text.
/// - Follow up: Valid SPDX id + file, just silence.
/// - Anything that isn’t a valid SPDX id, it explains why it’s rejected, and for
///      deprecated identifiers say what to use instead.
pub fn warnings(license: &str, files: &ProjectFiles) -> Vec<String> {
    let trimmed = license.trim();
    let shown = if trimmed.is_empty() {
        "empty".to_owned()
    } else {
        format!("\"{trimmed}\"")
    };
    let first_file = files
        .licenses
        .first()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned());

    if is_unset(license) {
        let message = match first_file {
            Some(file) => format!(
                "Found {file} at the project root, but `license` in baler.toml is {shown}. \
                 Set it to the SPDX identifier of that license, such as \"MIT\", \
                 \"Apache-2.0\" or \"GPL-3.0-or-later\"."
            ),
            None => format!(
                "No license is declared: `license` in baler.toml is {shown} and there is no \
                 license file (LICENSE, LICENSE.md, LICENSE.txt or COPYING) at the project \
                 root. Set `license` to an SPDX identifier such as \"MIT\" or \
                 \"Apache-2.0\", and add the license text."
            ),
        };
        return vec![message];
    }

    let mut found = Vec::new();

    if let Err(reason) = validate_spdx(trimmed) {
        let deprecated = deprecated_ids(trimmed);
        if deprecated.is_empty() {
            found.push(format!(
                "`license = \"{license}\"` in baler.toml is not a valid SPDX expression \
                 ({reason}). Use an identifier from https://spdx.org/licenses/, such as \
                 \"MIT\" or \"GPL-3.0-or-later\"."
            ));
        } else {
            let advice: Vec<String> = deprecated.iter().map(|id| deprecated_advice(id)).collect();
            found.push(format!(
                "`license = \"{license}\"` in baler.toml uses a deprecated SPDX identifier. {}",
                advice.join(" ")
            ));
        }
    }

    if first_file.is_none() {
        found.push(
            "No license file (LICENSE, LICENSE.md, LICENSE.txt or COPYING) at the project \
             root, so this bundle ships without a license text."
                .to_owned(),
        );
    }

    found
}
