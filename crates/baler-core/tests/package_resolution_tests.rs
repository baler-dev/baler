use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};

use anyhow::{bail, Context, Result};
use baler_core::baler_toml::DEFAULT_CRAN_MIRROR;
use baler_core::cran::client::{plan_packages, Planned};
use baler_core::cran::packages::{PackageRecord, RVersion};
use baler_core::cran::universe::{Index, Sources};
use baler_core::lockfile::{BalerLock, LockedPackage};
use baler_core::ops::resolve::ResolvedPackage;

const CRAN: &str = DEFAULT_CRAN_MIRROR;
const OTHER: &str = "https://other.example.org";

type Entry<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);
type Archived = Vec<(&'static str, Vec<(&'static str, &'static str)>)>;

/// A pretend network: package indexes and archives.
struct Net {
    indexes: HashMap<String, Index>,
    archive: HashMap<(String, String), Archived>,
    listings: Cell<usize>,
}

impl Net {
    fn new() -> Self {
        Self { indexes: HashMap::new(), archive: HashMap::new(), listings: Cell::new(0) }
    }

    fn repo(mut self, repo: &str, entries: &[Entry]) -> Self {
        let index = entries
            .iter()
            .map(|(name, version, deps)| {
                let record = PackageRecord {
                    version: RVersion::parse(version).unwrap(),
                    deps: deps.iter().map(|(d, c)| (d.to_string(), c.to_string())).collect(),
                };
                (name.to_string(), record)
            })
            .collect();
        self.indexes.insert(repo.to_owned(), index);
        self
    }

    fn archived(mut self, repo: &str, name: &str, releases: Archived) -> Self {
        self.archive.insert((repo.to_owned(), name.to_owned()), releases);
        self
    }

    fn run(
        &self,
        packages: &[(&str, &str, &str)],
        lock: Option<&BalerLock>,
        relock: bool,
    ) -> Result<Vec<Planned>> {
        let index = |repo: &str| self.indexes.get(repo).cloned().context("no such repo");
        let versions = |repo: &str, name: &str| {
            self.listings.set(self.listings.get() + 1);
            let releases = self.archive.get(&(repo.to_owned(), name.to_owned()));
            releases
                .map(|all| all.iter().map(|(raw, _)| RVersion::parse(raw)).collect())
                .unwrap_or_else(|| Ok(Vec::new()))
        };
        let deps = |repo: &str, name: &str, release: &RVersion| {
            let all = self.archive.get(&(repo.to_owned(), name.to_owned()));
            match all.and_then(|all| all.iter().find(|(raw, _)| *raw == release.as_str())) {
                Some((_, deps)) => Ok(deps
                    .iter()
                    .map(|(d, c)| (d.to_string(), c.to_string()))
                    .collect()),
                None => bail!("unexpected archive read of {name} {release}"),
            }
        };
        let sources =
            Sources { index: &index, archive_versions: &versions, archive_deps: &deps };
        let packages: BTreeMap<String, ResolvedPackage> = packages
            .iter()
            .map(|(name, spec, repo)| {
                (
                    name.to_string(),
                    ResolvedPackage { version_spec: spec.to_string(), repo: repo.to_string() },
                )
            })
            .collect();
        plan_packages(&packages, lock, relock, &sources)
    }
}

fn lock_of(pins: &[(&str, &str, &str)]) -> BalerLock {
    BalerLock::from_packages(
        pins.iter()
            .map(|(name, version, repo)| LockedPackage {
                name: name.to_string(),
                version: version.to_string(),
                repo: repo.to_string(),
                groups: Vec::new(),
            })
            .collect(),
    )
}

fn summary(planned: &[Planned]) -> Vec<String> {
    planned.iter().map(|p| format!("{} {}", p.name, p.version.as_str())).collect()
}

fn message(error: &anyhow::Error) -> String {
    format!("{error:#}")
}

#[test]
fn a_fresh_resolution_lists_dependencies_first() {
    let net = Net::new().repo(
        CRAN,
        &[("a", "1.0.0", &[("cli", ">= 3.0")]), ("cli", "3.6.2", &[])],
    );
    let planned = net.run(&[("a", "*", CRAN)], None, false).unwrap();
    assert_eq!(summary(&planned), ["cli 3.6.2", "a 1.0.0"]);
}

#[test]
fn the_solver_backs_up_to_an_archived_release_when_the_newest_conflicts() {
    let net = Net::new()
        .repo(
            CRAN,
            &[
                ("a", "2.0.0", &[("cli", ">= 3.0")]),
                ("b", "1.0.0", &[("cli", "< 3.0")]),
                ("cli", "3.6.2", &[]),
            ],
        )
        .archived(CRAN, "a", vec![("1.0.0", vec![])])
        .archived(CRAN, "cli", vec![("2.9.0", vec![])]);
    let planned = net.run(&[("a", "*", CRAN), ("b", "*", CRAN)], None, false).unwrap();
    let versions = summary(&planned);
    assert!(versions.contains(&"a 1.0.0".to_owned()), "{versions:?}");
    assert!(versions.contains(&"cli 2.9.0".to_owned()), "{versions:?}");
}

#[test]
fn a_locked_install_needs_no_archive_and_recovers_the_published_spelling() {
    // The lock lost the dash (0.3.1), the repository publishes 0.3-1.
    let net = Net::new().repo(CRAN, &[("pkg", "0.3-1", &[])]);
    let lock = lock_of(&[("pkg", "0.3.1", CRAN)]);
    let planned = net.run(&[("pkg", "*", CRAN)], Some(&lock), false).unwrap();
    assert_eq!(summary(&planned), ["pkg 0.3-1"]);
    assert_eq!(net.listings.get(), 0);
}

#[test]
fn a_locked_install_orders_pinned_packages_by_their_dependencies() {
    let net = Net::new().repo(
        CRAN,
        &[("a", "1.0.0", &[("b", "*")]), ("b", "1.0.0", &[("c", "*")]), ("c", "1.0.0", &[])],
    );
    let lock = lock_of(&[("a", "1.0.0", CRAN), ("b", "1.0.0", CRAN), ("c", "1.0.0", CRAN)]);
    let packages = [("a", "*", CRAN), ("b", "*", CRAN), ("c", "*", CRAN)];
    let planned = net.run(&packages, Some(&lock), false).unwrap();
    assert_eq!(summary(&planned), ["c 1.0.0", "b 1.0.0", "a 1.0.0"]);
}

#[test]
fn a_stale_pin_is_an_error_when_installing_and_resolved_again_when_relocking() {
    let net = Net::new().repo(CRAN, &[("cli", "3.6.2", &[])]);
    let lock = lock_of(&[("cli", "2.0.0", CRAN)]);
    let packages = [("cli", ">= 3.0", CRAN)];

    let error = net.run(&packages, Some(&lock), false).err().unwrap();
    assert!(message(&error).contains("stale"), "{}", message(&error));

    let planned = net.run(&packages, Some(&lock), true).unwrap();
    assert_eq!(summary(&planned), ["cli 3.6.2"]);
}

#[test]
fn a_pin_for_another_repo_is_an_error_when_installing_and_moves_when_relocking() {
    let net = Net::new()
        .repo(CRAN, &[("cli", "3.6.2", &[])])
        .repo(OTHER, &[("cli", "3.7.0", &[])]);
    let lock = lock_of(&[("cli", "3.6.2", CRAN)]);
    let packages = [("cli", "*", OTHER)];

    let error = net.run(&packages, Some(&lock), false).err().unwrap();
    assert!(message(&error).contains("pins 'cli' to repo"), "{}", message(&error));

    let planned = net.run(&packages, Some(&lock), true).unwrap();
    assert_eq!((planned[0].version.as_str(), planned[0].repo.as_str()), ("3.7.0", OTHER));
}

#[test]
fn a_new_dependency_cannot_move_a_pinned_package() {
    // cli is pinned to 3.5.0 (older than the index's 3.6.2). The new
    // package a is happy with either, so cli must stay where it is.
    let net = Net::new()
        .repo(
            CRAN,
            &[("a", "1.0.0", &[("cli", ">= 3.0")]), ("cli", "3.6.2", &[])],
        )
        .archived(CRAN, "cli", vec![("3.5.0", vec![])]);
    let lock = lock_of(&[("cli", "3.5.0", CRAN)]);
    let planned = net
        .run(&[("a", "*", CRAN), ("cli", "*", CRAN)], Some(&lock), false)
        .unwrap();
    assert_eq!(summary(&planned), ["cli 3.5.0", "a 1.0.0"]);
}

#[test]
fn a_new_dependency_that_needs_a_different_version_than_the_pin_is_a_conflict() {
    let net = Net::new()
        .repo(
            CRAN,
            &[("a", "1.0.0", &[("cli", ">= 3.6")]), ("cli", "3.6.2", &[])],
        )
        .archived(CRAN, "a", vec![])
        .archived(CRAN, "cli", vec![("3.5.0", vec![])]);
    let lock = lock_of(&[("cli", "3.5.0", CRAN)]);
    let error = net
        .run(&[("a", "*", CRAN), ("cli", "*", CRAN)], Some(&lock), false)
        .err()
        .unwrap();
    assert!(message(&error).contains("cli"), "{}", message(&error));
}

#[test]
fn relocking_keeps_a_pin_and_reads_that_releases_own_dependencies() {
    // a 2.0.0 is current and needs cli >= 3, but the lock pins a 1.0.0,
    // which needs cli < 3 and has its own pin at 2.9.0.
    let net = Net::new()
        .repo(
            CRAN,
            &[("a", "2.0.0", &[("cli", ">= 3.0")]), ("cli", "3.6.2", &[])],
        )
        .archived(CRAN, "a", vec![("1.0.0", vec![("cli", "< 3.0")])])
        .archived(CRAN, "cli", vec![("2.9.0", vec![])]);
    let lock = lock_of(&[("a", "1.0.0", CRAN), ("cli", "2.9.0", CRAN)]);
    let planned = net.run(&[("a", "*", CRAN)], Some(&lock), true).unwrap();
    assert_eq!(summary(&planned), ["cli 2.9.0", "a 1.0.0"]);
}

#[test]
fn a_requested_package_that_is_not_in_its_index_is_an_error_naming_the_repo() {
    let net = Net::new().repo(OTHER, &[]);
    let error = net.run(&[("ghost", "*", OTHER)], None, false).err().unwrap();
    let text = message(&error);
    assert!(text.contains("'ghost'") && text.contains(OTHER), "{text}");
}

#[test]
fn packages_that_ship_with_r_are_skipped() {
    let net = Net::new();
    let planned = net.run(&[("stats", "*", CRAN)], None, false).unwrap();
    assert!(planned.is_empty());
}

#[test]
fn an_unreadable_manifest_constraint_names_the_package() {
    let net = Net::new().repo(CRAN, &[("cli", "3.6.2", &[])]);
    let error = net.run(&[("cli", ">= banana", CRAN)], None, false).err().unwrap();
    assert!(message(&error).contains("'cli'"));
}
