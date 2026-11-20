//! Picks the {box} build that matches this machine out of a release's assets.
//!
//! This is plain string work on purpose, so every platform and R version can
//! be tested without a network or an R installation.
//!
//! Release assets are named `box_<version>_R<major.minor>_<os>-<arch>.<ext>`,
//! for example `box_1.2.3.261119_R4.5_linux-x86_64.tar.gz`. The version is not
//! known ahead of time, so the match is on everything after it.

use std::collections::BTreeSet;

use anyhow::{bail, Result};
use baler_core::paths::{RPlatform, RPlatformOs};

const EXTENSIONS: [&str; 3] = ["tar.gz", "tgz", "zip"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub os: &'static str,
    pub arch: String,
    pub r_minor: String,
}

impl Target {
    pub fn new(os: &'static str, arch: &str, r_minor: &str) -> Self {
        Self {
            os,
            arch: normalize_arch(arch),
            r_minor: r_minor.to_owned(),
        }
    }

    pub fn from_platform(platform: &RPlatform) -> Result<Self> {
        let os = match &platform.os {
            RPlatformOs::Windows => "windows",
            RPlatformOs::MacOs => "macos",
            RPlatformOs::Linux(_) => "linux",
            RPlatformOs::Other => bail!(
                "No pre-built {{box}} exists for this operating system. \
                 Build it from source instead."
            ),
        };
        Ok(Self::new(os, &platform.arch, &platform.r_version_short))
    }

    pub fn label(&self) -> String {
        format!("R {} on {}-{}", self.r_minor, self.os, self.arch)
    }

    fn needle(&self) -> String {
        format!("_R{}_{}-{}.", self.r_minor, self.os, self.arch)
    }
}

/// R reports `aarch64` on Apple silicon and Linux ARM, but other tools say
/// `arm64`, so both spellings end up as the one the release files use.
fn normalize_arch(arch: &str) -> String {
    match arch {
        "arm64" => "aarch64".to_owned(),
        "amd64" | "x64" => "x86_64".to_owned(),
        other => other.to_owned(),
    }
}

/// The archive format of a {box} asset, or `None` for anything else, such as
/// the `.sha256` file that sits next to every archive.
pub fn archive_extension(name: &str) -> Option<&'static str> {
    EXTENSIONS
        .iter()
        .find(|ext| name.ends_with(&format!(".{ext}")))
        .copied()
}

/// The asset built for `target`, if the release has one. Matching on
/// `_R4.5_` (underscore included) keeps R 4.5 from matching R 4.50.
pub fn pick_asset<'a>(names: &'a [String], target: &Target) -> Option<&'a str> {
    let needle = target.needle();
    names
        .iter()
        .map(String::as_str)
        .filter(|name| {
            name.starts_with("box_") && name.contains(&needle) && archive_extension(name).is_some()
        })
        .max()
}

/// Every R minor version the release has a build for on this OS and CPU,
/// used to tell the user what they could do when their own R has none.
pub fn available_r_minors(names: &[String], os: &str, arch: &str) -> Vec<String> {
    let platform = format!("{os}-{arch}.");
    let minors: BTreeSet<String> = names
        .iter()
        .filter(|name| archive_extension(name).is_some())
        .filter_map(|name| {
            let rest = name.strip_prefix("box_")?;
            let mut parts = rest.splitn(3, '_');
            let _version = parts.next()?;
            let minor = parts.next()?.strip_prefix('R')?;
            let tail = parts.next()?;
            tail.starts_with(&platform).then(|| minor.to_owned())
        })
        .collect();
    minors.into_iter().collect()
}

