use std::collections::BTreeMap;

use baler_core::baler_toml::{PackageDep, DEFAULT_CRAN_MIRROR};
use baler_core::lockfile::{BalerLock, LockGroup, LockedPackage};
use baler_core::ops::install::{run, InstallRequest};
use baler_core::ops::resolve::{resolve_locked, resolve_locked_with_extras, resolve_with_extras};

fn deps(text: &str) -> BTreeMap<String, PackageDep> {
    toml::from_str(text).expect("valid dependency table")
}

fn pin(name: &str, version: &str, groups: &[LockGroup]) -> LockedPackage {
    LockedPackage {
        name: name.to_owned(),
        version: version.to_owned(),
        repo: DEFAULT_CRAN_MIRROR.to_owned(),
        groups: groups.to_vec(),
    }
}

/// `waldo` is only reached through `testthat`, so it is locked as an
/// extras package without being declared in baler.toml.
fn lock_with_extras() -> BalerLock {
    BalerLock::from_packages(vec![
        pin("cli", "3.6.2", &[]),
        pin("testthat", "3.2.1", &[LockGroup::Extras]),
        pin("waldo", "0.5.2", &[LockGroup::Extras]),
    ])
}

#[test]
fn normal_install_skips_extras_pins() {
    let runtime = deps(r#"cli = ">=3.0.0""#);
    let plan = resolve_locked(&Some(runtime), &None, &lock_with_extras()).unwrap();

    assert!(plan.packages.contains_key("cli"));
    assert!(!plan.packages.contains_key("testthat"));
    assert!(!plan.packages.contains_key("waldo"));
}

#[test]
fn locked_extras_install_adds_extras_pins_and_their_transitive_packages() {
    let runtime = deps(r#"cli = ">=3.0.0""#);
    let extra = deps(r#"testthat = ">=3.0.0""#);
    let plan = resolve_locked_with_extras(
        &Some(runtime),
        &None,
        &extra,
        &None,
        &lock_with_extras(),
    )
    .unwrap();

    assert!(plan.packages.contains_key("cli"));
    assert_eq!(plan.packages["testthat"].version_spec, ">=3.0.0");
    assert_eq!(plan.packages["waldo"].version_spec, "*");
}

#[test]
fn locked_extras_install_fails_when_the_lock_lacks_an_extras_package() {
    let runtime = deps(r#"cli = ">=3.0.0""#);
    let extra = deps(r#"testthat = ">=3.0.0""#);
    let lock = BalerLock::from_packages(vec![pin("cli", "3.6.2", &[])]);

    let err = resolve_locked_with_extras(&Some(runtime), &None, &extra, &None, &lock).unwrap_err();
    let message = format!("{err:#}");

    assert!(message.contains("testthat"));
    assert!(message.contains("extras"));
    assert!(message.contains("baler lock"));
}

#[test]
fn locked_extras_install_fails_when_the_pin_no_longer_fits_the_constraint() {
    let runtime = deps(r#"cli = ">=3.0.0""#);
    let extra = deps(r#"testthat = ">=3.3.0""#);

    let err = resolve_locked_with_extras(&Some(runtime), &None, &extra, &None, &lock_with_extras())
        .unwrap_err();

    assert!(format!("{err:#}").contains("testthat"));
}

#[test]
fn package_locked_for_both_groups_is_planned_once() {
    let runtime = deps(r#"cli = ">=3.0.0""#);
    let extra = deps(r#"cli = ">=3.0.0""#);
    let lock = BalerLock::from_packages(vec![pin(
        "cli",
        "3.6.2",
        &[LockGroup::Runtime, LockGroup::Extras],
    )]);

    let plan = resolve_locked_with_extras(&Some(runtime), &None, &extra, &None, &lock).unwrap();

    assert_eq!(plan.packages.len(), 1);
    assert!(plan.packages.contains_key("cli"));
}

#[test]
fn unlocked_extras_plan_merges_both_groups() {
    let runtime = deps(r#"cli = ">=3.0.0""#);
    let extra = deps(r#"testthat = ">=3.0.0""#);

    let plan = resolve_with_extras(&Some(runtime), &None, &extra, &None).unwrap();

    assert!(plan.packages.contains_key("cli"));
    assert!(plan.packages.contains_key("testthat"));
}

#[test]
fn unlocked_extras_plan_accepts_an_identical_declaration_in_both_groups() {
    let runtime = deps(r#"cli = ">=3.0.0""#);
    let extra = deps(r#"cli = ">=3.0.0""#);

    let plan = resolve_with_extras(&Some(runtime), &None, &extra, &None).unwrap();

    assert_eq!(plan.packages.len(), 1);
}

#[test]
fn unlocked_extras_plan_rejects_a_differing_declaration_in_both_groups() {
    let runtime = deps(r#"cli = ">=3.0.0""#);
    let extra = deps(r#"cli = ">=3.5.0""#);

    let err = resolve_with_extras(&Some(runtime), &None, &extra, &None).unwrap_err();
    let message = format!("{err:#}");

    assert!(message.contains("'cli'"));
    assert!(message.contains("both [dependencies]"));
    assert!(message.contains(">=3.0.0"));
    assert!(message.contains(">=3.5.0"));
}

#[test]
fn extras_without_install_deps_is_rejected_before_anything_runs() {
    let req = InstallRequest {
        source: None,
        repo: None,
        version: None,
        path: Some(".".to_owned()),
        git: None,
        branch: None,
        tag: None,
        rev: None,
        url: None,
        module_dir: None,
    };

    let err = run(req, false, true).unwrap_err();
    
    assert!(format!("{err:#}").contains("--install-deps"));
}
