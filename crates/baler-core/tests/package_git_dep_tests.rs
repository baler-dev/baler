//! R package dependencies that come from GitHub, checked through
//! `BalerToml::from_dir` so the load-time validation runs too.

use baler_core::baler_toml::{BalerToml, PackageDep, PackageGitSpec};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

const HEADER: &str = r#"
[project]
name = "demo"
version = "0.1.0"
description = "demo"
authors = ["Someone"]
license = "MIT"
r_version = ">=4.0.0"
"#;

const BH: &str = "https://github.com/eddelbuettel/bh";

/// Writes `HEADER` plus `extra` to a fresh temp dir and loads it.
fn load(extra: &str) -> anyhow::Result<BalerToml> {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir: PathBuf =
        std::env::temp_dir().join(format!("baler-package-git-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("baler.toml"), format!("{HEADER}\n{extra}")).unwrap();
    let loaded = BalerToml::from_dir(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    loaded
}

fn error_text(extra: &str) -> String {
    format!("{:#}", load(extra).expect_err("manifest should be rejected"))
}

fn git_spec(dep: &PackageDep) -> &PackageGitSpec {
    match dep {
        PackageDep::Git(spec) => spec,
        other => panic!("expected a git entry, got {other:?}"),
    }
}

#[test]
fn git_alone_is_read_and_the_version_defaults_to_any() {
    let toml = load(&format!("[dependencies]\nBH = {{ git = \"{BH}\" }}\n")).unwrap();
    let dep = &toml.dependencies.packages["BH"];
    assert_eq!(git_spec(dep).git, BH);
    assert_eq!(dep.version(), "*");
    assert_eq!(dep.repo(), BH);
    assert_eq!(dep.explicit_repo(), Some(BH));
}

#[test]
fn git_with_a_tag_and_a_version_is_read() {
    let toml = load(&format!(
        "[dependencies]\nBH = {{ git = \"{BH}\", tag = \"1.84.0-0\", version = \">=1.80\" }}\n"
    ))
    .unwrap();
    let dep = &toml.dependencies.packages["BH"];
    assert_eq!(git_spec(dep).git_ref(), Some("1.84.0-0"));
    assert_eq!(dep.version(), ">=1.80");
}

#[test]
fn git_is_read_in_build_deps() {
    let toml = load(&format!(
        "[compiled-code.build_deps]\nBH = {{ git = \"{BH}\" }}\nRcpp = \"*\"\n"
    ))
    .unwrap();
    let build = toml.compiled_code.unwrap().build_deps.unwrap();
    assert_eq!(git_spec(&build["BH"]).git, BH);
    assert!(matches!(build["Rcpp"], PackageDep::Simple(_)));
}

#[test]
fn git_is_read_in_extras() {
    let toml = load(&format!("[extras]\nBH = {{ git = \"{BH}\" }}\n")).unwrap();
    let extras = toml.extras.unwrap();
    assert_eq!(git_spec(&extras.packages["BH"]).git, BH);
}

#[test]
fn bare_version_and_repo_entries_still_read_as_before() {
    let toml = load(
        "[dependencies]\ncli = \">=3.6\"\n\
         dplyr = { version = \">=1.1\", repo = \"https://packagemanager.posit.co/cran/latest\" }\n",
    )
    .unwrap();
    let packages = &toml.dependencies.packages;
    assert!(matches!(packages["cli"], PackageDep::Simple(_)));
    assert!(matches!(packages["dplyr"], PackageDep::Extended { .. }));
    assert_eq!(
        packages["dplyr"].explicit_repo(),
        Some("https://packagemanager.posit.co/cran/latest")
    );
    assert_eq!(packages["cli"].explicit_repo(), None);
}

#[test]
fn git_together_with_repo_is_rejected() {
    let err = error_text(&format!(
        "[dependencies]\nBH = {{ git = \"{BH}\", repo = \"https://cloud.r-project.org\" }}\n"
    ));
    assert!(err.contains("BH"), "{err}");
    assert!(err.contains("mutually exclusive"), "{err}");
}

#[test]
fn two_refs_are_rejected() {
    let err = error_text(&format!(
        "[compiled-code.build_deps]\nBH = {{ git = \"{BH}\", tag = \"a\", branch = \"b\" }}\n"
    ));
    assert!(err.contains("pick one ref"), "{err}");
}

#[test]
fn a_misspelled_key_is_rejected() {
    assert!(load(&format!("[dependencies]\nBH = {{ git = \"{BH}\", brach = \"main\" }}\n")).is_err());
}

#[test]
fn a_table_with_neither_version_nor_git_is_rejected() {
    assert!(load("[dependencies]\nBH = { tag = \"1.0\" }\n").is_err());
}
