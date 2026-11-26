//! The machine settings through the real `baler` binary: `BALER_LIB`,
//! `~/.baler/config.toml`, `baler config`, and the install-deps flags.
//!
//! Every test gets its own home directory, so a developer's own
//! `~/.baler/config.toml` and `BALER_*` variables never leak in.
//!
//! The install-deps tests use `install --extras`, which refuses to start
//! while install-deps is off, before it reads the module or touches R or
//! the network. A path that does not exist then tells the two cases apart:
//! off fails on the refusal, on fails later on the missing path.

use assert_cmd::Command;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const REFUSAL: &str = "--extras needs --install-deps";

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
        .env_remove("R_USER")
        .env_remove("R_ENVIRON_USER")
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

/// Runs `install --extras` on a path that does not exist and says whether
/// it was refused for install-deps being off.
fn refused(cmd: &mut Command, extra_args: &[&str]) -> bool {
    let assert = cmd
        .args(["install", "--path", "does-not-exist", "--extras"])
        .args(extra_args)
        .assert()
        .failure();
    stderr_of(&assert).contains(REFUSAL)
}

fn stdout_of(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

fn renviron(home: &Path) -> String {
    std::fs::read_to_string(home.join(".Renviron")).unwrap_or_default()
}

// ---- lib ----

#[test]
fn the_default_lib_is_under_the_home_directory() {
    let home = Scratch::new("default-lib");
    let assert = baler(home.path()).args(["config", "lib"]).assert().success();
    let expected = home.path().join(".baler").join("modules");
    assert_eq!(stdout_of(&assert).trim(), expected.display().to_string());
}

#[test]
fn the_config_file_sets_the_lib() {
    let home = Scratch::new("file-lib");
    let lib = home.path().join("from-file");
    write_config(home.path(), &format!("lib = {:?}\n", lib.to_str().unwrap()));
    let assert = baler(home.path()).args(["config", "lib"]).assert().success();
    assert_eq!(stdout_of(&assert).trim(), lib.display().to_string());
}

#[test]
fn the_environment_beats_the_config_file_for_the_lib() {
    let home = Scratch::new("env-lib");
    let from_file = home.path().join("from-file");
    let from_env = home.path().join("from-env");
    write_config(home.path(), &format!("lib = {:?}\n", from_file.to_str().unwrap()));
    let assert = baler(home.path()).env("BALER_LIB", &from_env).args(["config", "lib"]).assert().success();
    assert_eq!(stdout_of(&assert).trim(), from_env.display().to_string());
}

#[test]
fn list_reads_the_lib_that_install_wrote() {
    let home = Scratch::new("list-lib");
    let lib = home.path().join("mods");
    let project = init_project(home.path(), home.path());
    baler(home.path())
        .env("BALER_LIB", &lib)
        .args(["install", "--path", project.to_str().unwrap()])
        .assert()
        .success();
    let assert = baler(home.path()).env("BALER_LIB", &lib).args(["list", "--names-only"]).assert().success();
    assert!(stdout_of(&assert).contains("mymod"));
}

#[test]
fn the_lib_setting_does_not_change_the_lock() {
    let home = Scratch::new("lock");
    let project = init_project(home.path(), home.path());
    let lock = project.join("baler.lock");
    baler(home.path()).env("BALER_LIB", home.path().join("a")).args(["lock", project.to_str().unwrap()]).assert().success();
    let first = std::fs::read_to_string(&lock).unwrap();
    baler(home.path()).env("BALER_LIB", home.path().join("b")).args(["lock", project.to_str().unwrap()]).assert().success();
    assert_eq!(first, std::fs::read_to_string(&lock).unwrap());
}

// ---- bad settings ----

#[test]
fn an_unknown_key_in_the_config_file_is_rejected() {
    let home = Scratch::new("unknown-file-key");
    write_config(home.path(), "libb = \"/x\"\n");
    let assert = baler(home.path()).arg("list").assert().failure();
    assert!(stderr_of(&assert).contains("libb"));
}

#[test]
fn a_misspelled_install_deps_variable_is_rejected() {
    let home = Scratch::new("bad-env");
    let assert = baler(home.path()).env("BALER_INSTALL_DEPS", "maybe").arg("list").assert().failure();
    assert!(stderr_of(&assert).contains("BALER_INSTALL_DEPS"));
}

// ---- install-deps ----

#[test]
fn install_deps_is_off_by_default() {
    let home = Scratch::new("off");
    assert!(refused(&mut baler(home.path()), &[]));
}

#[test]
fn the_flag_turns_it_on() {
    let home = Scratch::new("flags");
    assert!(!refused(&mut baler(home.path()), &["--install-deps"]));
}

#[test]
fn the_environment_turns_it_on() {
    let home = Scratch::new("env-on");
    assert!(!refused(baler(home.path()).env("BALER_INSTALL_DEPS", "true"), &[]));
}

#[test]
fn the_config_file_turns_it_on() {
    let home = Scratch::new("file-on");
    write_config(home.path(), "install-deps = true\n");
    assert!(!refused(&mut baler(home.path()), &[]));
}

#[test]
fn the_environment_beats_the_config_file_for_install_deps() {
    let home = Scratch::new("env-beats-file");
    write_config(home.path(), "install-deps = true\n");
    assert!(refused(baler(home.path()).env("BALER_INSTALL_DEPS", "false"), &[]));
}

#[test]
fn the_flag_beats_the_environment_for_install_deps() {
    let home = Scratch::new("flag-beats-env");
    assert!(!refused(baler(home.path()).env("BALER_INSTALL_DEPS", "false"), &["--install-deps"]));
}

#[test]
fn the_last_of_the_two_flags_wins() {
    let home = Scratch::new("last-flag");
    assert!(!refused(&mut baler(home.path()), &["--no-install-deps", "--install-deps"]));
}

#[test]
fn no_install_deps_conflicts_with_extras() {
    let home = Scratch::new("conflict");
    let assert = baler(home.path())
        .args(["install", "--path", "x", "--extras", "--no-install-deps"])
        .assert()
        .failure();
    assert!(stderr_of(&assert).contains("cannot be used with"));
}

// ---- baler config ----

#[test]
fn config_lists_the_defaults_with_their_origin() {
    let home = Scratch::new("list");
    let plain = baler(home.path()).arg("config").assert().success();
    assert!(stdout_of(&plain).contains("install-deps = false\n"));
    let origins = baler(home.path()).args(["config", "--show-origin"]).assert().success();
    assert!(stdout_of(&origins).contains("install-deps = false  (default)"));
}

#[test]
fn config_sets_and_reads_install_deps() {
    let home = Scratch::new("set-deps");
    baler(home.path()).args(["config", "install-deps", "on"]).assert().success();
    let assert = baler(home.path()).args(["config", "install-deps"]).assert().success();
    assert_eq!(stdout_of(&assert).trim(), "true");
    assert!(!refused(&mut baler(home.path()), &[]));
}

#[test]
fn config_shows_the_file_as_the_origin_after_a_set() {
    let home = Scratch::new("origin");
    baler(home.path()).args(["config", "install-deps", "on"]).assert().success();
    let assert = baler(home.path()).args(["config", "--show-origin"]).assert().success();
    assert!(stdout_of(&assert).contains("config.toml"));
}

#[test]
fn config_lib_writes_the_config_file_and_renviron() {
    let home = Scratch::new("set-lib");
    let lib = home.path().join("mods");
    baler(home.path()).args(["config", "lib", lib.to_str().unwrap()]).assert().success();
    let line = format!("BALER_LIB=\"{}\"", lib.display().to_string().replace('\\', "/"));
    assert!(renviron(home.path()).contains(&line), "{}", renviron(home.path()));
    let assert = baler(home.path()).args(["config", "lib"]).assert().success();
    assert_eq!(stdout_of(&assert).trim(), lib.display().to_string());
}

#[test]
fn config_lib_changes_where_install_puts_modules() {
    let home = Scratch::new("set-lib-install");
    let lib = home.path().join("mods");
    let project = init_project(home.path(), home.path());
    baler(home.path()).args(["config", "lib", lib.to_str().unwrap()]).assert().success();
    baler(home.path()).args(["install", "--path", project.to_str().unwrap()]).assert().success();
    assert!(lib.is_dir());
    let assert = baler(home.path()).args(["list", "--names-only"]).assert().success();
    assert!(stdout_of(&assert).contains("mymod"));
}

#[test]
fn config_lib_keeps_the_rest_of_renviron_and_does_not_duplicate_its_block() {
    let home = Scratch::new("renviron-keep");
    std::fs::write(home.path().join(".Renviron"), "R_LIBS_USER=\"/x\"\n").unwrap();
    baler(home.path()).args(["config", "lib", home.path().join("one").to_str().unwrap()]).assert().success();
    baler(home.path()).args(["config", "lib", home.path().join("two").to_str().unwrap()]).assert().success();
    let text = renviron(home.path());
    assert!(text.starts_with("R_LIBS_USER=\"/x\"\n"));
    assert_eq!(text.matches("BALER_LIB=").count(), 1);
    assert!(text.contains("two"));
}

#[test]
fn config_unset_lib_removes_only_what_baler_wrote() {
    let home = Scratch::new("unset-lib");
    std::fs::write(home.path().join(".Renviron"), "R_LIBS_USER=\"/x\"\n").unwrap();
    baler(home.path()).args(["config", "lib", home.path().join("one").to_str().unwrap()]).assert().success();
    baler(home.path()).args(["config", "--unset", "lib"]).assert().success();
    assert_eq!(renviron(home.path()), "R_LIBS_USER=\"/x\"\n");
    let assert = baler(home.path()).args(["config", "lib"]).assert().success();
    assert_eq!(stdout_of(&assert).trim(), home.path().join(".baler").join("modules").display().to_string());
}

#[test]
fn config_unset_of_a_setting_that_is_not_set_says_so() {
    let home = Scratch::new("unset-missing");
    let assert = baler(home.path()).args(["config", "--unset", "install-deps"]).assert().success();
    assert!(stdout_of(&assert).contains("is not set"));
}

#[test]
fn config_warns_when_the_environment_wins() {
    let home = Scratch::new("warn");
    let assert = baler(home.path())
        .env("BALER_INSTALL_DEPS", "false")
        .args(["config", "install-deps", "on"])
        .assert()
        .success();
    assert!(stderr_of(&assert).contains("BALER_INSTALL_DEPS"));
}

#[test]
fn config_names_the_known_settings_for_an_unknown_one() {
    let home = Scratch::new("unknown-key");
    let assert = baler(home.path()).args(["config", "nope"]).assert().failure();
    let err = stderr_of(&assert);
    assert!(err.contains("install-deps") && err.contains("lib"), "{err}");
    assert!(!home.path().join(".baler").join("config.toml").exists());
}

#[test]
fn config_rejects_a_bad_value_without_writing() {
    let home = Scratch::new("bad-value");
    baler(home.path()).args(["config", "install-deps", "maybe"]).assert().failure();
    assert!(!home.path().join(".baler").join("config.toml").exists());
}
