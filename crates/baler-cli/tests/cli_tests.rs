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
fn init_without_a_name_initializes_the_current_directory() {
    let dir = Scratch::new("init-in-place");

    baler_cmd().current_dir(dir.path()).arg("init").assert().success();

    assert!(dir.path().join("baler.toml").is_file());
    assert!(dir.path().join("README.md").is_file());
}

#[test]
fn init_without_a_name_refuses_to_overwrite_baler_toml() {
    let dir = Scratch::new("init-in-place-existing");
    std::fs::write(dir.path().join("baler.toml"), "# mine\n").unwrap();

    baler_cmd().current_dir(dir.path()).arg("init").assert().failure();

    let contents = std::fs::read_to_string(dir.path().join("baler.toml")).unwrap();
    assert_eq!(contents, "# mine\n");
}

#[test]
fn init_name_flag_and_positional_name_conflict() {
    // `--name` is for the in-place form; with `<name>` it is a usage error.
    let dir = Scratch::new("init-name-conflict");

    baler_cmd()
        .current_dir(dir.path())
        .args(["init", "somemod", "--name", "other"])
        .assert()
        .failure();

    assert!(!dir.path().join("baler.toml").exists());
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

// ---- lock / bundle --locked ----
//
// These use a freshly `init`ed project, which declares no R packages, so
// `baler lock` has nothing to resolve and never touches the network. They
// pin down the flag wiring and the "an unchanged lock is left alone"
// behavior, not CRAN resolution.

fn init_project(cwd: &Scratch) -> PathBuf {
    let project = cwd.path().join("mymod-proj");
    baler_cmd()
        .args(["init", "mymod", "--dir-name", project.to_str().unwrap()])
        .assert()
        .success();
    project
}

fn lock_path(project: &Path) -> PathBuf {
    project.join("baler.lock")
}

fn stderr_of(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stderr.clone()).unwrap()
}

#[test]
fn lock_writes_a_lock_and_a_second_run_leaves_it_untouched() {
    let cwd = Scratch::new("lock-idempotent");
    let project = init_project(&cwd);

    baler_cmd().args(["lock", project.to_str().unwrap()]).assert().success();
    let lock = lock_path(&project);
    assert!(lock.is_file());
    let bytes = std::fs::read(&lock).unwrap();
    let mtime = std::fs::metadata(&lock).unwrap().modified().unwrap();

    let assert = baler_cmd().args(["lock", project.to_str().unwrap()]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("up to date"), "stdout was:\n{stdout}");

    assert_eq!(std::fs::read(&lock).unwrap(), bytes, "an unchanged lock must not be rewritten");
    assert_eq!(std::fs::metadata(&lock).unwrap().modified().unwrap(), mtime);
}

#[test]
fn lock_check_fails_without_a_lock_and_does_not_create_one() {
    let cwd = Scratch::new("lock-check-missing");
    let project = init_project(&cwd);

    let assert = baler_cmd()
        .args(["lock", project.to_str().unwrap(), "--check"])
        .assert()
        .failure()
        .code(1);

    assert!(stderr_of(&assert).contains("No baler.lock found"), "stderr was:\n{}", stderr_of(&assert));
    assert!(!lock_path(&project).exists(), "--check must never write the lock");
}

#[test]
fn lock_check_passes_on_a_current_lock_and_does_not_rewrite_it() {
    let cwd = Scratch::new("lock-check-current");
    let project = init_project(&cwd);

    baler_cmd().args(["lock", project.to_str().unwrap()]).assert().success();
    let before = std::fs::read(lock_path(&project)).unwrap();

    baler_cmd().args(["lock", project.to_str().unwrap(), "--check"]).assert().success();

    assert_eq!(std::fs::read(lock_path(&project)).unwrap(), before);
}

#[test]
fn lock_check_conflicts_with_the_flags_that_write() {
    // Rejected by clap before any project is read, so "." is fine here.
    for other in ["--update", "--with-rver", "--remove"] {
        let assert = baler_cmd().args(["lock", ".", "--check", other]).assert().failure();
        assert!(
            stderr_of(&assert).contains("cannot be used with"),
            "--check with {other}, stderr was:\n{}",
            stderr_of(&assert)
        );
    }
}

#[test]
fn lock_update_rejects_a_path_given_where_a_package_name_goes() {
    // --update takes any number of values, so `--update ./proj` is read as
    // a package name. It should say so instead of "not in baler.lock".
    let assert = baler_cmd()
        .args(["lock", ".", "--update", "./other"])
        .assert()
        .failure()
        .code(1);

    assert!(stderr_of(&assert).contains("looks like a path"), "stderr was:\n{}", stderr_of(&assert));
}

