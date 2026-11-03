use baler_core::baler_toml::ModuleSource;
use baler_core::lockfile::{self, BalerLock, LockedModule};

const RCALC: &str = "https://github.com/baler-dev/baler";

fn declared(git_ref: &str) -> ModuleSource {
    ModuleSource::Git {
        url: RCALC.to_owned(),
        git_ref: Some(git_ref.to_owned()),
        module_dir: None,
    }
}

fn pin() -> LockedModule {
    LockedModule {
        name: "rcalc".to_owned(),
        version: "0.1.0".to_owned(),
        git: Some(RCALC.to_owned()),
        git_ref: Some("v1".to_owned()),
        module_dir: None,
        rev: Some("9fceb02".to_owned()),
        url: None,
    }
}

#[test]
fn a_lock_without_modules_stays_format_2() {
    let lock = BalerLock::from_packages(Vec::new());
    assert_eq!(lock.version, 2);
    assert_eq!(lock.clone().with_modules(vec![pin()]).version, 3);
    assert_eq!(lock.with_modules(Vec::new()).version, 2);
}

#[test]
fn module_entries_round_trip_through_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let lock = BalerLock::from_packages(Vec::new()).with_modules(vec![pin()]);
    lockfile::write_lock(dir.path(), &lock).unwrap();

    let text = std::fs::read_to_string(dir.path().join(lockfile::LOCK_FILE_NAME)).unwrap();
    assert!(text.contains("[[module]]"), "{text}");
    assert!(text.contains("rev = \"9fceb02\""), "{text}");
    assert!(text.contains("ref = \"v1\""), "{text}");

    let back = lockfile::read(dir.path()).unwrap().unwrap();
    assert_eq!(back, lock);
}

#[test]
fn a_lock_from_a_newer_format_asks_for_an_upgrade() {
    let error = lockfile::parse("version = 4\n[[future]]\nx = 1\n", "baler.lock").unwrap_err();
    let text = error.to_string();
    assert!(text.contains("version 4") && text.contains("update baler"), "{text}");
}

#[test]
fn a_moved_pin_is_a_change() {
    let old = BalerLock::from_packages(Vec::new()).with_modules(vec![pin()]);
    let mut moved = pin();
    moved.rev = Some("abc1234".to_owned());
    let new = BalerLock::from_packages(Vec::new()).with_modules(vec![moved]);

    assert!(!old.same_content(&new));
    assert!(old.same_content(&old.clone()));
    let changes = old.changes(&new);
    assert_eq!(changes, ["~ module rcalc moved from 9fceb02 to abc1234"]);
}

#[test]
fn an_entry_matches_only_the_source_it_was_locked_from() {
    assert!(pin().matches(&declared("v1")));
    assert!(!pin().matches(&declared("v2")));
}

#[test]
fn the_pinned_source_uses_the_commit() {
    let pinned = pin().pinned_source().unwrap();
    assert_eq!(pinned, declared("9fceb02"));
}

#[test]
fn modules_are_looked_up_by_name_when_updating() {
    let lock = BalerLock::from_packages(Vec::new()).with_modules(vec![pin()]);
    assert!(lock.locked_module("rcalc").is_some());
    assert!(lock.locked_module("other").is_none());
    let without = lock.without(&["rcalc".to_owned()]);
    assert!(without.modules.is_empty());
    assert_eq!(without.version, lock.version);
}
