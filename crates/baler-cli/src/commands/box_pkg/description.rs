//! Reads back what an installed {box} says about itself, without starting R.
//!
//! An installed R package keeps its `DESCRIPTION` file in the package
//! directory, and the {box} fork declares `Config/baler/support` in it. Next
//! to that, `baler box install` drops a `baler-source.txt` into the package
//! directory with the repository, ref and commit the build came from. It lives
//! inside the package, so reinstalling or removing {box} takes it along and
//! there is no separate state file that can go stale.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

pub const SUPPORT_FIELD: &str = "Config/baler/support";
pub const SOURCE_FILE: &str = "baler-source.txt";

/// The value of one field of a DESCRIPTION (DCF) file. A value can go on over
/// several lines, and the lines after the first start with whitespace.
pub fn dcf_field(text: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    let mut value: Option<String> = None;
    for line in text.lines() {
        if let Some(current) = value.as_mut() {
            if line.starts_with(' ') || line.starts_with('\t') {
                current.push(' ');
                current.push_str(line.trim());
                continue;
            }
            break;
        }
        if let Some(rest) = line.strip_prefix(&prefix) {
            value = Some(rest.trim().to_owned());
        }
    }
    value.filter(|v| !v.is_empty())
}

/// The protocol level a `Config/baler/support` value declares. `true` is
/// level 1, and a whole number leaves room to raise the level later without
/// breaking older builds. Anything else, `false` and `0` included, is no
/// support at all.
pub fn support_level(value: &str) -> Option<u32> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("true") {
        return Some(1);
    }
    value.parse::<u32>().ok().filter(|n| *n > 0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub repo: String,
    pub git_ref: String,
    pub sha: String,
}

impl Source {
    pub fn short_sha(&self) -> &str {
        &self.sha[..self.sha.len().min(7)]
    }
}

/// Reads `box-source.txt`: `repo=`, `ref=` and `sha=` lines, the format
/// `scripts/resolve-box.sh` writes.
pub fn parse_source(text: &str) -> Result<Source> {
    let mut repo = None;
    let mut git_ref = None;
    let mut sha = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().to_owned();
        match key.trim() {
            "repo" => repo = Some(value),
            "ref" => git_ref = Some(value),
            "sha" => sha = Some(value),
            _ => {}
        }
    }
    let Some(sha) = sha else {
        bail!("box-source.txt has no sha= line");
    };
    if !(7..=40).contains(&sha.len()) || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("box-source.txt has a sha= line that is not a commit hash: '{sha}'");
    }
    Ok(Source {
        repo: repo.unwrap_or_else(|| "unknown".to_owned()),
        git_ref: git_ref.unwrap_or_else(|| "unknown".to_owned()),
        sha: sha.to_ascii_lowercase(),
    })
}

#[derive(Debug)]
pub struct InstalledBox {
    pub version: Option<String>,
    pub support: Option<u32>,
    pub source: Option<Source>,
}

impl InstalledBox {
    /// Whether this {box} came from the baler fork. The `DESCRIPTION` field
    /// is the real marker. The source file counts too, so a build that was
    /// installed by `baler box install` is never mistaken for upstream, even
    /// if the field is renamed some day.
    pub fn is_baler_build(&self) -> bool {
        self.support.is_some() || self.source.is_some()
    }
}

#[derive(Debug)]
pub enum BoxState {
    Missing,
    /// `<lib>/box` exists but is not a readable R package.
    Damaged(PathBuf),
    Installed(InstalledBox),
}

pub fn inspect(lib: &Path) -> BoxState {
    let dir = lib.join("box");
    if !dir.exists() {
        return BoxState::Missing;
    }
    let Ok(description) = std::fs::read_to_string(dir.join("DESCRIPTION")) else {
        return BoxState::Damaged(dir);
    };
    let source = std::fs::read_to_string(dir.join(SOURCE_FILE))
        .ok()
        .and_then(|text| parse_source(&text).ok());
    BoxState::Installed(InstalledBox {
        version: dcf_field(&description, "Version"),
        support: dcf_field(&description, SUPPORT_FIELD).and_then(|v| support_level(&v)),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn reads_a_field() {
        let text = "Package: box\nVersion: 1.2.3.261119\nConfig/baler/support: true\n";
        assert_eq!(dcf_field(text, "Version").as_deref(), Some("1.2.3.261119"));
        assert_eq!(dcf_field(text, SUPPORT_FIELD).as_deref(), Some("true"));
        assert_eq!(dcf_field(text, "Depends"), None);
    }

    #[test]
    fn reads_a_field_that_goes_on_over_several_lines() {
        let text = "Title: Write Reusable,\n    Composable code\nVersion: 1\n";
        assert_eq!(dcf_field(text, "Title").as_deref(), Some("Write Reusable, Composable code"));
        assert_eq!(dcf_field(text, "Version").as_deref(), Some("1"));
    }

    #[test]
    fn handles_windows_line_endings() {
        let text = "Package: box\r\nVersion: 1.2.3\r\n";
        assert_eq!(dcf_field(text, "Version").as_deref(), Some("1.2.3"));
    }

    #[test]
    fn a_field_name_is_matched_whole() {
        let text = "Versions: 9\nVersion: 1\n";
        assert_eq!(dcf_field(text, "Version").as_deref(), Some("1"));
    }

    #[test]
    fn support_levels() {
        assert_eq!(support_level("true"), Some(1));
        assert_eq!(support_level("TRUE"), Some(1));
        assert_eq!(support_level("2"), Some(2));
        assert_eq!(support_level("false"), None);
        assert_eq!(support_level("0"), None);
        assert_eq!(support_level("yes please"), None);
    }

    #[test]
    fn parses_box_source() {
        let text = format!("repo=joshuamarie/box\nref=feature/baler-support\nsha={SHA}\n");
        let source = parse_source(&text).unwrap();
        assert_eq!(source.repo, "joshuamarie/box");
        assert_eq!(source.git_ref, "feature/baler-support");
        assert_eq!(source.sha, SHA);
        assert_eq!(source.short_sha(), "0123456");
    }

    #[test]
    fn box_source_needs_a_real_commit_hash() {
        assert!(parse_source("repo=a/b\nref=main\n").is_err());
        assert!(parse_source("sha=not-a-hash\n").is_err());
        assert!(parse_source("sha=abc\n").is_err());
    }

    #[test]
    fn inspects_an_install_directory() {
        let lib = tempfile::tempdir().unwrap();
        assert!(matches!(inspect(lib.path()), BoxState::Missing));

        let dir = lib.path().join("box");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(matches!(inspect(lib.path()), BoxState::Damaged(_)));

        std::fs::write(dir.join("DESCRIPTION"), "Package: box\nVersion: 1.2.3\n").unwrap();
        let BoxState::Installed(upstream) = inspect(lib.path()) else {
            panic!("expected an installed box");
        };
        assert_eq!(upstream.version.as_deref(), Some("1.2.3"));
        assert!(!upstream.is_baler_build());

        std::fs::write(
            dir.join("DESCRIPTION"),
            "Package: box\nVersion: 1.2.3\nConfig/baler/support: true\n",
        )
        .unwrap();
        let BoxState::Installed(fork) = inspect(lib.path()) else {
            panic!("expected an installed box");
        };
        assert_eq!(fork.support, Some(1));
        assert!(fork.is_baler_build());
    }
}
