//! Uses the real `convert-proj` example module that ships at the repo
//! root as a fixture, instead of a synthetic one. This is the actual
//! deliverable a user would bundle/install with `baler`, so a passing
//! suite here means the example in the repo genuinely still works with
//! the current baler-core code, not just a stand-in.
//!
//! Values are read from convert-proj's real `baler.toml` at test time
//! (module name, version, ...) rather than hardcoded, so this doesn't
//! silently go stale if that file changes.
//!
//! Assumes examples/modules/convert-proj/baler.toml has already been
//! migrated to the [project] anatomy — this reads whatever the file
//! on disk says, it doesn't hardcode the old shape.

use baler_core::baler_toml::BalerToml;
use baler_core::formats::tar;
use baler_core::ops::install::InstallRequest;
use baler_core::ops::{install, remove};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

// install/remove resolve their target dir from BALER_LIB (env vars are
// process-global); see the same guard pattern in install_lifecycle_tests.rs.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Path to the real convert-proj/ directory at the repo root, resolved
/// relative to this crate (crates/baler-core) rather than the process's
/// current working directory, so `cargo test` works the same no matter
/// where it's invoked from.
fn convert_proj_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/modules/convert-proj")
}

fn unique_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("baler-convertproj-test-{label}-{n}-{}", std::process::id()))
}

struct Scratch(PathBuf);

impl Scratch {
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

struct BalerLibGuard {
    previous: Option<String>,
}

impl BalerLibGuard {
    fn set(path: &Path) -> Self {
        let previous = std::env::var("BALER_LIB").ok();
        unsafe { std::env::set_var("BALER_LIB", path); }
        Self { previous }
    }
}

impl Drop for BalerLibGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(v) => unsafe { std::env::set_var("BALER_LIB", v) },
            None => unsafe { std::env::remove_var("BALER_LIB") },
        }
    }
}

/// Builds the `--path <dir>` shape of an install request — every
/// install in this file is from a local directory, nothing here
/// exercises --git/--url/registry lookups.
fn install_from_path(path: &str) -> InstallRequest {
    InstallRequest {
        source: None,
        repo: None,
        version: None,
        path: Some(path.to_owned()),
        git: None,
        branch: None,
        tag: None,
        rev: None,
        url: None,
        module_dir: None,
    }
}

/// Sanity check that the fixture is actually present before trusting any
/// other test's results, if this fails, the other tests here are
/// meaningless, not passing-by-accident.
#[test]
fn convert_proj_fixture_exists_with_baler_toml() {
    let dir = convert_proj_dir();
    assert!(dir.is_dir(), "convert-proj not found at {}", dir.display());
    assert!(dir.join("baler.toml").is_file());
}

#[test]
fn convert_proj_baler_toml_parses_and_resolves_src() {
    let dir = convert_proj_dir();
    let toml = BalerToml::from_dir(&dir).expect("convert-proj/baler.toml should parse");
    let src = toml.resolve_src_dir(&dir).expect("src dir should resolve");

    // The module's entry point must exist, per baler's own contract.
    assert!(src.join("__init__.R").is_file());
}

#[test]
fn convert_proj_bundles_as_tar_gz_with_matching_metadata() {
    let dir = convert_proj_dir();
    let toml = BalerToml::from_dir(&dir).unwrap();
    let src = toml.resolve_src_dir(&dir).unwrap();

    let archive_dir = Scratch::reserved("tar-archive");
    std::fs::create_dir_all(archive_dir.path()).unwrap();
    let archive_path = archive_dir.path().join("convert.tar.gz");

    let files = tar::collect_files(&src).unwrap();
    assert!(!files.is_empty(), "convert-proj source tree should not be empty");

    tar::bundle(&src, &dir, &archive_path, &toml.project.name, &toml.project.version, &[], &[]).unwrap();
    assert!(archive_path.is_file());

    let read_back = tar::read_toml(&archive_path).unwrap();
    assert_eq!(read_back.project.name, toml.project.name);
    assert_eq!(read_back.project.version, toml.project.version);
}

#[test]
fn convert_proj_installs_via_baler_install_run() {
    let _guard = ENV_LOCK.lock().unwrap();

    let dir = convert_proj_dir();
    let toml = BalerToml::from_dir(&dir).expect("convert-proj/baler.toml should parse");

    let lib = Scratch::reserved("lib");
    let _env = BalerLibGuard::set(lib.path());

    // install_deps = false → dependency install stays a dry run, so this
    // never touches the network regardless of what convert-proj declares
    // under [project.dependencies].
    let req = install_from_path(dir.to_str().unwrap());
    install::run(req, false).expect("installing convert-proj should succeed");

    let module_dir = lib.path().join(&toml.project.name);
    assert!(module_dir.join("__init__.R").is_file());

    // The submodules actually present in convert-proj (mass/, temp/) must
    // survive the bundle → install round trip intact.
    assert!(module_dir.join("mass").join("__init__.R").is_file());
    assert!(module_dir.join("mass").join("basic_mass.R").is_file());
    assert!(module_dir.join("mass").join("const.R").is_file());
    assert!(module_dir.join("mass").join("conversions.R").is_file());
    assert!(module_dir.join("mass").join("cross_system_mass.R").is_file());
    assert!(module_dir.join("mass").join("imperial_mass.R").is_file());
    assert!(module_dir.join("mass").join("specialty.R").is_file());

    assert!(module_dir.join("temp").join("__init__.R").is_file());
    assert!(module_dir.join("temp").join("conversions.R").is_file());

    // baler.toml and README.md are project files, not module files,
    // must not leak into the installed tree.
    assert!(!module_dir.join("baler.toml").exists());
    assert!(!module_dir.join("README.md").exists());

    // baler.toml (the shipped source manifest, no longer a generated
    // manifest.json) lands in .dist-info, and is still valid.
    let dist_info = lib.path().join(format!("{}-{}.dist-info", toml.project.name, toml.project.version));
    assert!(dist_info.join("baler.toml").is_file());

    let dist_toml_text = std::fs::read_to_string(dist_info.join("baler.toml")).unwrap();
    let dist_toml: BalerToml = ::toml::from_str(&dist_toml_text).unwrap();
    assert_eq!(dist_toml.project.name, toml.project.name);
    assert_eq!(dist_toml.project.version, toml.project.version);

    remove::run(&toml.project.name, true).expect("removing convert-proj should succeed");
    assert!(!module_dir.exists());
}
