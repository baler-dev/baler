//! The machine settings through the real `baler` binary: `--lib`,
//! `BALER_LIB` and `~/.baler/config.toml`.
//!
//! Every test gets its own home directory, so a developer's own
//! `~/.baler/config.toml` and `BALER_*` variables never leak in.

use assert_cmd::Command;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

struct Scratch(PathBuf);
impl Scratch {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("baler-settings-{label}-{n}-{}", std::process::id()));
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

fn baler(home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("baler").expect("baler binary should be built by `cargo test`");
    cmd.env_remove("RUST_BACKTRACE")
        .env_remove("BALER_LIB")
        .env_remove("BALER_INSTALL_DEPS")
        .env("HOME", home)
        .env("USERPROFILE", home);
    cmd
}

fn write_config(home: &Path, contents: &str) {
    let dir = home.join(".baler");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), contents).unwrap();
}

fn init_project(home: &Path, parent: &Path) -> PathBuf {
    let project = parent.join("mymod-proj");
    baler(home)
        .args(["init", "mymod", "--dir-name", project.to_str().unwrap()])
        .assert()
        .success();
    project
}

fn stderr_of(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stderr.clone()).unwrap()
}

// ---- lib ----

#[test]
fn the_default_lib_is_under_the_home_directory() {
    let home = Scratch::new("default-lib");
    let work = Scratch::new("default-lib-work");
    let project = init_project(home.path(), work.path());

    baler(home.path()).args(["install", "--path", project.to_str().unwrap()]).assert().success();

    assert!(home.path().join(".baler/modules/mymod/__init__.r").is_file());
}

#[test]
fn the_config_file_sets_the_lib() {
    let home = Scratch::new("config-lib");
    let work = Scratch::new("config-lib-work");
    let target = Scratch::new("config-lib-target");
    let project = init_project(home.path(), work.path());
    write_config(home.path(), &format!("lib = '{}'\n", target.path().display()));

    baler(home.path()).args(["install", "--path", project.to_str().unwrap()]).assert().success();

    assert!(target.path().join("mymod/__init__.r").is_file());
    assert!(!home.path().join(".baler/modules").exists());
}

#[test]
fn the_environment_beats_the_config_file_for_the_lib() {
    let home = Scratch::new("env-lib");
    let work = Scratch::new("env-lib-work");
    let from_file = Scratch::new("env-lib-file");
    let from_env = Scratch::new("env-lib-env");
    let project = init_project(home.path(), work.path());
    write_config(home.path(), &format!("lib = '{}'\n", from_file.path().display()));

    baler(home.path())
        .env("BALER_LIB", from_env.path())
        .args(["install", "--path", project.to_str().unwrap()])
        .assert()
        .success();

    assert!(from_env.path().join("mymod/__init__.r").is_file());
    assert!(!from_file.path().join("mymod").exists());
}

#[test]
fn the_flag_beats_the_environment_and_the_config_file_for_the_lib() {
    let home = Scratch::new("flag-lib");
    let work = Scratch::new("flag-lib-work");
    let from_file = Scratch::new("flag-lib-file");
    let from_env = Scratch::new("flag-lib-env");
    let from_flag = Scratch::new("flag-lib-flag");
    let project = init_project(home.path(), work.path());
    write_config(home.path(), &format!("lib = '{}'\n", from_file.path().display()));

    baler(home.path())
        .env("BALER_LIB", from_env.path())
        .args(["--lib", from_flag.path().to_str().unwrap(), "install", "--path", project.to_str().unwrap()])
        .assert()
        .success();

    assert!(from_flag.path().join("mymod/__init__.r").is_file());
    assert!(!from_env.path().join("mymod").exists());
    assert!(!from_file.path().join("mymod").exists());
}

#[test]
fn the_lib_flag_can_follow_the_subcommand() {
    let home = Scratch::new("global-lib");
    let work = Scratch::new("global-lib-work");
    let target = Scratch::new("global-lib-target");
    let project = init_project(home.path(), work.path());

    baler(home.path())
        .args(["install", "--path", project.to_str().unwrap(), "--lib", target.path().to_str().unwrap()])
        .assert()
        .success();

    assert!(target.path().join("mymod/__init__.r").is_file());
}

#[test]
fn a_relative_lib_flag_is_relative_to_the_current_directory() {
    let home = Scratch::new("relative-lib");
    let work = Scratch::new("relative-lib-work");
    let project = init_project(home.path(), work.path());

    baler(home.path())
        .current_dir(work.path())
        .args(["--lib", "mods", "install", "--path", project.to_str().unwrap()])
        .assert()
        .success();

    assert!(work.path().join("mods/mymod/__init__.r").is_file());
}

#[test]
fn list_reads_the_lib_that_install_wrote() {
    let home = Scratch::new("list-lib");
    let work = Scratch::new("list-lib-work");
    let target = Scratch::new("list-lib-target");
    let project = init_project(home.path(), work.path());
    write_config(home.path(), &format!("lib = '{}'\n", target.path().display()));

    baler(home.path()).args(["install", "--path", project.to_str().unwrap()]).assert().success();
    let assert = baler(home.path()).arg("list").assert().success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("mymod"), "stdout was:\n{stdout}");
}

// ---- bad settings ----

#[test]
fn an_unknown_key_in_the_config_file_is_rejected() {
    let home = Scratch::new("bad-key");
    write_config(home.path(), "install-dep = false\n");

    let assert = baler(home.path()).arg("list").assert().failure();

    assert!(stderr_of(&assert).contains("install-dep"), "stderr was:\n{}", stderr_of(&assert));
}

#[test]
fn a_misspelled_install_deps_variable_is_rejected() {
    let home = Scratch::new("bad-env");

    let assert = baler(home.path()).env("BALER_INSTALL_DEPS", "maybe").arg("list").assert().failure();

    assert!(stderr_of(&assert).contains("BALER_INSTALL_DEPS"), "stderr was:\n{}", stderr_of(&assert));
}

// ---- the lock does not depend on the machine settings ----

#[test]
fn the_lib_setting_does_not_change_the_lock() {
    let home = Scratch::new("lock-lib");
    let work = Scratch::new("lock-lib-work");
    let lib_a = Scratch::new("lock-lib-a");
    let lib_b = Scratch::new("lock-lib-b");
    let project = init_project(home.path(), work.path());
    let lock = project.join("baler.lock");

    baler(home.path()).args(["--lib", lib_a.path().to_str().unwrap(), "lock", project.to_str().unwrap()]).assert().success();
    let first = std::fs::read(&lock).unwrap();
    std::fs::remove_file(&lock).unwrap();

    baler(home.path()).args(["--lib", lib_b.path().to_str().unwrap(), "lock", project.to_str().unwrap()]).assert().success();

    assert_eq!(std::fs::read(&lock).unwrap(), first);
}
