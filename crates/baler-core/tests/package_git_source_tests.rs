//! The GitHub source id, the DESCRIPTION it reads and writes, and how the
//! solver plans a GitHub package, all without a network.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use anyhow::{bail, Context, Result};
use baler_core::baler_toml::DEFAULT_CRAN_MIRROR;
use baler_core::cran::client::git::{
    index_from_description, installed_origin_matches, is_git_id, pinned_source, remote_sha,
    same_source, with_remote_fields, GitId,
};
use baler_core::cran::client::{plan_packages, Planned};
use baler_core::cran::packages::{PackageRecord, RVersion};
use baler_core::cran::universe::{Index, Sources};
use baler_core::lockfile::{BalerLock, LockedPackage};
use baler_core::ops::resolve::ResolvedPackage;

const URL: &str = "https://github.com/eddelbuettel/bh";
const SHA1: &str = "1111111111111111111111111111111111111111";
const SHA2: &str = "2222222222222222222222222222222222222222";

fn id(text: &str) -> GitId {
    GitId::parse(text).unwrap()
}

// ---- the id ----

#[test]
fn an_id_round_trips_with_and_without_a_ref_and_a_commit() {
    for text in [
        format!("git+{URL}"),
        format!("git+{URL}@master"),
        format!("git+{URL}#{SHA1}"),
        format!("git+{URL}@master#{SHA1}"),
        format!("git+{URL}@feature/x#{SHA1}"),
    ] {
        assert_eq!(id(&text).to_string(), text);
    }
}

#[test]
fn a_ref_may_hold_slashes_and_still_parse_apart_from_the_url() {
    let parsed = id(&format!("git+{URL}@feature/x#{SHA1}"));
    assert_eq!(parsed.url, URL);
    assert_eq!(parsed.git_ref.as_deref(), Some("feature/x"));
    assert_eq!(parsed.sha.as_deref(), Some(SHA1));
}

#[test]
fn anything_that_is_not_an_id_is_refused() {
    assert!(GitId::parse(DEFAULT_CRAN_MIRROR).is_err());
    assert!(GitId::parse("git+not-a-url").is_err());
    assert!(GitId::parse(&format!("git+{URL}@")).is_err());
    assert!(GitId::parse(&format!("git+{URL}#")).is_err());
    assert!(!is_git_id(DEFAULT_CRAN_MIRROR));
    assert!(is_git_id(&format!("git+{URL}")));
}

#[test]
fn only_github_owner_and_repo_are_accepted() {
    assert_eq!(id(&format!("git+{URL}")).owner_repo().unwrap(), ("eddelbuettel".into(), "bh".into()));
    let other = id("git+https://gitlab.com/a/b").owner_repo().unwrap_err().to_string();
    assert!(other.contains("github.com only") && other.contains("`repo`"), "{other}");
    let deep = id(&format!("git+{URL}/tree/main")).owner_repo().unwrap_err().to_string();
    assert!(deep.contains("https://github.com/<owner>/<repo>"), "{deep}");
}

// ---- same source ----

#[test]
fn a_declared_id_matches_its_lock_entry_whatever_commit_the_lock_holds() {
    let declared = format!("git+{URL}");
    assert!(same_source(&declared, &format!("git+{URL}#{SHA1}")));
    assert!(same_source(&declared, &format!("git+{URL}#{SHA2}")));
}

#[test]
fn a_different_ref_or_repository_is_another_source() {
    let locked = format!("git+{URL}@master#{SHA1}");
    assert!(same_source(&format!("git+{URL}@master"), &locked));
    assert!(!same_source(&format!("git+{URL}@devel"), &locked));
    assert!(!same_source(&format!("git+{URL}"), &locked));
    assert!(!same_source("git+https://github.com/other/bh", &locked));
}

#[test]
fn a_declared_commit_has_to_agree_with_the_locked_one_abbreviated_or_not() {
    let locked = format!("git+{URL}#{SHA1}");
    assert!(same_source(&format!("git+{URL}#1111111"), &locked));
    assert!(!same_source(&format!("git+{URL}#2222222"), &locked));
    assert!(!same_source(&format!("git+{URL}#{SHA1}"), &format!("git+{URL}")));
}

