//! A GitHub package declared in baler.toml reaches the resolver as a source
//! id, and the lock keeps the commit it was resolved to.

use std::collections::BTreeMap;

use baler_core::baler_toml::{PackageDep, PackageGitSpec, DEFAULT_CRAN_MIRROR};
use baler_core::lockfile::{build_grouped, parse, LockGroup, PinnedPackage};
use baler_core::ops::resolve::{resolve, resolve_locked};
use semver::Version;

fn git_dep(spec: PackageGitSpec) -> BTreeMap<String, PackageDep> {
    let mut deps = BTreeMap::new();
    deps.insert("BH".to_string(), PackageDep::Git(spec));
    deps
}

fn bh() -> PackageGitSpec {
    PackageGitSpec { git: "https://github.com/eddelbuettel/bh".to_string(), ..Default::default() }
}

#[test]
fn a_github_package_resolves_to_its_source_id() {
    let plan = resolve(&Some(git_dep(bh())), &None).unwrap();
    assert_eq!(plan.packages["BH"].repo, "git+https://github.com/eddelbuettel/bh");
    assert_eq!(plan.packages["BH"].version_spec, "*");
}

#[test]
fn a_branch_a_tag_and_a_rev_each_end_up_in_the_id() {
    let branch = PackageGitSpec { branch: Some("devel".into()), ..bh() };
    assert_eq!(branch.id(), "git+https://github.com/eddelbuettel/bh@devel");

    let tag = PackageGitSpec { tag: Some("v1.2".into()), ..bh() };
    assert_eq!(tag.id(), "git+https://github.com/eddelbuettel/bh@v1.2");

    let rev = PackageGitSpec { rev: Some("5a1b9c0".into()), ..bh() };
    assert_eq!(rev.id(), "git+https://github.com/eddelbuettel/bh#5a1b9c0");
}

#[test]
fn a_trailing_slash_or_dot_git_does_not_make_another_source() {
    let plain = PackageGitSpec { git: "https://github.com/eddelbuettel/bh/".into(), ..bh() };
    let dotted = PackageGitSpec { git: "https://github.com/eddelbuettel/bh.git".into(), ..bh() };
    assert_eq!(plain.id(), bh().id());
    assert_eq!(dotted.id(), bh().id());
}

#[test]
fn the_version_constraint_of_a_github_package_is_kept() {
    let spec = PackageGitSpec { version: Some(">= 1.80".into()), ..bh() };
    let plan = resolve(&Some(git_dep(spec)), &None).unwrap();
    assert!(plan.packages["BH"].version_spec.contains("1.80"), "{}", plan.packages["BH"].version_spec);
}

#[test]
fn a_cran_package_still_resolves() {
    let mut deps = BTreeMap::new();
    deps.insert("cli".to_string(), PackageDep::Simple(">=3.6".to_string()));
    let plan = resolve(&Some(deps), &None).unwrap();
    assert_eq!(plan.packages["cli"].repo, DEFAULT_CRAN_MIRROR);
}

// ---- the lock ----

fn lock_with(repo: &str) -> baler_core::lockfile::BalerLock {
    let mut pinned = BTreeMap::new();
    pinned.insert(
        "BH".to_string(),
        PinnedPackage {
            version: Version::parse("1.87.0-1").unwrap(),
            repo: repo.to_string(),
            groups: vec![LockGroup::Build],
        },
    );
    build_grouped(&pinned, None)
}

const SHA: &str = "5a1b9c0d2e3f4a5b6c7d8e9f00112233445566778";

#[test]
fn a_lock_with_a_github_package_is_format_four_and_reads_back() {
    let repo = format!("git+https://github.com/eddelbuettel/bh#{}", &SHA[..40]);
    let lock = lock_with(&repo);
    assert_eq!(lock.version, 4);
    let text = toml::to_string_pretty(&lock).unwrap();
    let read = parse(&text, "baler.lock").unwrap();
    assert!(read.same_content(&lock));
    assert_eq!(read.packages[0].repo, repo);
}

#[test]
fn a_lock_without_one_stays_format_two() {
    assert_eq!(lock_with(DEFAULT_CRAN_MIRROR).version, 2);
}

#[test]
fn a_locked_install_accepts_the_pinned_commit_of_the_declared_source() {
    let repo = format!("git+https://github.com/eddelbuettel/bh#{}", &SHA[..40]);
    let lock = lock_with(&repo);
    let mut deps = git_dep(bh());
    // the lock holds BH in the build group, so declare it as a runtime dep
    // of a lock written for it
    let runtime = {
        let mut pinned = BTreeMap::new();
        pinned.insert(
            "BH".to_string(),
            PinnedPackage {
                version: Version::parse("1.87.0-1").unwrap(),
                repo: repo.clone(),
                groups: vec![],
            },
        );
        build_grouped(&pinned, None)
    };
    let plan = resolve_locked(&Some(deps.clone()), &None, &runtime).unwrap();
    assert_eq!(plan.packages["BH"].repo, repo);

    // the build group is not installed by a runtime install, and is not a hit
    assert!(resolve_locked(&Some(deps.clone()), &None, &lock).is_err());

    // another branch than the one locked is a stale lock
    deps.insert(
        "BH".to_string(),
        PackageDep::Git(PackageGitSpec { branch: Some("devel".into()), ..bh() }),
    );
    let err = resolve_locked(&Some(deps), &None, &runtime).unwrap_err().to_string();
    assert!(err.contains("repo changed"), "{err}");
}
