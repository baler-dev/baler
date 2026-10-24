use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use anyhow::{bail, Context, Result};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::baler_toml::ModuleSource;

pub const LOCK_FILE_NAME: &str = "baler.lock";
/// Newest lock format this build can read. Version 3 added `[[module]]`
/// entries. A lock with no modules is still written as version 2, so a project
/// that does not use modules keeps a lock that older baler can read.
const LOCK_FORMAT_VERSION: u32 = 3;
const PACKAGES_ONLY_FORMAT_VERSION: u32 = 2;
/// Oldest lock format still readable. Version 2 only added the optional
/// `groups` field, and a package without it is a runtime package, so a
/// version 1 file means the same thing under version 2.
const MIN_LOCK_FORMAT_VERSION: u32 = 1;

/// What a locked package is needed for. Unknown names in a lock are
/// rejected, so a typo fails loudly instead of being ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LockGroup {
    /// Loaded by the module's code at run time.
    Runtime,
    /// A `[compiled-code].build_deps` package, only needed to compile.
    Build,
    /// An `[extras]` package, only needed for tests and dev tooling.
    Extras,
}

impl LockGroup {
    pub fn as_str(self) -> &'static str {
        match self {
            LockGroup::Runtime => "runtime",
            LockGroup::Build => "build",
            LockGroup::Extras => "extras",
        }
    }
}

/// One package pinned to the exact version and repo baler resolved it
/// to the last time the lock was written. R packages only for now —
/// module deps have no automatic resolve+install path yet (execute_plan
/// only reports whether a module is already installed), so there is
/// nothing concrete to pin for them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub repo: String,
    /// Absent means runtime only, the common case, so it stays out of
    /// the file. A package needed by more than one group lists all of
    /// them, `runtime` included.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<LockGroup>,
}

impl LockedPackage {
    /// True when a normal install needs this package.
    pub fn is_runtime(&self) -> bool {
        self.groups.is_empty() || self.groups.contains(&LockGroup::Runtime)
    }

    /// True when `group` needs this package. An entry with no `groups`
    /// line is a runtime package.
    pub fn in_group(&self, group: LockGroup) -> bool {
        if group == LockGroup::Runtime {
            self.is_runtime()
        } else {
            self.groups.contains(&group)
        }
    }
}