#[test]
fn the_url_is_compared_without_regard_to_case_and_a_repository_is_compared_exactly() {
    assert!(same_source(
        "git+https://github.com/EddelBuettel/BH",
        &format!("git+{URL}#{SHA1}")
    ));
    assert!(same_source(DEFAULT_CRAN_MIRROR, DEFAULT_CRAN_MIRROR));
    assert!(!same_source(DEFAULT_CRAN_MIRROR, &format!("git+{URL}#{SHA1}")));
    assert!(!same_source(&format!("git+{URL}"), DEFAULT_CRAN_MIRROR));
}

#[test]
fn only_a_matching_lock_entry_with_a_commit_pins_the_source() {
    let declared = format!("git+{URL}");
    let locked = format!("git+{URL}#{SHA1}");
    assert_eq!(pinned_source(&declared, &locked), Some(locked.clone()));
    assert_eq!(pinned_source(&declared, &format!("git+{URL}")), None);
    assert_eq!(pinned_source(&format!("git+{URL}@devel"), &locked), None);
    assert_eq!(pinned_source(DEFAULT_CRAN_MIRROR, DEFAULT_CRAN_MIRROR), None);
}

// ---- DESCRIPTION ----

const DESCRIPTION: &str = "Package: BH\n\
Type: Package\n\
Title: Boost C++ Header Files\n\
Version: 1.87.0-1\n\
Authors@R: c(person(\"Dirk\", \"Eddelbuettel\", role = \"aut\"),\n\
    person(\"John\", \"Doe\", role = \"ctb\"))\n\
Depends: R (>= 3.4.0)\n\
Imports: cli (>= 3.0)\n\
LinkingTo: Rcpp\n\
License: BSL-1.0\n";

#[test]
fn the_index_of_a_github_package_is_its_description_with_the_commit_as_source() {
    let source = id(&format!("git+{URL}@master#{SHA1}"));
    let index = index_from_description(DESCRIPTION, &source).unwrap();
    let record = &index["BH"];
    assert_eq!(record.version.as_str(), "1.87.0-1");
    let deps: Vec<&str> = record.deps.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(deps, ["cli", "Rcpp"]);
    assert_eq!(record.source.as_deref(), Some(source.to_string().as_str()));
}

#[test]
fn a_description_without_a_package_is_refused() {
    let source = id(&format!("git+{URL}#{SHA1}"));
    assert!(index_from_description("Title: nothing\n", &source).is_err());
}

#[test]
fn the_installed_description_records_where_the_package_came_from() {
    let source = id(&format!("git+{URL}@master#{SHA1}"));
    let text = with_remote_fields(DESCRIPTION, &source).unwrap();
    assert!(text.contains("RemoteType: github\n"), "{text}");
    assert!(text.contains("RemoteRepo: bh\n"), "{text}");
    assert!(text.contains("RemoteUsername: eddelbuettel\n"), "{text}");
    assert!(text.contains("RemoteRef: master\n"), "{text}");
    assert_eq!(remote_sha(&text).as_deref(), Some(SHA1));
    assert!(text.contains("Package: BH\n") && text.contains("Version: 1.87.0-1\n"));
}

#[test]
fn recording_the_origin_twice_replaces_it_instead_of_stacking_it() {
    let first = with_remote_fields(DESCRIPTION, &id(&format!("git+{URL}#{SHA1}"))).unwrap();
    let second = with_remote_fields(&first, &id(&format!("git+{URL}#{SHA2}"))).unwrap();
    assert_eq!(second.matches("RemoteSha:").count(), 1);
    assert_eq!(remote_sha(&second).as_deref(), Some(SHA2));
}

#[test]
fn an_id_without_a_commit_has_nothing_to_record() {
    assert!(with_remote_fields(DESCRIPTION, &id(&format!("git+{URL}"))).is_err());
}

#[test]
fn an_installed_package_matches_only_the_commit_it_was_built_from() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("DESCRIPTION");
    let cran = DESCRIPTION.to_string();
    let github = with_remote_fields(DESCRIPTION, &id(&format!("git+{URL}#{SHA1}"))).unwrap();

    std::fs::write(&path, &github).unwrap();
    assert!(installed_origin_matches(&path, &format!("git+{URL}#{SHA1}")));
    assert!(!installed_origin_matches(&path, &format!("git+{URL}#{SHA2}")));
    assert!(!installed_origin_matches(&path, DEFAULT_CRAN_MIRROR));

    std::fs::write(&path, &cran).unwrap();
    assert!(installed_origin_matches(&path, DEFAULT_CRAN_MIRROR));
    assert!(!installed_origin_matches(&path, &format!("git+{URL}#{SHA1}")));
    assert!(!installed_origin_matches(&dir.path().join("missing"), DEFAULT_CRAN_MIRROR));
}

