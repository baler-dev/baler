use anyhow::{bail, Context, Result};

use std::fs;
use std::path::Path;

/// What `set_license` did to `baler.toml`.
pub enum Change {
    /// The file already held this license, so it was not written.
    Unchanged,
    /// The file now holds the license. `old` is what it held before, if
    /// it had a `license` at all.
    Set { old: Option<String> },
}

/// Sets `license` in the `[project]` table of an existing `baler.toml`.
///
/// Only that one line changes. Comments, spacing, the order of the keys
/// and the line endings stay as they were, and a trailing comment on the
/// line is kept. When there is no `license` yet, it is added after
/// `version`. The edited text is read back as TOML and compared with the
/// original with only the license changed, and nothing is written unless
/// the two match, so a file this does not understand is left alone.
pub(super) fn set_license(manifest: &Path, license: &str) -> Result<Change> {
    let text = fs::read_to_string(manifest)
        .with_context(|| format!("Failed to read {}", manifest.display()))?;
    let before: toml::Value = text
        .parse()
        .with_context(|| format!("{} is not valid TOML", manifest.display()))?;
    let Some(project) = before.get("project").and_then(|p| p.as_table()) else {
        bail!("{} has no [project] table, so there is no license to set.", manifest.display());
    };
    let old = project.get("license").and_then(|v| v.as_str()).map(str::to_owned);
    if old.as_deref() == Some(license) {
        return Ok(Change::Unchanged);
    }

    let hand = || {
        format!(
            "Could not change `license` in {} safely. Edit that line by hand to \
             license = \"{license}\".",
            manifest.display()
        )
    };
    let Some(edited) = edit(&text, license) else {
        bail!(hand());
    };

    let mut expected = before.clone();
    if let Some(table) = expected.get_mut("project").and_then(|p| p.as_table_mut()) {
        table.insert("license".to_owned(), toml::Value::String(license.to_owned()));
    }
    match edited.parse::<toml::Value>() {
        Ok(after) if after == expected => {}
        _ => bail!(hand()),
    }

    fs::write(manifest, edited)
        .with_context(|| format!("Failed to write {}", manifest.display()))?;
    Ok(Change::Set { old })
}

/// Splits a line into its text and its line ending (`\n`, `\r\n` or none).
fn split_eol(line: &str) -> (&str, &str) {
    let body = line.trim_end_matches(['\r', '\n']);
    (body, &line[body.len()..])
}

/// True when `line` sets `key`, as in `license = "MIT"`.
fn sets_key(line: &str, key: &str) -> bool {
    line.trim_start()
        .strip_prefix(key)
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

/// `line` with the string after its `=` replaced by `value`, keeping any
/// comment after it. `None` when the value is not a one-line string.
fn replace_value(line: &str, value: &str) -> Option<String> {
    let eq = line.find('=')?;
    let after = &line[eq + 1..];
    let start = eq + 1 + (after.len() - after.trim_start().len());
    let quote = line[start..].chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let mut escaped = false;
    let mut end = None;
    for (i, ch) in line[start + 1..].char_indices() {
        if quote == '"' && escaped {
            escaped = false;
        } else if quote == '"' && ch == '\\' {
            escaped = true;
        } else if ch == quote {
            end = Some(start + 1 + i + 1);
            break;
        }
    }
    let end = end?;
    Some(format!("{}{}{}", &line[..start], value, &line[end..]))
}

fn edit(text: &str, license: &str) -> Option<String> {
    let value = format!("\"{}\"", license.replace('\\', "\\\\").replace('"', "\\\""));
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_owned).collect();

    let mut in_project = false;
    let (mut header, mut version, mut current) = (None, None, None);
    for (i, line) in lines.iter().enumerate() {
        let (body, _) = split_eol(line);
        let trimmed = body.trim_start();
        if trimmed.starts_with('[') {
            let rest = trimmed.strip_prefix("[project]").map(str::trim_start);
            in_project = rest.is_some_and(|rest| rest.is_empty() || rest.starts_with('#'));
            if in_project && header.is_none() {
                header = Some(i);
            }
        } else if in_project && sets_key(body, "license") && current.is_none() {
            current = Some(i);
        } else if in_project && sets_key(body, "version") && version.is_none() {
            version = Some(i);
        }
    }

    if let Some(i) = current {
        let (body, eol) = split_eol(&lines[i]);
        let replaced = replace_value(body, &value)?;
        lines[i] = format!("{replaced}{eol}");
    } else {
        let after = version.or(header)?;
        let (_, eol) = split_eol(&lines[after]);
        let eol = if eol.is_empty() { "\n" } else { eol }.to_owned();
        if lines[after].ends_with(|c| c != '\n') {
            lines[after].push_str(&eol);
        }
        lines.insert(after + 1, format!("license = {value}{eol}"));
    }
    Some(lines.concat())
}