/// One box-bale module, pinned to the code it was locked at. Exactly one of
/// `git` and `url` is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedModule {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// The `branch`, `tag` or `rev` the manifest declared, as written.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_dir: Option<String>,
    /// The commit `ref` pointed at when the lock was written. Installs fetch
    /// this commit, so a branch or tag that moved does not change what is
    /// installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl LockedModule {
    pub fn new(name: &str, version: &Version, source: &ModuleSource, rev: Option<String>) -> Self {
        let (git, git_ref, module_dir, url) = match source {
            ModuleSource::Git { url, git_ref, module_dir } => {
                (Some(url.clone()), git_ref.clone(), module_dir.clone(), None)
            }
            ModuleSource::Url(url) => (None, None, None, Some(url.clone())),
        };
        Self { name: name.to_owned(), version: version.to_string(), git, git_ref, module_dir, rev, url }
    }

    /// The source `baler.toml` declared when the lock was written.
    pub fn declared_source(&self) -> Result<ModuleSource> {
        match (&self.git, &self.url) {
            (Some(git), None) => Ok(ModuleSource::Git {
                url: git.clone(),
                git_ref: self.git_ref.clone(),
                module_dir: self.module_dir.clone(),
            }),
            (None, Some(url)) => Ok(ModuleSource::Url(url.clone())),
            _ => bail!(
                "baler.lock entry for module '{}' must have exactly one of `git` and `url`",
                self.name
            ),
        }
    }

    /// The source to fetch: the pinned commit when there is one.
    pub fn pinned_source(&self) -> Result<ModuleSource> {
        Ok(match self.declared_source()? {
            ModuleSource::Git { url, git_ref, module_dir } => {
                ModuleSource::Git { url, git_ref: self.rev.clone().or(git_ref), module_dir }
            }
            other => other,
        })
    }

    /// True when `declared` is the source this entry was locked from.
    pub fn matches(&self, declared: &ModuleSource) -> bool {
        self.declared_source().map(|own| &own == declared).unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BalerLock {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub r_version: Option<String>,
    #[serde(default, rename = "package")]
    pub packages: Vec<LockedPackage>,
    #[serde(default, rename = "module", skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<LockedModule>,
}

fn default_version() -> u32 {
    PACKAGES_ONLY_FORMAT_VERSION
}

impl BalerLock {
    /// Build a lock from an already-resolved package set — used when
    /// reconstructing a lock from a bundle's embedded manifest, where
    /// there is no baler.lock file on disk to read.
    pub fn from_packages(packages: Vec<LockedPackage>) -> Self {
        Self {
            version: PACKAGES_ONLY_FORMAT_VERSION,
            r_version: None,
            packages,
            modules: Vec::new(),
        }
    }

    /// The same lock with these module entries. A lock that holds modules is
    /// format 3, one that holds none stays format 2.
    pub fn with_modules(mut self, mut modules: Vec<LockedModule>) -> Self {
        modules.sort_by(|a, b| a.name.cmp(&b.name));
        self.version = if modules.is_empty() {
            PACKAGES_ONLY_FORMAT_VERSION
        } else {
            LOCK_FORMAT_VERSION
        };
        self.modules = modules;
        self
    }

    pub fn locked_module(&self, name: &str) -> Option<&LockedModule> {
        self.modules.iter().find(|m| m.name == name)
    }

    /// Look up a locked package by name and return its exact version,
    /// already parsed. A malformed entry is a hard error rather than a
    /// silent fall-through to fresh resolution — a lock the tool can't
    /// trust is worse than no lock at all, since it would look
    /// authoritative while quietly not being enforced.
    pub fn locked_version(&self, name: &str) -> Result<Option<(Version, String)>> {
        let Some(p) = self.packages.iter().find(|p| p.name == name) else {
            return Ok(None);
        };
        let v = parse_pin_version(&p.name, &p.version)?;
        Ok(Some((v, p.repo.clone())))
    }

    /// Every pin in the lock, parsed, whatever group needs it. Used to
    /// seed re-resolution so a package the manifest still reaches keeps
    /// the version the lock gave it.
    pub fn pins(&self) -> Result<HashMap<String, (Version, String)>> {
        let mut pins = HashMap::with_capacity(self.packages.len());
        for p in &self.packages {
            let v = parse_pin_version(&p.name, &p.version)?;
            pins.insert(p.name.clone(), (v, p.repo.clone()));
        }
        Ok(pins)
    }

    /// A copy of this lock with the named packages removed, so the next
    /// resolution treats them as unpinned and picks them fresh while
    /// every other pin stays where it is.
    pub fn without(&self, names: &[String]) -> BalerLock {
        BalerLock {
            version: self.version,
            r_version: self.r_version.clone(),
            packages: self
                .packages
                .iter()
                .filter(|p| !names.iter().any(|n| n == &p.name))
                .cloned()
                .collect(),
            modules: self
                .modules
                .iter()
                .filter(|m| !names.iter().any(|n| n == &m.name))
                .cloned()
                .collect(),
        }
    }

    /// True when both locks pin the same things, ignoring format version,
    /// package order, and whether a runtime-only package spells out
    /// `groups = ["runtime"]`. A lock written by an older baler that
    /// means the same thing as a freshly resolved one is not a change,
    /// and rewriting it would only add noise to a diff.
    pub fn same_content(&self, other: &BalerLock) -> bool {
        self.r_version == other.r_version
            && self.canonical() == other.canonical()
            && self.canonical_modules() == other.canonical_modules()
    }

    fn canonical_modules(&self) -> Vec<LockedModule> {
        let mut modules = self.modules.clone();
        modules.sort_by(|a, b| a.name.cmp(&b.name));
        modules
    }

    fn canonical(&self) -> Vec<LockedPackage> {
        let mut packages: Vec<LockedPackage> = self
            .packages
            .iter()
            .map(|p| LockedPackage { groups: normalize_groups(&p.groups), ..p.clone() })
            .collect();
        packages.sort_by(|a, b| a.name.cmp(&b.name));
        packages
    }

    /// Human-readable list of what differs between this lock and `newer`,
    /// one line per package: `+` added, `-` removed, `~` changed.
    pub fn changes(&self, newer: &BalerLock) -> Vec<String> {
        let old: BTreeMap<String, LockedPackage> =
            self.canonical().into_iter().map(|p| (p.name.clone(), p)).collect();
        let new: BTreeMap<String, LockedPackage> =
            newer.canonical().into_iter().map(|p| (p.name.clone(), p)).collect();
        let names: BTreeSet<&String> = old.keys().chain(new.keys()).collect();

        let mut lines = Vec::new();
        for name in names {
            match (old.get(name), new.get(name)) {
                (None, Some(n)) => lines.push(format!("+ {} {}", name, n.version)),
                (Some(o), None) => lines.push(format!("- {} {}", name, o.version)),
                (Some(o), Some(n)) => {
                    if o.version != n.version {
                        lines.push(format!("~ {} {} -> {}", name, o.version, n.version));
                    } else if o.repo != n.repo {
                        lines.push(format!("~ {} repo {} -> {}", name, o.repo, n.repo));
                    } else if o.groups != n.groups {
                        lines.push(format!("~ {} dependency groups changed", name));
                    }
                }
                (None, None) => {}
            }
        }
        let old_modules: BTreeMap<&str, &LockedModule> =
            self.modules.iter().map(|m| (m.name.as_str(), m)).collect();
        let new_modules: BTreeMap<&str, &LockedModule> =
            newer.modules.iter().map(|m| (m.name.as_str(), m)).collect();
        let module_names: BTreeSet<&str> =
            old_modules.keys().chain(new_modules.keys()).copied().collect();
        for name in module_names {
            match (old_modules.get(name), new_modules.get(name)) {
                (None, Some(n)) => lines.push(format!("+ module {} {}", name, n.version)),
                (Some(o), None) => lines.push(format!("- module {} {}", name, o.version)),
                (Some(o), Some(n)) => {
                    if o.version != n.version {
                        lines.push(format!("~ module {} {} -> {}", name, o.version, n.version));
                    } else if o.rev != n.rev {
                        lines.push(format!(
                            "~ module {} moved from {} to {}",
                            name,
                            o.rev.as_deref().unwrap_or("(none)"),
                            n.rev.as_deref().unwrap_or("(none)")
                        ));
                    } else if o != n {
                        lines.push(format!("~ module {} source changed", name));
                    }
                }
                (None, None) => {}
            }
        }
        if self.r_version != newer.r_version {
            lines.push(format!(
                "~ r_version {} -> {}",
                self.r_version.as_deref().unwrap_or("(none)"),
                newer.r_version.as_deref().unwrap_or("(none)")
            ));
        }
        lines
    }
}

fn parse_pin_version(name: &str, version: &str) -> Result<Version> {
    Version::parse(version).with_context(|| {
        format!(
            "baler.lock has an invalid version for '{}': '{}'",
            name, version
        )
    })
}

/// Parse lock file contents and check the format version. `origin` names
/// where the text came from (a path, or an archive) so errors can say so.
/// Shared by `read` and by the reader for a lock embedded in a bundle, so
/// a lock from either place is held to the same rules.
pub fn parse(contents: &str, origin: &str) -> Result<BalerLock> {
    /// Only the format version, read leniently, so a lock written by a newer
    /// baler gets an upgrade message and not an "unknown field" error.
    #[derive(Deserialize)]
    struct Header {
        #[serde(default)]
        version: Option<u32>,
    }
    let header: Header =
        toml::from_str(contents).with_context(|| format!("Failed to parse {}", origin))?;
    if let Some(version) = header.version {
        check_format_version(version, origin)?;
    }

    let lock: BalerLock =
        toml::from_str(contents).with_context(|| format!("Failed to parse {}", origin))?;
    check_format_version(lock.version, origin)?;
    Ok(lock)
}

fn check_format_version(version: u32, origin: &str) -> Result<()> {
    if version < MIN_LOCK_FORMAT_VERSION || version > LOCK_FORMAT_VERSION {
        bail!(
            "{} declares lock format version {}, but this build of baler \
             only understands versions {} to {}. Regenerate it with `baler lock`. \
             If it was written by a newer baler, update baler instead.",
            origin,
            version,
            MIN_LOCK_FORMAT_VERSION,
            LOCK_FORMAT_VERSION
        );
    }
    Ok(())
}

/// Read `baler.lock` from `project_root`. `Ok(None)` means the file
/// doesn't exist — its presence is the only opt-in switch, so a missing
/// lock is not an error, it just means baler resolves fresh the way
/// it always has. An existing-but-unparseable lock IS an error: silently
/// ignoring a broken lock would defeat the reason it exists.
pub fn read(project_root: &Path) -> Result<Option<BalerLock>> {
    let path = project_root.join(LOCK_FILE_NAME);
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    parse(&contents, &path.display().to_string()).map(Some)
}

/// Write `baler.lock` from a resolved set of `{name: (version, repo)}`,
/// sorted by package name for a stable, diffable file — an unsorted lock
/// would produce noisy diffs on every write even when nothing about the
/// resolved graph actually changed.
///
/// `r_version` is `None` by default (see `--with-rver` on
/// `baler lock`): the field is provenance-only, never enforced (the
/// real constraint is `baler.toml`'s `module.r_version`, checked
/// separately), and two contributors on different R installs writing
/// otherwise-identical locks would produce a spurious diff if this were
/// recorded unconditionally.
pub fn write(
    project_root: &Path,
    resolved: &BTreeMap<String, (Version, String)>,
    r_version: Option<&str>,
) -> Result<()> {
    let pinned = resolved
        .iter()
        .map(|(name, (version, repo))| {
            let pin = PinnedPackage {
                version: version.clone(),
                repo: repo.clone(),
                groups: Vec::new(),
            };
            (name.clone(), pin)
        })
        .collect();
    write_grouped(project_root, &pinned, r_version)
}

/// One package ready to be written by `write_grouped`.
pub struct PinnedPackage {
    pub version: Version,
    pub repo: String,
    pub groups: Vec<LockGroup>,
}

/// Build the lock for a resolved set, without touching the disk. Packages
/// are sorted by name, and a package whose only group is `runtime` gets no
/// `groups` line. Split out of `write_grouped` so a caller can compare the
/// result with the lock already on disk and skip the write when nothing
/// changed.
pub fn build_grouped(
    pinned: &BTreeMap<String, PinnedPackage>,
    r_version: Option<&str>,
) -> BalerLock {
    let mut packages: Vec<LockedPackage> = pinned
        .iter()
        .map(|(name, pin)| LockedPackage {
            name: name.clone(),
            version: pin.version.to_string(),
            repo: pin.repo.clone(),
            groups: normalize_groups(&pin.groups),
        })
        .collect();
    packages.sort_by(|a, b| a.name.cmp(&b.name));

    BalerLock {
        version: PACKAGES_ONLY_FORMAT_VERSION,
        r_version: r_version.map(str::to_owned),
        packages,
        modules: Vec::new(),
    }
}

/// Serialize `lock` to `baler.lock` in `project_root`, through a temp file
/// and a rename so a crash never leaves a half-written lock behind.
pub fn write_lock(project_root: &Path, lock: &BalerLock) -> Result<()> {
    let contents = toml::to_string_pretty(lock).context("Failed to serialize baler.lock")?;

    let path = project_root.join(LOCK_FILE_NAME);
    let tmp_path = path.with_extension("lock.tmp");
    std::fs::write(&tmp_path, &contents)
        .with_context(|| format!("Failed to write {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &path)
        .with_context(|| format!("Failed to finalize {}", path.display()))?;
    Ok(())
}

/// Same as `write`, but each package also records the groups that need
/// it. A package whose only group is `runtime` is written without a
/// `groups` line. Always writes; `baler lock` uses `build_grouped` and
/// `write_lock` directly so it can skip an unchanged lock.
pub fn write_grouped(
    project_root: &Path,
    pinned: &BTreeMap<String, PinnedPackage>,
    r_version: Option<&str>,
) -> Result<()> {
    write_lock(project_root, &build_grouped(pinned, r_version))
}

fn normalize_groups(groups: &[LockGroup]) -> Vec<LockGroup> {
    let mut sorted = groups.to_vec();
    sorted.sort();
    sorted.dedup();
    if sorted == [LockGroup::Runtime] {
        Vec::new()
    } else {
        sorted
    }
}
