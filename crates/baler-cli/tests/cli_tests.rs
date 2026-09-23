//! Integration tests for the `baler` CLI binary. These spawn the
//! actual compiled executable via `assert_cmd`, so they catch clap
//! wiring bugs that unit tests against `commands::*::run()` can't.
//!
//! Each test spawns its own subprocess, so env vars set via `.env(...)`
//! are isolated per test, unlike baler-core's BALER_LIB-mutating tests.

use assert_cmd::Command;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

fn unique_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("baler-cli-test-{label}-{n}-{}", std::process::id()))
}

struct Scratch(PathBuf);
impl Scratch {
    /// Creates the directory immediately.
    fn new(label: &str) -> Self {
        let dir = unique_dir(label);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    /// Reserves a path without creating it, for dirs the CLI itself creates.
    fn reserved(label: &str) -> Self {
        Self(unique_dir(label))
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

fn baler_cmd() -> Command {
    let mut cmd = Command::cargo_bin("baler").expect("baler binary should be built by `cargo test`");
    // Strip RUST_BACKTRACE so stderr assertions stay deterministic.
    cmd.env_remove("RUST_BACKTRACE");
    cmd
}

// ---- --version / --help / no subcommand ----

#[test]
fn version_flag_prints_crate_version() {
    let assert = baler_cmd().arg("--version").assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert_eq!(stdout.trim(), format!("baler {}", env!("CARGO_PKG_VERSION")));
}

#[test]
fn help_flag_lists_all_subcommands() {
    let assert = baler_cmd().arg("--help").assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for subcommand in ["init", "bundle", "install", "remove"] {
        assert!(stdout.contains(subcommand), "--help output missing '{subcommand}':\n{stdout}");
    }
}

#[test]
fn no_subcommand_exits_nonzero_with_usage() {
    let assert = baler_cmd().assert().failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    // Binary name varies by platform, so pin to stable structural content.
    assert!(stderr.contains("Commands:"), "stderr was:\n{stderr}");
    assert!(stderr.contains("install"), "stderr was:\n{stderr}");
}

// ---- init ----

#[test]
fn init_creates_expected_project_layout() {
    let target = Scratch::reserved("init-layout");

    baler_cmd()
        .args(["init", "mymod", "--dir-name", target.path().to_str().unwrap()])
        .assert()
        .success();

    assert!(target.path().join("baler.toml").is_file());
    assert!(target.path().join("README.md").is_file());
    assert!(target.path().join("mymod").join("__init__.r").is_file());
}

#[test]
fn init_dir_name_flag_is_wired_to_clap_correctly() {
    // Checks --dir-name (kebab-case) reaches InitArgs.dir_name (snake_case).
    let target = Scratch::reserved("dir-name-wiring");

    baler_cmd()
        .args(["init", "somemod", "--dir-name", target.path().to_str().unwrap()])
        .assert()
        .success();

    assert!(target.path().is_dir());
    let contents = std::fs::read_to_string(target.path().join("baler.toml")).unwrap();
    assert!(contents.contains("name = \"somemod\""));
}

#[test]
fn init_missing_name_arg_fails_with_usage_error() {
    baler_cmd().arg("init").assert().failure();
}

// ---- bundle ----

#[test]
fn bundle_produces_tar_gz_by_default() {
    let cwd = Scratch::new("bundle-cwd");
    let project = cwd.path().join("mymod-proj");

    baler_cmd()
        .args(["init", "mymod", "--dir-name", project.to_str().unwrap()])
        .assert()
        .success();

    baler_cmd()
        .current_dir(cwd.path())
        .args(["bundle", project.to_str().unwrap()])
        .assert()
        .success();

    assert!(cwd.path().join("mymod_0.1.0.tar.gz").is_file());
}

// ---- install / remove ----

#[test]
fn install_then_remove_round_trip() {
    let project_root = Scratch::new("install-project-root");
    let project = project_root.path().join("mymod-proj");
    let lib = Scratch::reserved("install-lib");

    baler_cmd()
        .args(["init", "mymod", "--dir-name", project.to_str().unwrap()])
        .assert()
        .success();

    baler_cmd()
        .env("BALER_LIB", lib.path())
        .args(["install", "--path", project.to_str().unwrap()])
        .assert()
        .success();

    let module_dir = lib.path().join("mymod");
    assert!(module_dir.join("__init__.r").is_file());

    baler_cmd()
        .env("BALER_LIB", lib.path())
        .args(["remove", "mymod", "--force"])
        .assert()
        .success();

    assert!(!module_dir.exists());
}

#[test]
fn install_on_nonexistent_source_fails_with_clear_error() {
    let bogus = unique_dir("install-bogus-source");

    let assert = baler_cmd()
        .args(["install", "--path", bogus.to_str().unwrap()])
        .assert()
        .failure()
        .code(1);

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("--path expects a directory or a .tar.gz"),
        "stderr was:\n{stderr}"
    );
}

#[test]
fn remove_nonexistent_module_fails_with_clear_error() {
    let lib = Scratch::new("remove-empty-lib");

    let assert = baler_cmd()
        .env("BALER_LIB", lib.path())
        .args(["remove", "doesnotexist", "--force"])
        .assert()
        .failure()
        .code(1);

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("is not installed"), "stderr was:\n{stderr}");
}

#[test]
fn remove_without_force_respects_declined_confirmation() {
    let project_root = Scratch::new("remove-confirm-project-root");
    let project = project_root.path().join("mymod-proj");
    let lib = Scratch::reserved("remove-confirm-lib");

    baler_cmd()
        .args(["init", "mymod", "--dir-name", project.to_str().unwrap()])
        .assert()
        .success();

    baler_cmd()
        .env("BALER_LIB", lib.path())
        .args(["install", "--path", project.to_str().unwrap()])
        .assert()
        .success();

    let module_dir = lib.path().join("mymod");
    assert!(module_dir.exists());

    // No --force: answering "n" to the stdin prompt should decline,
    // leave the module installed, and still exit successfully.
    let assert = baler_cmd()
        .env("BALER_LIB", lib.path())
        .args(["remove", "mymod"])
        .write_stdin("n\n")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("Aborted."), "stdout was:\n{stdout}");

