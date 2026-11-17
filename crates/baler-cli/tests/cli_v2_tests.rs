//! Integration tests for `baler list`, `baler show`, the license checks,
//! and the `.dist-info` handling they depend on. Each one runs the real
//! `baler` binary with its own `BALER_LIB`, so nothing is shared.

use assert_cmd::Command;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

fn unique_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("baler-list-test-{label}-{n}-{}", std::process::id()))
}

struct Scratch(PathBuf);
impl Scratch {
    fn new(label: &str) -> Self {
        let dir = unique_dir(label);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn baler(lib: &Path) -> Command {
    let mut cmd = Command::cargo_bin("baler").expect("baler binary should be built by `cargo test`");
    cmd.env_remove("RUST_BACKTRACE");
    cmd.env("BALER_LIB", lib);
    cmd
}

fn stdout_of(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

fn stderr_of(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stderr.clone()).unwrap()
}

/// Scaffolds `rmod` with `baler init` and returns the project directory.
fn new_project(workdir: &Scratch, lib: &Path, license: Option<&str>) -> PathBuf {
    let mut cmd = baler(lib);
    cmd.current_dir(workdir.path()).args(["init", "rmod"]);
    if let Some(license) = license {
        cmd.args(["--license", license]);
    }
    cmd.assert().success();
    workdir.path().join("rmod-proj")
}

fn set_version(project: &Path, version: &str) {
    let toml = std::fs::read_to_string(project.join("baler.toml")).unwrap();
    let current = toml.lines().find(|l| l.starts_with("version = ")).unwrap().to_owned();
    std::fs::write(
        project.join("baler.toml"),
        toml.replace(&current, &format!("version = \"{version}\"")),
    )
    .unwrap();
}

/// Bundles `project` and installs the archive.
fn bundle_and_install(project: &Path, lib: &Path, version: &str) {
    baler(lib).current_dir(project).args(["bundle", "."]).assert().success();
    let archive = project.join(format!("rmod_{version}.tar.gz"));
    baler(lib).args(["install", "--path"]).arg(&archive).assert().success();
}

fn entries(dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = match std::fs::read_dir(dir) {
        Ok(read) => read.map(|e| e.unwrap().file_name().to_str().unwrap().to_owned()).collect(),
        Err(_) => Vec::new(),
    };
    found.sort();
    found
}

// ---- list and show ----

#[test]
fn list_on_an_empty_install_directory_says_so() {
    let lib = Scratch::new("list-empty");

    let assert = baler(lib.path()).arg("list").assert().success();

    let out = stdout_of(&assert);
    assert!(out.starts_with("No modules installed in "), "{out}");
}

#[test]
fn list_and_show_follow_a_module_from_install_to_removal() {
    let lib = Scratch::new("round-trip-lib");
    let work = Scratch::new("round-trip-work");
    let project = new_project(&work, lib.path(), Some("MIT"));
    std::fs::write(project.join("LICENSE"), "MIT License\n").unwrap();
    bundle_and_install(&project, lib.path(), "0.1.0");

    let listed = stdout_of(&baler(lib.path()).arg("list").assert().success());
    let row = listed.lines().nth(1).unwrap_or_else(|| panic!("no row in:\n{listed}"));
    assert_eq!(row.split_whitespace().collect::<Vec<_>>(), ["rmod", "0.1.0", "MIT", ">=4.0.0"]);

    let shown = stdout_of(&baler(lib.path()).args(["show", "rmod"]).assert().success());
    assert!(shown.starts_with("rmod 0.1.0\n"), "{shown}");
    assert!(shown.contains("License file: LICENSE"), "{shown}");
    assert!(shown.contains("README:       README.md"), "{shown}");

    // The files really are in the dist-info folder, with their content.
    let dist_info = lib.path().join("rmod-0.1.0.dist-info");
    assert_eq!(std::fs::read_to_string(dist_info.join("LICENSE")).unwrap(), "MIT License\n");
    assert!(dist_info.join("README.md").is_file());

    baler(lib.path()).args(["remove", "rmod", "--force"]).assert().success();

    let after = stdout_of(&baler(lib.path()).arg("list").assert().success());
    assert!(after.starts_with("No modules installed"), "{after}");
    assert_eq!(entries(lib.path()), Vec::<String>::new(), "remove must leave nothing behind");
}

#[test]
fn show_for_a_module_that_is_not_installed_fails_clearly() {
    let lib = Scratch::new("show-missing");

    let assert = baler(lib.path()).args(["show", "nothere"]).assert().failure();

    assert!(stderr_of(&assert).contains("'nothere' is not installed"));
}

// ---- upgrade and removal leave no stale dist-info ----

#[test]
fn upgrading_a_module_leaves_only_the_new_versions_dist_info() {
    let lib = Scratch::new("upgrade-lib");
    let work = Scratch::new("upgrade-work");
    let project = new_project(&work, lib.path(), Some("MIT"));
    bundle_and_install(&project, lib.path(), "0.1.0");
    set_version(&project, "0.2.0");
    bundle_and_install(&project, lib.path(), "0.2.0");

    assert_eq!(entries(lib.path()), ["rmod", "rmod-0.2.0.dist-info"]);
    let listed = stdout_of(&baler(lib.path()).arg("list").assert().success());
    assert_eq!(listed.lines().count(), 2, "{listed}");
    assert!(listed.contains("0.2.0"), "{listed}");
}

#[test]
fn replacing_a_dev_version_with_the_release_leaves_one_dist_info_folder() {
    let lib = Scratch::new("dev-upgrade-lib");
    let work = Scratch::new("dev-upgrade-work");
    let project = new_project(&work, lib.path(), Some("MIT"));
    set_version(&project, "0.3.0-dev");
    bundle_and_install(&project, lib.path(), "0.3.0-dev");
    set_version(&project, "0.3.0");
    bundle_and_install(&project, lib.path(), "0.3.0");

    assert_eq!(entries(lib.path()), ["rmod", "rmod-0.3.0.dist-info"]);
    let listed = stdout_of(&baler(lib.path()).arg("list").assert().success());
    assert!(listed.contains("0.3.0"), "{listed}");
    assert!(!listed.contains("dev"), "{listed}");
}

#[test]
fn removing_a_module_cleans_up_dist_info_that_baler_0_1_0_left_behind() {
    let lib = Scratch::new("leftover-lib");
    // What 0.1.0 left after `baler remove ghost`: metadata, no module.
    let ghost = lib.path().join("ghost-1.0.0.dist-info");
    std::fs::create_dir_all(&ghost).unwrap();
    std::fs::write(ghost.join("baler.toml"), "").unwrap();

    let listed = stdout_of(&baler(lib.path()).arg("list").assert().success());
    assert!(listed.starts_with("No modules installed"), "list must ignore it:\n{listed}");

    let assert = baler(lib.path()).args(["remove", "ghost", "--force"]).assert().success();

    assert!(stdout_of(&assert).contains("cleaned up"));
    assert_eq!(entries(lib.path()), Vec::<String>::new());
}

#[test]
fn remove_refuses_a_name_that_would_delete_more_than_one_module() {
    let lib = Scratch::new("guard-lib");
    let work = Scratch::new("guard-work");
    let project = new_project(&work, lib.path(), Some("MIT"));
    bundle_and_install(&project, lib.path(), "0.1.0");

    for name in [".", "..", "a/b"] {
        let assert = baler(lib.path()).args(["remove", name, "--force"]).assert().failure();
        assert!(stderr_of(&assert).contains("is not a module name"), "{name}");
    }

    assert_eq!(entries(lib.path()), ["rmod", "rmod-0.1.0.dist-info"], "nothing may be deleted");
}

// ---- the license ----

#[test]
fn init_writes_the_license_it_was_given() {
    let lib = Scratch::new("init-license-lib");
    let work = Scratch::new("init-license-work");

    let project = new_project(&work, lib.path(), Some("GPL-3.0-or-later"));

    let toml = std::fs::read_to_string(project.join("baler.toml")).unwrap();
    assert!(toml.contains("license = \"GPL-3.0-or-later\""), "{toml}");
}

#[test]
fn init_in_place_writes_the_license_it_was_given() {
    let lib = Scratch::new("inplace-license-lib");
    let work = Scratch::new("inplace-license-work");

    baler(lib.path())
        .current_dir(work.path())
        .args(["init", "--name", "rmod", "--license", "Apache-2.0"])
        .assert()
        .success();

    let toml = std::fs::read_to_string(work.path().join("baler.toml")).unwrap();
    assert!(toml.contains("license = \"Apache-2.0\""), "{toml}");
}

#[test]
fn init_without_a_license_keeps_the_unknown_placeholder() {
    let lib = Scratch::new("init-default-lib");
    let work = Scratch::new("init-default-work");

    let project = new_project(&work, lib.path(), None);

    let toml = std::fs::read_to_string(project.join("baler.toml")).unwrap();
    assert!(toml.contains("license = \"Unknown\""), "{toml}");
}

#[test]
fn init_refuses_a_license_that_is_not_spdx_and_writes_nothing() {
    let lib = Scratch::new("init-bad-lib");
    let work = Scratch::new("init-bad-work");

    let assert = baler(lib.path())
        .current_dir(work.path())
        .args(["init", "rmod", "--license", "GPL (>= 3)"])
        .assert()
        .failure();

    assert!(stderr_of(&assert).contains("not a valid SPDX expression"));
    assert_eq!(entries(work.path()), Vec::<String>::new());

    baler(lib.path())
        .current_dir(work.path())
        .args(["init", "--name", "rmod", "--license", "mit"])
        .assert()
        .failure();
    assert_eq!(entries(work.path()), Vec::<String>::new());
}

#[test]
fn bundle_warns_when_there_is_no_license_file_but_still_bundles() {
    let lib = Scratch::new("warn-file-lib");
    let work = Scratch::new("warn-file-work");
    let project = new_project(&work, lib.path(), Some("MIT"));

    let assert = baler(lib.path()).current_dir(&project).args(["bundle", "."]).assert().success();

    let err = stderr_of(&assert);
    assert!(err.contains("warning: No license file"), "{err}");
    assert!(!err.contains("SPDX"), "the field is fine, so only the file is flagged:\n{err}");
    assert!(project.join("rmod_0.1.0.tar.gz").is_file());
}

#[test]
fn bundle_is_quiet_about_the_license_when_field_and_file_are_both_fine() {
    let lib = Scratch::new("quiet-lib");
    let work = Scratch::new("quiet-work");
    let project = new_project(&work, lib.path(), Some("MIT"));
    std::fs::write(project.join("LICENSE"), "MIT License\n").unwrap();

    let assert = baler(lib.path()).current_dir(&project).args(["bundle", "."]).assert().success();

    let err = stderr_of(&assert);
    assert!(!err.to_lowercase().contains("license"), "{err}");
}

#[test]
fn bundle_warns_about_the_unknown_placeholder_and_an_invalid_license() {
    let lib = Scratch::new("warn-field-lib");
    let work = Scratch::new("warn-field-work");
    let project = new_project(&work, lib.path(), None);
    std::fs::write(project.join("LICENSE"), "text\n").unwrap();

    let unknown = baler(lib.path()).current_dir(&project).args(["bundle", "."]).assert().success();
    assert!(stderr_of(&unknown).contains("is \"Unknown\""), "{}", stderr_of(&unknown));

    let toml = std::fs::read_to_string(project.join("baler.toml")).unwrap();
    std::fs::write(project.join("baler.toml"), toml.replace("\"Unknown\"", "\"GPL (>= 3)\"")).unwrap();
    let invalid = baler(lib.path()).current_dir(&project).args(["bundle", "."]).assert().success();
    assert!(stderr_of(&invalid).contains("not a valid SPDX expression"), "{}", stderr_of(&invalid));
}

#[test]
fn bundle_warns_about_a_declared_readme_that_does_not_exist() {
    let lib = Scratch::new("warn-readme-lib");
    let work = Scratch::new("warn-readme-work");
    let project = new_project(&work, lib.path(), Some("MIT"));
    std::fs::write(project.join("LICENSE"), "text\n").unwrap();
    let toml = std::fs::read_to_string(project.join("baler.toml")).unwrap();
    std::fs::write(
        project.join("baler.toml"),
        toml.replace("# readme = \"README.md\"", "readme = \"docs/gone.md\""),
    )
    .unwrap();

    let assert = baler(lib.path()).current_dir(&project).args(["bundle", "."]).assert().success();

    assert!(stderr_of(&assert).contains("docs/gone.md"), "{}", stderr_of(&assert));
}
