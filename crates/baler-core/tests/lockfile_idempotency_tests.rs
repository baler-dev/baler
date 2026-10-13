//! Tests for the pure parts of `baler.lock` handling: parsing, the
//! format-version check, comparing locks, and building/writing them.
//! Behavior that needs a package index (relocking, `--check`,
//! `--update <pkg>`) is covered where the resolver is tested.

use std::collections::BTreeMap;

use baler_core::lockfile::{
    build_grouped, parse, read, write_grouped, BalerLock, LockGroup, LockedPackage,
    PinnedPackage, LOCK_FILE_NAME,
};
use semver::Version;

fn pkg(name: &str, version: &str, groups: &[LockGroup]) -> LockedPackage {
    LockedPackage {
        name: name.into(),
        version: version.into(),
        repo: "https://cran.example".into(),
        groups: groups.to_vec(),
    }
}

fn lock(packages: Vec<LockedPackage>, r_version: Option<&str>) -> BalerLock {
    BalerLock { version: 2, r_version: r_version.map(str::to_owned), packages }
}

#[test]
fn parse_accepts_both_supported_format_versions() {
    assert!(parse("version = 1\n", "x").is_ok());
    assert!(parse("version = 2\n", "x").is_ok());
}

#[test]
fn parse_rejects_unknown_format_versions_and_names_the_origin() {
    let err = parse("version = 3\n", "bundle.tar.gz").unwrap_err().to_string();
    assert!(err.contains("bundle.tar.gz"), "{err}");
    assert!(err.contains("version 3"), "{err}");
    assert!(parse("version = 0\n", "x").is_err());
}

#[test]
fn same_content_ignores_order_format_version_and_explicit_runtime_group() {
    let a = lock(
        vec![pkg("b", "1.0.0", &[]), pkg("a", "2.0.0", &[LockGroup::Runtime])],
        None,
    );
    let mut b = lock(vec![pkg("a", "2.0.0", &[]), pkg("b", "1.0.0", &[])], None);
    b.version = 1;
    assert!(a.same_content(&b));
}

#[test]
fn same_content_notices_version_group_and_r_version_changes() {
    let base = lock(vec![pkg("a", "1.0.0", &[])], Some("4.4.1"));
    assert!(!base.same_content(&lock(vec![pkg("a", "1.0.1", &[])], Some("4.4.1"))));
    assert!(!base.same_content(&lock(vec![pkg("a", "1.0.0", &[LockGroup::Build])], Some("4.4.1"))));
    assert!(!base.same_content(&lock(vec![pkg("a", "1.0.0", &[])], Some("4.3.0"))));
    assert!(!base.same_content(&lock(vec![pkg("a", "1.0.0", &[])], None)));
}

#[test]
fn build_grouped_drops_runtime_only_groups_and_sorts() {
    let mut pinned = BTreeMap::new();
    for (name, groups) in [
        ("z", vec![LockGroup::Runtime]),
        ("a", vec![LockGroup::Extras, LockGroup::Runtime, LockGroup::Runtime]),
    ] {
        pinned.insert(
            name.to_string(),
            PinnedPackage {
                version: Version::new(1, 0, 0),
                repo: "r".into(),
                groups,
            },
        );
    }
    let built = build_grouped(&pinned, None);
    assert_eq!(built.packages[0].name, "a");
    assert_eq!(built.packages[0].groups, vec![LockGroup::Runtime, LockGroup::Extras]);
    assert_eq!(built.packages[1].name, "z");
    assert!(built.packages[1].groups.is_empty());
}

#[test]
fn write_then_read_round_trips_and_is_stable() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut pinned = BTreeMap::new();
    pinned.insert(
        "a".to_string(),
        PinnedPackage { version: Version::new(1, 2, 3), repo: "r".into(), groups: vec![] },
    );
    write_grouped(dir.path(), &pinned, Some("4.4.1")).unwrap();
    let first = std::fs::read_to_string(dir.path().join(LOCK_FILE_NAME)).unwrap();
    let read_back = read(dir.path()).unwrap().unwrap();
    assert!(read_back.same_content(&build_grouped(&pinned, Some("4.4.1"))));

    write_grouped(dir.path(), &pinned, Some("4.4.1")).unwrap();
    let second = std::fs::read_to_string(dir.path().join(LOCK_FILE_NAME)).unwrap();
    assert_eq!(first, second);
}

#[test]
fn changes_lists_added_removed_and_changed() {
    let old = lock(vec![pkg("a", "1.0.0", &[]), pkg("gone", "1.0.0", &[])], None);
    let new = lock(vec![pkg("a", "1.1.0", &[]), pkg("fresh", "0.1.0", &[])], Some("4.4.1"));
    let lines = old.changes(&new);
    assert!(lines.contains(&"~ a 1.0.0 -> 1.1.0".to_string()), "{lines:?}");
    assert!(lines.contains(&"- gone 1.0.0".to_string()), "{lines:?}");
    assert!(lines.contains(&"+ fresh 0.1.0".to_string()), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("~ r_version")), "{lines:?}");
    assert!(old.changes(&old).is_empty());
}

#[test]
fn pins_rejects_a_malformed_version() {
    let bad = lock(vec![pkg("a", "not-a-version", &[])], None);
    assert!(bad.pins().is_err());
    let good = lock(vec![pkg("a", "1.0.0", &[])], None);
    assert_eq!(good.pins().unwrap()["a"].0, Version::new(1, 0, 0));
}

#[test]
fn without_removes_only_the_named_packages() {
    let l = lock(vec![pkg("a", "1.0.0", &[]), pkg("b", "1.0.0", &[])], Some("4.4.1"));
    let trimmed = l.without(&["a".to_string()]);
    assert_eq!(trimmed.packages.len(), 1);
    assert_eq!(trimmed.packages[0].name, "b");
    assert_eq!(trimmed.r_version.as_deref(), Some("4.4.1"));
}

#[test]
fn in_group_treats_missing_groups_as_runtime() {
    assert!(pkg("a", "1.0.0", &[]).in_group(LockGroup::Runtime));
    assert!(!pkg("a", "1.0.0", &[]).in_group(LockGroup::Build));
    assert!(pkg("a", "1.0.0", &[LockGroup::Build]).in_group(LockGroup::Build));
    assert!(!pkg("a", "1.0.0", &[LockGroup::Build]).in_group(LockGroup::Runtime));
}