// ---- planning ----

/// A pretend network. `github` maps a source id, as the solver asks for it,
/// to the DESCRIPTION and commit the branch points at.
struct Net {
    cran: Index,
    github: HashMap<String, (String, String)>,
    asked: RefCell<Vec<String>>,
}

impl Net {
    fn new() -> Self {
        Self { cran: Index::new(), github: HashMap::new(), asked: RefCell::default() }
    }

    fn cran(mut self, name: &str, version: &str) -> Self {
        let record = PackageRecord {
            version: RVersion::parse(version).unwrap(),
            deps: Vec::new(),
            source: None,
        };
        self.cran.insert(name.to_string(), record);
        self
    }

    /// `source` is what is asked for, `commit` what it resolves to.
    fn github(mut self, source: &str, commit: &str, description: &str) -> Self {
        self.github.insert(source.to_string(), (commit.to_string(), description.to_string()));
        self
    }

    fn run(
        &self,
        packages: &[(&str, &str, &str)],
        lock: Option<&BalerLock>,
        relock: bool,
    ) -> Result<Vec<Planned>> {
        let index = |repo: &str| -> Result<Index> {
            self.asked.borrow_mut().push(repo.to_owned());
            if !is_git_id(repo) {
                return Ok(self.cran.clone());
            }
            let (commit, description) =
                self.github.get(repo).with_context(|| format!("no such source {repo}"))?;
            let resolved = GitId::parse(repo)?.with_sha(commit);
            index_from_description(description, &resolved)
        };
        let versions = |_: &str, _: &str| Ok(Vec::new());
        let deps = |_: &str, name: &str, _: &RVersion| -> Result<Vec<(String, String)>> {
            bail!("unexpected archive read of {name}")
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

    fn asked_for(&self, source: &str) -> bool {
        self.asked.borrow().iter().any(|asked| asked == source)
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

fn bh_description(version: &str) -> String {
    format!("Package: BH\nVersion: {version}\n")
}

fn summary(planned: &[Planned]) -> Vec<String> {
    planned
        .iter()
        .map(|p| format!("{} {} {}", p.name, p.version.as_str(), p.repo))
        .collect()
}

fn message(error: &anyhow::Error) -> String {
    format!("{error:#}")
}

#[test]
fn a_fresh_resolution_plans_the_commit_the_branch_points_at() {
    let declared = format!("git+{URL}");
    let net = Net::new().github(&declared, SHA1, &bh_description("1.87.0-1"));
    let planned = net.run(&[("BH", "*", &declared)], None, false).unwrap();
    assert_eq!(summary(&planned), [format!("BH 1.87.0-1 git+{URL}#{SHA1}")]);
}

#[test]
fn a_locked_install_fetches_the_pinned_commit_and_never_asks_for_the_branch() {
    let declared = format!("git+{URL}");
    let pinned = format!("git+{URL}#{SHA1}");
    let net = Net::new()
        .github(&declared, SHA2, &bh_description("1.90.0-1"))
        .github(&pinned, SHA1, &bh_description("1.87.0-1"));
    let lock = lock_of(&[("BH", "1.87.0-1", &pinned)]);
    let planned = net.run(&[("BH", "*", &declared)], Some(&lock), false).unwrap();
    assert_eq!(summary(&planned), [format!("BH 1.87.0-1 {pinned}")]);
    assert!(!net.asked_for(&declared));
}

#[test]
fn relocking_keeps_the_pinned_commit_even_when_the_branch_moved() {
    let declared = format!("git+{URL}");
    let pinned = format!("git+{URL}#{SHA1}");
    let net = Net::new()
        .github(&declared, SHA2, &bh_description("1.90.0-1"))
        .github(&pinned, SHA1, &bh_description("1.87.0-1"));
    let lock = lock_of(&[("BH", "1.87.0-1", &pinned)]);
    let planned = net.run(&[("BH", "*", &declared)], Some(&lock), true).unwrap();
    assert_eq!(summary(&planned), [format!("BH 1.87.0-1 {pinned}")]);
}

#[test]
fn updating_one_package_moves_it_to_the_new_commit() {
    // `baler lock --update BH` drops the pin first, so the lock is empty.
    let declared = format!("git+{URL}");
    let net = Net::new().github(&declared, SHA2, &bh_description("1.90.0-1"));
    let lock = lock_of(&[]);
    let planned = net.run(&[("BH", "*", &declared)], Some(&lock), true).unwrap();
    assert_eq!(summary(&planned), [format!("BH 1.90.0-1 git+{URL}#{SHA2}")]);
}

#[test]
fn a_pin_for_another_branch_is_an_error_when_installing_and_moves_when_relocking() {
    let locked = format!("git+{URL}@master#{SHA1}");
    let declared = format!("git+{URL}@devel");
    let net = Net::new().github(&declared, SHA2, &bh_description("1.90.0-1"));
    let lock = lock_of(&[("BH", "1.87.0-1", &locked)]);

    let error = net.run(&[("BH", "*", &declared)], Some(&lock), false).err().unwrap();
    assert!(message(&error).contains("pins 'BH' to repo"), "{}", message(&error));

    let planned = net.run(&[("BH", "*", &declared)], Some(&lock), true).unwrap();
    assert_eq!(summary(&planned), [format!("BH 1.90.0-1 git+{URL}@devel#{SHA2}")]);
}

#[test]
fn a_github_package_moving_to_cran_is_an_error_when_installing() {
    let pinned = format!("git+{URL}#{SHA1}");
    let net = Net::new().cran("BH", "1.87.0-1");
    let lock = lock_of(&[("BH", "1.87.0-1", &pinned)]);
    let error = net.run(&[("BH", "*", DEFAULT_CRAN_MIRROR)], Some(&lock), false).err().unwrap();
    assert!(message(&error).contains("pins 'BH' to repo"), "{}", message(&error));
}

#[test]
fn the_constraint_of_the_declaration_is_checked_against_the_description() {
    let declared = format!("git+{URL}");
    let net = Net::new().github(&declared, SHA1, &bh_description("1.87.0-1"));
    assert!(net.run(&[("BH", ">= 1.80", &declared)], None, false).is_ok());
    let error = net.run(&[("BH", ">= 2.0", &declared)], None, false).err().unwrap();
    assert!(message(&error).contains("BH"), "{}", message(&error));
}

#[test]
fn a_name_that_is_not_the_package_in_the_repository_is_named_in_the_error() {
    let declared = format!("git+{URL}");
    let net = Net::new().github(&declared, SHA1, "Package: other\nVersion: 1.0.0\n");
    let error = net.run(&[("BH", "*", &declared)], None, false).err().unwrap();
    let text = message(&error);
    assert!(text.contains("'BH'") && text.contains("DESCRIPTION"), "{text}");
}

#[test]
fn the_dependencies_of_a_github_package_come_from_cran() {
    let declared = format!("git+{URL}");
    let net = Net::new()
        .cran("cli", "3.6.2")
        .github(&declared, SHA1, "Package: BH\nVersion: 1.87.0-1\nImports: cli (>= 3.0)\n");
    let planned = net.run(&[("BH", "*", &declared)], None, false).unwrap();
    assert_eq!(
        summary(&planned),
        [
            format!("cli 3.6.2 {DEFAULT_CRAN_MIRROR}"),
            format!("BH 1.87.0-1 git+{URL}#{SHA1}"),
        ]
    );
}

#[test]
fn a_github_source_is_never_searched_for_a_package_nobody_declared_from_it() {
    let declared = format!("git+{URL}");
    let net = Net::new()
        .cran("cli", "3.6.2")
        .github(&declared, SHA1, &bh_description("1.87.0-1"));
    net.run(&[("cli", "*", DEFAULT_CRAN_MIRROR), ("BH", "*", &declared)], None, false).unwrap();
    // cli is declared, so the only lookups are the CRAN index and the one
    // GitHub source, the latter exactly once.
    let github_lookups = net.asked.borrow().iter().filter(|s| is_git_id(s)).count();
    assert_eq!(github_lookups, 1);
}