#[test]
fn lock_update_of_a_package_not_in_the_lock_is_an_error() {
    let cwd = Scratch::new("lock-update-unknown");
    let project = init_project(&cwd);
    baler_cmd().args(["lock", project.to_str().unwrap()]).assert().success();

    let assert = baler_cmd()
        .args(["lock", project.to_str().unwrap(), "--update", "nope"])
        .assert()
        .failure()
        .code(1);

    assert!(
        stderr_of(&assert).contains("'nope' is not in baler.lock"),
        "stderr was:\n{}",
        stderr_of(&assert)
    );
}

#[test]
fn lock_update_of_a_package_needs_an_existing_lock() {
    let cwd = Scratch::new("lock-update-no-lock");
    let project = init_project(&cwd);

    let assert = baler_cmd()
        .args(["lock", project.to_str().unwrap(), "--update", "purrr"])
        .assert()
        .failure()
        .code(1);

    assert!(
        stderr_of(&assert).contains("No baler.lock to update"),
        "stderr was:\n{}",
        stderr_of(&assert)
    );
    assert!(!lock_path(&project).exists());
}

#[test]
fn lock_with_rver_is_refused_on_an_existing_lock_without_update() {
    let cwd = Scratch::new("lock-rver-refused");
    let project = init_project(&cwd);
    baler_cmd().args(["lock", project.to_str().unwrap()]).assert().success();
    let before = std::fs::read(lock_path(&project)).unwrap();

    let assert = baler_cmd()
        .args(["lock", project.to_str().unwrap(), "--with-rver"])
        .assert()
        .failure()
        .code(1);

    assert!(stderr_of(&assert).contains("--update"), "stderr was:\n{}", stderr_of(&assert));
    assert_eq!(std::fs::read(lock_path(&project)).unwrap(), before);
}

#[test]
fn lock_and_bundle_help_list_the_new_flags() {
    let assert = baler_cmd().args(["lock", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for flag in ["--update", "--check", "--with-rver", "--remove"] {
        assert!(stdout.contains(flag), "lock --help missing '{flag}':\n{stdout}");
    }

    let assert = baler_cmd().args(["bundle", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("--locked"), "bundle --help missing '--locked':\n{stdout}");
}

#[test]
fn bundle_locked_fails_without_a_lock_and_produces_no_archive() {
    let cwd = Scratch::new("bundle-locked-missing");
    let project = init_project(&cwd);

    let assert = baler_cmd()
        .current_dir(cwd.path())
        .args(["bundle", project.to_str().unwrap(), "--locked"])
        .assert()
        .failure()
        .code(1);

    assert!(
        stderr_of(&assert).contains("--locked needs a baler.lock"),
        "stderr was:\n{}",
        stderr_of(&assert)
    );
    assert!(!cwd.path().join("mymod_0.1.0.tar.gz").exists());
}

#[test]
fn bundle_locked_succeeds_with_a_current_lock() {
    let cwd = Scratch::new("bundle-locked-current");
    let project = init_project(&cwd);
    baler_cmd().args(["lock", project.to_str().unwrap()]).assert().success();

    baler_cmd()
        .current_dir(cwd.path())
        .args(["bundle", project.to_str().unwrap(), "--locked"])
        .assert()
        .success();

    assert!(cwd.path().join("mymod_0.1.0.tar.gz").is_file());
}

#[test]
fn bundle_locked_rejects_an_unreadable_lock_but_plain_bundle_only_warns() {
    let cwd = Scratch::new("bundle-locked-unreadable");
    let project = init_project(&cwd);
    // A lock written by a newer baler than this one understands.
    std::fs::write(lock_path(&project), "version = 99\n").unwrap();

    let assert = baler_cmd()
        .current_dir(cwd.path())
        .args(["bundle", project.to_str().unwrap(), "--locked"])
        .assert()
        .failure()
        .code(1);
    assert!(
        stderr_of(&assert).contains("--locked needs a readable baler.lock"),
        "stderr was:\n{}",
        stderr_of(&assert)
    );
    assert!(!cwd.path().join("mymod_0.1.0.tar.gz").exists());

    let assert = baler_cmd()
        .current_dir(cwd.path())
        .args(["bundle", project.to_str().unwrap()])
        .assert()
        .success();
    assert!(stderr_of(&assert).contains("warning"), "stderr was:\n{}", stderr_of(&assert));
    assert!(cwd.path().join("mymod_0.1.0.tar.gz").is_file());
}
