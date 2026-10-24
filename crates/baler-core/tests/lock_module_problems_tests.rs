use baler_core::baler_toml::BalerToml;
use baler_core::lockfile::{BalerLock, LockedModule};
use baler_core::ops::resolve::lock_problems_for;

const RCALC: &str = "https://github.com/baler-dev/baler";

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

fn root_with_module(tag: &str, constraint: &str) -> BalerToml {
    toml::from_str(&format!(
        "[project]\nname = \"statmodule\"\nversion = \"0.1.0\"\ndescription = \"x\"\n\
         authors = []\nlicense = \"MIT\"\nr_version = \">=4.0\"\n\n\
         [dependencies.baler]\nrcalc = {{ git = \"{RCALC}\", tag = \"{tag}\", version = \"{constraint}\" }}\n"
    ))
    .unwrap()
}

#[test]
fn a_format_3_lock_must_pin_every_module_that_has_a_source() {
    let mut other = pin();
    other.name = "shapes".to_owned();
    let lock = BalerLock::from_packages(Vec::new()).with_modules(vec![other]);
    let problems = lock_problems_for(&root_with_module("v1", "*"), &lock).unwrap();
    assert_eq!(problems, ["module 'rcalc' is not in baler.lock"]);
}

#[test]
fn a_pinned_module_that_still_fits_is_not_a_problem() {
    let lock = BalerLock::from_packages(Vec::new()).with_modules(vec![pin()]);
    assert!(lock_problems_for(&root_with_module("v1", ">=0.1"), &lock).unwrap().is_empty());
}

#[test]
fn a_pinned_version_the_manifest_no_longer_accepts_is_a_problem() {
    let lock = BalerLock::from_packages(Vec::new()).with_modules(vec![pin()]);
    let problems = lock_problems_for(&root_with_module("v1", ">=1.0"), &lock).unwrap();
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("locked at 0.1.0"), "{}", problems[0]);
}

#[test]
fn a_changed_tag_in_the_manifest_is_a_problem() {
    let lock = BalerLock::from_packages(Vec::new()).with_modules(vec![pin()]);
    let problems = lock_problems_for(&root_with_module("v2", "*"), &lock).unwrap();
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("different source"), "{}", problems[0]);
}

#[test]
fn a_lock_from_before_modules_were_pinned_is_not_asked_for_them() {
    let lock = BalerLock::from_packages(Vec::new());
    assert_eq!(lock.version, 2);
    assert!(lock_problems_for(&root_with_module("v1", "*"), &lock).unwrap().is_empty());
}
