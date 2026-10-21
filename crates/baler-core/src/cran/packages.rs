use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use semver::Version;

use crate::ver::Ver;

/// Packages that ship with R itself (they never installable from CRAN).
const BASE_PACKAGES: &[&str] = &[
    // Native packages (base)
    "R", "base", "compiler", "datasets", "graphics", "grDevices",
    "grid", "methods", "parallel", "splines", "stats", "stats4",
    "tcltk", "tools", "utils",
    // recommended packages: ship with R, not on CRAN src/contrib
    "boot", "class", "cluster", "codetools", "foreign", "KernSmooth",
    "lattice", "MASS", "Matrix", "mgcv", "nlme", "nnet", "rpart",
    "spatial", "survival",
];

/// True for packages that ship with R and are on no repository index.
pub fn is_base_package(name: &str) -> bool {
    BASE_PACKAGES.contains(&name)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RVersion {
    raw: String,
    ver: Ver,
    semver: Version,
}

impl RVersion {
    /// The one place a CRAN-style version string becomes a value. Used
    /// for every version that comes from PACKAGES.gz, a CRAN Archive
    /// listing, or a package's own installed DESCRIPTION, so all three
    /// read versions identically instead of drifting into their own
    /// slightly-different parsing. `ver()` is the R-ordered value to
    /// compare. `semver()` is derived from it for code that still holds
    /// `semver` values, and a zero-padded component (`1.0-01`), which
    /// `semver` rejects, no longer makes a package disappear.
    pub fn parse(raw: &str) -> Result<Self> {
        let ver = Ver::parse(raw).with_context(|| format!("Failed to parse version: {raw}"))?;
        let semver = ver.to_semver();
        Ok(Self { raw: ver.as_str().to_owned(), ver, semver })
    }

    /// A version with no real raw string behind it, e.g. reused across
    /// a repo boundary where only the parsed `Version` survived. Falls
    /// back to `Version`'s own `Display`, which round-trips correctly
    /// for ordinary 3-component versions but not for anything that
    /// went through the fourth-component-as-pre-release reshaping in
    /// `Ver::to_semver`, nor for a dash that was not a dot (`0.3-1`).
    /// That is a real, disclosed gap, not a silent one. `ver()` is exact
    /// for the pre-release spelling. A pre-release that is not numeric
    /// cannot come from an R version, so it falls back to its first three
    /// components instead of failing, and callers that read pins from a
    /// lock check them with `Ver::from_semver` first.
    pub fn synthetic(semver: Version) -> Self {
        let raw = semver.to_string();
        let ver = Ver::from_semver(&semver)
            .unwrap_or_else(|_| Ver::from_parts(vec![semver.major, semver.minor, semver.patch]));
        Self { raw, ver, semver }
    }

    /// The exact string as published. Use this, never `Display` on the
    /// `semver::Version`, when building a CRAN URL or filename.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// The value to compare and match against requirements, with R's
    /// ordering.
    pub fn ver(&self) -> &Ver {
        &self.ver
    }

    pub fn semver(&self) -> &Version {
        &self.semver
    }
}

impl std::fmt::Display for RVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.raw)
    }
}

#[derive(Debug, Clone)]
pub struct PackageRecord {
    pub version: RVersion,
    pub deps: Vec<(String, String)>,
}

/// Shared client for every request this module makes. A bare
/// `reqwest::blocking::get` has no timeout at all. A stalled connection
/// (mirror hung, network partition mid-download) would block the whole
/// install indefinitely instead of failing with a message pointing at
/// the actual cause.
fn http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")
}