/// Whether the release has any {box} archive at all, for any platform.
pub fn has_any_build(names: &[String]) -> bool {
    names
        .iter()
        .any(|name| name.starts_with("box_") && archive_extension(name).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn release() -> Vec<String> {
        names(&[
            "baler-v0.2.0-x86_64-unknown-linux-gnu.tar.gz",
            "baler-v0.2.0-x86_64-unknown-linux-gnu.tar.gz.sha256",
            "baler-installer.sh",
            "box-source.txt",
            "box_1.2.3.261119_R4.5_linux-x86_64.tar.gz",
            "box_1.2.3.261119_R4.5_linux-x86_64.tar.gz.sha256",
            "box_1.2.3.261119_R4.6_linux-x86_64.tar.gz",
            "box_1.2.3.261119_R4.5_linux-aarch64.tar.gz",
            "box_1.2.3.261119_R4.5_macos-aarch64.tgz",
            "box_1.2.3.261119_R4.5_macos-x86_64.tgz",
            "box_1.2.3.261119_R4.5_windows-x86_64.zip",
        ])
    }

    #[test]
    fn picks_the_build_for_each_platform() {
        let release = release();
        let cases = [
            ("linux", "x86_64", "4.5", "box_1.2.3.261119_R4.5_linux-x86_64.tar.gz"),
            ("linux", "x86_64", "4.6", "box_1.2.3.261119_R4.6_linux-x86_64.tar.gz"),
            ("linux", "aarch64", "4.5", "box_1.2.3.261119_R4.5_linux-aarch64.tar.gz"),
            ("macos", "aarch64", "4.5", "box_1.2.3.261119_R4.5_macos-aarch64.tgz"),
            ("macos", "x86_64", "4.5", "box_1.2.3.261119_R4.5_macos-x86_64.tgz"),
            ("windows", "x86_64", "4.5", "box_1.2.3.261119_R4.5_windows-x86_64.zip"),
        ];
        for (os, arch, minor, expected) in cases {
            let target = Target::new(os, arch, minor);
            assert_eq!(pick_asset(&release, &target), Some(expected), "{os}-{arch} R{minor}");
        }
    }

    #[test]
    fn never_picks_the_checksum_file() {
        let only_checksum = names(&["box_1.2.3_R4.5_linux-x86_64.tar.gz.sha256"]);
        let target = Target::new("linux", "x86_64", "4.5");
        assert_eq!(pick_asset(&only_checksum, &target), None);
    }

    #[test]
    fn an_r_minor_is_matched_whole() {
        let release = names(&["box_1.2.3_R4.50_linux-x86_64.tar.gz"]);
        let target = Target::new("linux", "x86_64", "4.5");
        assert_eq!(pick_asset(&release, &target), None);
    }

    #[test]
    fn no_build_for_an_r_version_the_release_lacks() {
        let target = Target::new("windows", "x86_64", "4.6");
        assert_eq!(pick_asset(&release(), &target), None);
    }

    #[test]
    fn arm64_is_read_as_aarch64() {
        let target = Target::new("macos", "arm64", "4.5");
        assert_eq!(target.arch, "aarch64");
        assert_eq!(
            pick_asset(&release(), &target),
            Some("box_1.2.3.261119_R4.5_macos-aarch64.tgz")
        );
    }

    #[test]
    fn lists_the_r_versions_a_platform_has() {
        assert_eq!(available_r_minors(&release(), "linux", "x86_64"), vec!["4.5", "4.6"]);
        assert_eq!(available_r_minors(&release(), "windows", "x86_64"), vec!["4.5"]);
        assert!(available_r_minors(&release(), "windows", "aarch64").is_empty());
    }

    #[test]
    fn knows_a_release_published_without_box() {
        assert!(has_any_build(&release()));
        assert!(!has_any_build(&names(&["baler-installer.sh", "box-source.txt"])));
    }

    #[test]
    fn knows_the_archive_formats() {
        assert_eq!(archive_extension("box_1_R4.5_linux-x86_64.tar.gz"), Some("tar.gz"));
        assert_eq!(archive_extension("box_1_R4.5_macos-aarch64.tgz"), Some("tgz"));
        assert_eq!(archive_extension("box_1_R4.5_windows-x86_64.zip"), Some("zip"));
        assert_eq!(archive_extension("box_1_R4.5_windows-x86_64.zip.sha256"), None);
    }
}
