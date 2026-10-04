use std::collections::BTreeMap;

use baler_core::lockfile::{self, LockGroup, PinnedPackage};
use semver::Version;
use tempfile::TempDir;

fn pin(version: &str, groups: Vec<LockGroup>) -> PinnedPackage {
    PinnedPackage {
        version: Version::parse(version).unwrap(),
        repo: "https://cloud.r-project.org".to_owned(),
        groups,
    }
}

fn write_one(dir: &TempDir, name: &str, groups: Vec<LockGroup>) {
    let mut pinned = BTreeMap::new();
    pinned.insert(name.to_owned(), pin("1.2.3", groups));
    lockfile::write_grouped(dir.path(), &pinned, None).unwrap();
}

fn lock_text(dir: &TempDir) -> String {
    std::fs::read_to_string(dir.path().join(lockfile::LOCK_FILE_NAME)).unwrap()
}

#[test]
fn runtime_only_package_is_written_without_groups() {
    let dir = TempDir::new().unwrap();
    write_one(&dir, "cli", vec![LockGroup::Runtime]);

    assert!(!lock_text(&dir).contains("groups"));
    let lock = lockfile::read(dir.path()).unwrap().unwrap();
    assert!(lock.packages[0].groups.is_empty());
    assert!(lock.packages[0].is_runtime());
}

#[test]
fn build_and_extras_groups_round_trip() {
    let dir = TempDir::new().unwrap();
    let mut pinned = BTreeMap::new();
    pinned.insert("Rcpp".to_owned(), pin("1.0.12", vec![LockGroup::Build]));
    pinned.insert("testthat".to_owned(), pin("3.2.1", vec![LockGroup::Extras]));
    lockfile::write_grouped(dir.path(), &pinned, None).unwrap();

    let lock = lockfile::read(dir.path()).unwrap().unwrap();
    assert_eq!(lock.packages[0].name, "Rcpp");
    assert_eq!(lock.packages[0].groups, vec![LockGroup::Build]);
    assert!(!lock.packages[0].is_runtime());
    assert_eq!(lock.packages[1].groups, vec![LockGroup::Extras]);
    assert!(!lock.packages[1].is_runtime());
}

#[test]
fn runtime_plus_build_keeps_both_groups_in_order() {
    let dir = TempDir::new().unwrap();
    write_one(&dir, "Rcpp", vec![LockGroup::Build, LockGroup::Runtime, LockGroup::Build]);

    let lock = lockfile::read(dir.path()).unwrap().unwrap();
    assert_eq!(lock.packages[0].groups, vec![LockGroup::Runtime, LockGroup::Build]);
    assert!(lock.packages[0].is_runtime());
}

#[test]
fn plain_write_still_produces_runtime_packages() {
    let dir = TempDir::new().unwrap();
    let mut resolved = BTreeMap::new();
    resolved.insert(
        "cli".to_owned(),
        (Version::parse("3.6.2").unwrap(), "https://cloud.r-project.org".to_owned()),
    );
    lockfile::write(dir.path(), &resolved, None).unwrap();

    let lock = lockfile::read(dir.path()).unwrap().unwrap();
    assert!(lock.packages[0].is_runtime());
    assert!(!lock_text(&dir).contains("groups"));
}

#[test]
fn version_one_lock_without_groups_still_reads() {
    let dir = TempDir::new().unwrap();
    let old = "version = 1\n\n[[package]]\nname = \"cli\"\nversion = \"3.6.2\"\nrepo = \"https://cloud.r-project.org\"\n";
    std::fs::write(dir.path().join(lockfile::LOCK_FILE_NAME), old).unwrap();

    let lock = lockfile::read(dir.path()).unwrap().unwrap();
    assert!(lock.packages[0].is_runtime());
}

#[test]
fn unknown_group_name_is_rejected() {
    let dir = TempDir::new().unwrap();
    let bad = "version = 2\n\n[[package]]\nname = \"cli\"\nversion = \"3.6.2\"\nrepo = \"r\"\ngroups = [\"runtme\"]\n";
    std::fs::write(dir.path().join(lockfile::LOCK_FILE_NAME), bad).unwrap();

    assert!(lockfile::read(dir.path()).is_err());
}

#[test]
fn unsupported_format_version_is_rejected() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(lockfile::LOCK_FILE_NAME), "version = 3\n").unwrap();
    assert!(lockfile::read(dir.path()).is_err());

    std::fs::write(dir.path().join(lockfile::LOCK_FILE_NAME), "version = 0\n").unwrap();
    assert!(lockfile::read(dir.path()).is_err());
}