    assert!(module_dir.exists(), "module should still be installed after declining removal");
}

// ---- bare-name reservation (module registry conflict) ----
//
// The positional <name> is always a bare registry name. It's never
// sniffed as a path, archive, or GitHub reference. Local and GitHub
// installs always require the explicit --path or --git flag.

#[test]
fn install_bare_name_matching_local_dir_is_reserved_not_silently_installed() {
    let cwd = Scratch::new("bare-name-cwd");
    let project = cwd.path().join("convert-proj");

    baler_cmd()
        .args(["init", "convert", "--dir-name", project.to_str().unwrap()])
        .assert()
        .success();

    // convert-proj/ genuinely exists in cwd, but must NOT be silently installed.
    let assert = baler_cmd()
        .current_dir(cwd.path())
        .args(["install", "convert-proj"])
        .assert()
        .failure()
        .code(1);

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("looks like a module name"), "stderr was:\n{stderr}");
    assert!(stderr.contains("registry"), "stderr was:\n{stderr}");
}

#[test]
fn install_path_flag_installs_local_dir() {
    let cwd = Scratch::new("path-flag-dir-cwd");
    let project = cwd.path().join("convert-proj");
    let lib = Scratch::reserved("path-flag-dir-lib");

    baler_cmd()
        .args(["init", "convert", "--dir-name", project.to_str().unwrap()])
        .assert()
        .success();

    // Same directory as the reservation test above, via --path this time.
    baler_cmd()
        .current_dir(cwd.path())
        .env("BALER_LIB", lib.path())
        .args(["install", "--path", "convert-proj"])
        .assert()
        .success();

    assert!(lib.path().join("convert").join("__init__.r").is_file());
}

#[test]
fn install_path_flag_accepts_bare_archive_filename() {
    let cwd = Scratch::new("path-flag-archive-cwd");
    let project = cwd.path().join("mymod-proj");
    let lib = Scratch::reserved("path-flag-archive-lib");

    baler_cmd()
        .args(["init", "mymod", "--dir-name", project.to_str().unwrap()])
        .assert()
        .success();

    baler_cmd()
        .current_dir(cwd.path())
        .args(["bundle", project.to_str().unwrap()])
        .assert()
        .success();

    // --path never sniffs shape, so a bare filename needs no ./ prefix.
    baler_cmd()
        .current_dir(cwd.path())
        .env("BALER_LIB", lib.path())
        .args(["install", "--path", "mymod_0.1.0.tar.gz"])
        .assert()
        .success();

    assert!(lib.path().join("mymod").join("__init__.r").is_file());
}

// ---- --repo scaffolding (no registry backend yet) ----
//
// install_from_registry's body is the one piece left as a stub; the
// flag wiring and mutual-exclusivity checks are real and tested here.

#[test]
fn install_bare_name_with_repo_hits_the_not_implemented_stub() {
    let assert = baler_cmd()
        .args(["install", "somepkg", "--repo", "https://modules.example.com"])
        .assert()
        .failure()
        .code(1);

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("registries aren't implemented yet"), "stderr was:\n{stderr}");
    assert!(stderr.contains("somepkg"), "stderr was:\n{stderr}");
    assert!(stderr.contains("https://modules.example.com"), "stderr was:\n{stderr}");
}

#[test]
fn install_repo_flag_rejected_with_git_flag() {
    // --repo and --git conflict at the clap layer (lib.rs).
    let assert = baler_cmd()
        .args([
            "install",
            "--git",
            "https://github.com/someuser/somerepo",
            "--repo",
            "https://modules.example.com",
        ])
        .assert()
        .failure();

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("cannot be used with"), "stderr was:\n{stderr}");
}

#[test]
fn install_repo_flag_rejected_with_path_flag() {
    let cwd = Scratch::new("repo-flag-path-flag-cwd");
    let project = cwd.path().join("mymod-proj");

    baler_cmd()
        .args(["init", "mymod", "--dir-name", project.to_str().unwrap()])
        .assert()
        .success();

    let assert = baler_cmd()
        .args(["install", "--path", "./mymod-proj", "--repo", "https://modules.example.com"])
        .current_dir(cwd.path())
        .assert()
        .failure();

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("cannot be used with"), "stderr was:\n{stderr}");
}

#[test]
fn install_bare_name_without_repo_still_gets_the_original_reserved_error() {
    let assert = baler_cmd().args(["install", "somepkg"]).assert().failure().code(1);

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("looks like a module name"), "stderr was:\n{stderr}");
    assert!(stderr.contains("--repo"), "stderr was:\n{stderr}");
}