/// Fetch and parse `PACKAGES.gz` from a CRAN-like repository URL.
pub fn fetch(repo_url: &str) -> Result<HashMap<String, PackageRecord>> {
    let url = format!(
        "{}/src/contrib/PACKAGES.gz",
        repo_url.trim_end_matches('/')
    );
    let client = http_client()?;

    let mut last_err = None;
    for attempt in 1..=3 {
        match client.get(&url).send() {
            Ok(response) if response.status().is_success() => {
                let bytes = response.bytes().context("Failed to read PACKAGES.gz bytes")?;
                let gz = GzDecoder::new(bytes.as_ref());
                return parse_dcf(BufReader::new(gz));
            }
            Ok(response) => {
                let status = response.status();
                eprintln!("  [warn] attempt {}/3: HTTP {} fetching index, retrying...", attempt, status);
                last_err = Some(anyhow::anyhow!("HTTP {} fetching package index: {}", status, url));
            }
            Err(e) => {
                eprintln!("  [warn] attempt {}/3: request failed, retrying...", attempt);
                last_err = Some(anyhow::anyhow!("Failed to fetch package index: {}: {}", url, e));
            }
        }
        if attempt < 3 {
            std::thread::sleep(std::time::Duration::from_secs(2u64.pow(attempt)));
        }
    }

    Err(last_err.unwrap())
}

/// This fetches all archived versions of a package from CRAN's Archive/ HTML index.
/// Returns versions sorted newest-first. An empty result (rather than an
/// error) means the package simply has no archive (not every package does).
pub fn fetch_archive_versions(repo_url: &str, pkg: &str) -> Result<Vec<RVersion>> {
    let url = format!(
        "{}/src/contrib/Archive/{}/",
        repo_url.trim_end_matches('/'),
        pkg
    );

    let response = http_client()?
        .get(&url)
        .send()
        .with_context(|| format!("Failed to fetch archive listing: {}", url))?;

    if !response.status().is_success() {
        return Ok(Vec::new());
    }

    let body = response.text()
        .with_context(|| format!("Failed to read archive listing body: {}", url))?;

    Ok(parse_archive_listing(&body, pkg))
}

/// The `Depends`, `Imports` and `LinkingTo` entries of one archived release.
///
/// The CRAN index only describes the newest release of each package, so an
/// older release's real dependencies live in its own tarball. This downloads
/// that tarball and reads `DESCRIPTION` from it. It is called only when the
/// solver has to step off the newest release.
pub fn fetch_archive_deps(
    repo_url: &str,
    pkg: &str,
    version: &RVersion,
) -> Result<Vec<(String, String)>> {
    let url = format!(
        "{}/src/contrib/Archive/{}/{}_{}.tar.gz",
        repo_url.trim_end_matches('/'),
        pkg,
        pkg,
        version.as_str()
    );
    let response = http_client()?
        .get(&url)
        .send()
        .with_context(|| format!("Failed to download {url}"))?
        .error_for_status()
        .with_context(|| format!("Failed to download {url}"))?;
    let bytes = response
        .bytes()
        .with_context(|| format!("Failed to read the body of {url}"))?;
    description_deps(bytes.as_ref(), pkg)
        .with_context(|| format!("Failed to read the dependencies of {pkg} {version} from {url}"))
}

/// Dependencies declared by the `DESCRIPTION` inside a package tarball.
pub fn description_deps(tarball: &[u8], pkg: &str) -> Result<Vec<(String, String)>> {
    let wanted = std::path::PathBuf::from(pkg).join("DESCRIPTION");
    let mut archive = tar::Archive::new(GzDecoder::new(tarball));
    for entry in archive.entries().context("Failed to list the tarball")? {
        let mut entry = entry.context("Failed to read a tarball entry")?;
        if entry.path().context("Bad path in the tarball")? != wanted {
            continue;
        }
        let mut text = String::new();
        entry
            .read_to_string(&mut text)
            .context("DESCRIPTION is not valid UTF-8")?;
        let records = parse_dcf(text.as_bytes())?;
        return match records.into_values().next() {
            Some(record) => Ok(record.deps),
            None => bail!("DESCRIPTION has no readable Package and Version fields"),
        };
    }
    bail!("{} is not in the tarball", wanted.display())
}

