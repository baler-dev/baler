//! Talks to GitHub Releases: finds a release, downloads its files, and checks
//! them against the SHA-256 file published next to each archive.

use std::fmt;
use std::fs::File;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use baler_core::cran::client::git::token_for;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const DEFAULT_API: &str = "https://api.github.com";
const REPO: &str = "baler-dev/baler";

/// Points the release lookup at another API root, such as a mirror or a test
/// server. The GitHub token is only ever sent to `api.github.com`, so this
/// cannot be used to hand it to another host.
const API_ENV: &str = "BALER_RELEASE_API";

#[derive(Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
}

impl Release {
    pub fn find(&self, name: &str) -> Option<&ReleaseAsset> {
        self.assets.iter().find(|asset| asset.name == name)
    }

    pub fn asset_names(&self) -> Vec<String> {
        self.assets.iter().map(|asset| asset.name.clone()).collect()
    }
}

/// An error that a second attempt can fix. The latest release has its {box}
/// archives replaced every few hours (new ones go up first, then the old ones
/// are removed), so a download that starts just before that can find its file
/// gone, or fetch an archive and a checksum from two different generations.
#[derive(Debug)]
pub struct Retryable(pub String);

impl fmt::Display for Retryable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Retryable {}

/// An answer other than success, kept as a type so callers can tell a 404
/// from other failures without reading the message.
#[derive(Debug)]
struct HttpStatus {
    code: u16,
    url: String,
}

impl fmt::Display for HttpStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HTTP {} from {}", self.code, self.url)
    }
}

impl std::error::Error for HttpStatus {}

fn is_not_found(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<HttpStatus>()
        .is_some_and(|status| status.code == 404)
}

/// A tag ends up in a URL, so it is held to the characters tags are made of.
pub fn check_tag(tag: &str) -> Result<()> {
    let ok = !tag.is_empty()
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !ok {
        bail!("'{tag}' is not a release tag. Tags hold letters, digits, '.', '_' and '-'.");
    }
    Ok(())
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent("baler")
        .timeout(Duration::from_secs(300))
        .build()
        .context("Failed to build HTTP client")
}

fn api_root() -> String {
    match std::env::var(API_ENV) {
        Ok(root) if !root.is_empty() => root.trim_end_matches('/').to_owned(),
        _ => DEFAULT_API.to_owned(),
    }
}

fn send(url: &str, accept: Option<&str>) -> Result<reqwest::blocking::Response> {
    let mut request = client()?.get(url);
    if let Some(accept) = accept {
        request = request.header("Accept", accept);
    }
    let has_token = match token_for(url) {
        Some(token) => {
            request = request.bearer_auth(token);
            true
        }
        None => false,
    };
    let response = request
        .send()
        .with_context(|| format!("Failed to reach {url}"))?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    match status.as_u16() {
        403 | 429 if !has_token => bail!(
            "GitHub refused {url} (HTTP {status}), most likely its rate limit for anonymous \
             requests. Set GITHUB_TOKEN (or GH_TOKEN) to raise it."
        ),
        code => Err(anyhow::Error::new(HttpStatus {
            code,
            url: url.to_owned(),
        })),
    }
}

/// The release marked Latest, or the one tagged `tag`.
pub fn fetch_release(tag: Option<&str>) -> Result<Release> {
    let root = api_root();
    let url = match tag {
        Some(tag) => format!("{root}/repos/{REPO}/releases/tags/{tag}"),
        None => format!("{root}/repos/{REPO}/releases/latest"),
    };
    let response = match send(&url, Some("application/vnd.github+json")) {
        Ok(response) => response,
        Err(e) if is_not_found(&e) => match tag {
            Some(tag) => bail!("{REPO} has no release tagged '{tag}'."),
            None => bail!("{REPO} has no published release yet."),
        },
        Err(e) => return Err(e),
    };
    response
        .json::<Release>()
        .with_context(|| format!("GitHub answered {url} with something that is not a release"))
}

/// A release file that is gone is the sign of a refresh in progress, so it is
/// worth a second attempt rather than a failure.
fn send_asset(url: &str) -> Result<reqwest::blocking::Response> {
    send(url, None).map_err(|e| {
        if is_not_found(&e) {
            anyhow::Error::new(Retryable(format!("{url} is gone from the release")))
        } else {
            e
        }
    })
}

pub fn download_text(url: &str) -> Result<String> {
    send_asset(url)?
        .text()
        .with_context(|| format!("Failed to read {url}"))
}

pub fn download_file(url: &str, dest: &Path) -> Result<()> {
    let mut response = send_asset(url)?;
    let mut file = File::create(dest)
        .with_context(|| format!("Failed to create {}", dest.display()))?;
    response
        .copy_to(&mut file)
        .with_context(|| format!("Failed to download {url}"))?;
    Ok(())
}

/// The hash in a `.sha256` file, which holds the hash and then the file name,
/// the format `sha256sum` writes.
pub fn parse_checksum(text: &str) -> Result<String> {
    let hash = text
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("The checksum file is not valid, so the download cannot be verified.");
    }
    Ok(hash)
}

pub fn sha256_of(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .with_context(|| format!("Failed to open {}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub fn verify_sha256(path: &Path, expected: &str, label: &str) -> Result<()> {
    let actual = sha256_of(path)?;
    if actual != expected {
        return Err(anyhow::Error::new(Retryable(format!(
            "Checksum mismatch for {label} (expected {expected}, got {actual})"
        ))));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_release_tags() {
        assert!(check_tag("v0.2.0").is_ok());
        assert!(check_tag("v0.2.0-rc.1").is_ok());
    }

    #[test]
    fn rejects_anything_that_could_change_the_url() {
        for bad in ["", "v1/../x", "v1?x=1", "v1 2", "v1#x", "a%2fb"] {
            assert!(check_tag(bad).is_err(), "'{bad}' should be rejected");
        }
    }

    #[test]
    fn reads_a_sha256sum_file() {
        let hash = "A".repeat(64);
        let line = format!("{hash}  box_1.2.3_R4.5_linux-x86_64.tar.gz\n");
        assert_eq!(parse_checksum(&line).unwrap(), "a".repeat(64));
    }

    #[test]
    fn rejects_a_malformed_checksum_file() {
        assert!(parse_checksum("").is_err());
        assert!(parse_checksum("not-a-hash file").is_err());
        assert!(parse_checksum(&"a".repeat(63)).is_err());
        assert!(parse_checksum(&"g".repeat(64)).is_err());
    }

    #[test]
    fn hashes_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        std::fs::write(&path, "hello").unwrap();
        assert_eq!(
            sha256_of(&path).unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn a_mismatch_is_retryable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        std::fs::write(&path, "hello").unwrap();
        let error = verify_sha256(&path, &"0".repeat(64), "hello.txt").unwrap_err();
        assert!(error.downcast_ref::<Retryable>().is_some());
        assert!(verify_sha256(
            &path,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
            "hello.txt"
        )
        .is_ok());
    }
}