/// Parse an Apache-style HTML directory listing for `{pkg}_{version}.tar.gz`
/// links. Deliberately a simple substring scan rather than a full HTML
/// parser. CRAN's Archive listings are consistently plain `<a href="...">`
/// tags, so this avoids pulling in an HTML parsing dependency for one job.
pub fn parse_archive_listing(html: &str, pkg: &str) -> Vec<RVersion> {
    let prefix = format!("{}_", pkg);
    let mut versions = Vec::new();

    for part in html.split("href=\"") {
        let Some(end) = part.find('"') else { continue };
        let href = &part[..end];

        let Some(name) = href.strip_suffix(".tar.gz") else { continue };
        let Some(ver_str) = name.strip_prefix(&prefix) else { continue };

        if let Ok(v) = RVersion::parse(ver_str) {
            versions.push(v);
        }
    }

    versions.sort_by(|a, b| b.ver().cmp(a.ver()));
    versions
}

/// One record of a DCF file while it is being read.
#[derive(Default)]
struct Pending {
    name: Option<String>,
    version: Option<String>,
    deps: Vec<(String, String)>,
    /// The dependency field being read, joined across its continuation
    /// lines. These fields are wrapped at a fixed width, even in the middle
    /// of an entry (`rlang (>=` then `1.3.0)`), so a field can only be
    /// parsed once all of it has been read.
    field: Option<String>,
    problem: Option<String>,
}

impl Pending {
    fn finish_field(&mut self) {
        let Some(text) = self.field.take() else {
            return;
        };
        match parse_dep_field(&text) {
            Ok(found) => self.deps.extend(found),
            Err(error) => {
                self.problem.get_or_insert(format!("{error:#}"));
            }
        }
    }
}

pub fn parse_dcf(reader: impl BufRead) -> Result<HashMap<String, PackageRecord>> {
    let mut map = HashMap::new();
    let mut pending = Pending::default();

    for line in reader.lines() {
        let line = line.context("Failed to read line from PACKAGES")?;

        if line.is_empty() {
            pending.finish_field();
            flush(&mut map, std::mem::take(&mut pending));
            continue;
        }

        // Continuation line (starts with whitespace)
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(text) = pending.field.as_mut() {
                text.push(' ');
                text.push_str(line.trim());
            }
            continue;
        }

        // New field
        pending.finish_field();
        if let Some((key, val)) = line.split_once(": ") {
            match key {
                "Package" => pending.name = Some(val.to_owned()),
                "Version" => pending.version = Some(val.to_owned()),
                "Imports" | "Depends" | "LinkingTo" => pending.field = Some(val.to_owned()),
                _ => {}
            }
        }
    }

    // Final block, the file may not end with a blank line
    pending.finish_field();
    flush(&mut map, pending);

    Ok(map)
}

/// Add a finished record to the index. A record that cannot be read is
/// skipped with a warning that says why, so a package never turns up with
/// half its dependencies.
fn flush(map: &mut HashMap<String, PackageRecord>, pending: Pending) {
    let (Some(name), Some(version)) = (pending.name, pending.version) else {
        return;
    };
    if let Some(problem) = pending.problem {
        eprintln!("  [warn] skipping '{name}' in the package index: {problem}");
        return;
    }
    match RVersion::parse(&version) {
        Ok(parsed) => {
            map.insert(name, PackageRecord { version: parsed, deps: pending.deps });
        }
        Err(error) => eprintln!("  [warn] skipping '{name}' in the package index: {error:#}"),
    }
}

/// Parse a comma-separated dep field into bare package names.
/// Handles both `"rlang (>= 1.0.0)"` and `"rlang(>=1.0.0)"` (no space).
/// Filters out base and recommended packages.
pub fn parse_dep_field(s: &str) -> Result<Vec<(String, String)>> {
    let mut found = Vec::new();
    for entry in s.split(',') {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (bare, constraint) = match trimmed.split_once('(') {
            Some((name, rest)) => (name.trim(), rest.trim_end_matches(')').trim().to_owned()),
            None => {
                let bare = trimmed.split_once(' ').map(|(n, _)| n).unwrap_or(trimmed);
                (bare.trim(), "*".to_owned())
            }
        };
        if !is_package_name(bare) {
            bail!("{trimmed:?} is not a package name with an optional (constraint)");
        }
        if BASE_PACKAGES.contains(&bare) {
            continue;
        }
        found.push((bare.to_owned(), constraint));
    }
    Ok(found)
}

fn is_package_name(text: &str) -> bool {
    text.chars().next().is_some_and(|first| first.is_ascii_alphabetic())
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
}
