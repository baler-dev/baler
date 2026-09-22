use baler_core::baler_toml::BalerToml;
use baler_core::formats::tar;
use baler_core::lockfile::{BalerLock, LockedPackage};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

fn unique_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("baler-fmt-test-{label}-{n}-{}", std::process::id()))
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

/// Build a small fixture module source tree:
/// <src>/__init__.R
/// <src>/decomp/helpers.R
/// <src>/.hidden_file   (should be excluded from the bundle)
fn make_fixture_src(dir: &Path) {
    std::fs::create_dir_all(dir.join("decomp")).unwrap();
    std::fs::write(dir.join("__init__.R"), "#' @export\nbox::use()\n").unwrap();
    std::fs::write(dir.join("decomp").join("helpers.R"), "helper <- function() 1\n").unwrap();
    std::fs::write(dir.join(".hidden_file"), "should not be bundled").unwrap();
}

/// Build a fixture *project*: a real baler.toml at `root`, plus a
/// `root/src/` module source tree. Two separate directories, not one,
/// deliberately — bundle() now reads baler.toml off disk at the
/// project root, so project_root and src_path have to be genuinely
/// distinct for a collision test (below) to mean anything.
fn make_fixture_project(root: &Path, baler_toml_contents: &str) -> PathBuf {
    std::fs::write(root.join("baler.toml"), baler_toml_contents).unwrap();
    let src = root.join("src");
    make_fixture_src(&src);
    src
}

fn fixture_baler_toml(name: &str) -> String {
    format!(
        r#"[project]
name = "{name}"
version = "0.1.0"
description = "A test module"
authors = [
    "Jane Doe",
    {{ name = "John Smith", email = "john@example.com" }},
]
license = "MIT"
r_version = "4.0.0"

[project.dependencies]
dplyr = "*"

[tool.test]
framework = "testthat"
dir = "tests"
"#
    )
}

fn fixture_lock_contents() -> String {
    let lock = BalerLock {
        version: 1,
        r_version: None,
        packages: vec![LockedPackage {
            name: "dplyr".to_owned(),
            version: "1.1.4".to_owned(),
            repo: "https://cloud.r-project.org".to_owned(),
        }],
    };
    ::toml::to_string_pretty(&lock).unwrap()
}

#[test]
fn tar_bundle_and_unpack_round_trip() {
    let project = Scratch::new("tar-project");
    let src = make_fixture_project(project.path(), &fixture_baler_toml("mymod"));
    std::fs::write(project.path().join("baler.lock"), fixture_lock_contents()).unwrap();

    let files = tar::collect_files(&src).unwrap();
    // Hidden files must not be picked up for the bundled file list.
    assert!(!files.iter().any(|f| f.contains(".hidden_file")));

    let archive = Scratch::new("tar-archive");
    let archive_path = archive.path().join("mymod_0.1.0.tar.gz");

    tar::bundle(&src, project.path(), &archive_path, "mymod", "0.1.0", &[], &[]).unwrap();
    assert!(archive_path.is_file());

    let install = Scratch::new("tar-install");
    tar::unpack(&archive_path, install.path(), "mymod", "0.1.0").unwrap();

    // Module files land directly under <install_dir>/<name>/...
    assert!(install.path().join("mymod").join("__init__.R").is_file());
    assert!(install.path().join("mymod").join("decomp").join("helpers.R").is_file());
    // The hidden file was never bundled, so it can't appear in the install.
    assert!(!install.path().join("mymod").join(".hidden_file").exists());

    // baler.toml and baler.lock land in the dist-info dir, not inside
    // the module dir.
    let dist_info = install.path().join("mymod-0.1.0.dist-info");
    assert!(dist_info.join("baler.toml").is_file());
    assert!(dist_info.join("baler.lock").is_file());
    assert!(!install.path().join("mymod").join("baler.toml").exists());

    let toml_text = std::fs::read_to_string(dist_info.join("baler.toml")).unwrap();
    let read_back: BalerToml = ::toml::from_str(&toml_text).unwrap();
    assert_eq!(read_back.project.name, "mymod");
    assert!(read_back.project.dependencies.packages.contains_key("dplyr"));

    let lock_text = std::fs::read_to_string(dist_info.join("baler.lock")).unwrap();
    let locked: BalerLock = ::toml::from_str(&lock_text).unwrap();
    assert_eq!(locked.packages[0].name, "dplyr");
    assert_eq!(locked.packages[0].version, "1.1.4");
}

#[test]
fn tar_read_toml_reads_embedded_baler_toml() {
    let project = Scratch::new("tar-readtoml-project");
    let src = make_fixture_project(project.path(), &fixture_baler_toml("readback"));

    let archive = Scratch::new("tar-readtoml-archive");
    let archive_path = archive.path().join("readback_0.1.0.tar.gz");
    tar::bundle(&src, project.path(), &archive_path, "readback", "0.1.0", &[], &[]).unwrap();

    let toml = tar::read_toml(&archive_path).unwrap();
    assert_eq!(toml.project.name, "readback");
    assert_eq!(toml.project.version, "0.1.0");
    assert_eq!(toml.project.license, "MIT");
    assert!(toml.project.dependencies.packages.contains_key("dplyr"));

    let test_cfg = toml.tool.and_then(|t| t.test)
        .expect("test config should survive the round trip");
    assert_eq!(test_cfg.framework, "testthat");
    assert_eq!(test_cfg.dir.as_deref(), Some("tests"));

    let john = toml.project.authors.iter().find(|a| a.name() == "John Smith")
        .expect("Extended author should survive the round trip");
    assert_eq!(john.email(), Some("john@example.com"));
}

#[test]
fn tar_user_file_named_baler_toml_survives_bundling() {
    let project = Scratch::new("tar-collision-project");
    let src = make_fixture_project(project.path(), &fixture_baler_toml("collisionmod"));
    // A module source file that happens to share a name with the
    // project's own baler.toml. It lives inside the module's src dir,
    // not at the project root, so it must not collide with the real
    // one on write, and must not be misrouted into .dist-info on unpack.
    std::fs::write(src.join("baler.toml"), "user's own data, not baler's").unwrap();

    let archive = Scratch::new("tar-collision-archive");
    let archive_path = archive.path().join("collisionmod_0.1.0.tar.gz");
    tar::bundle(&src, project.path(), &archive_path, "collisionmod", "0.1.0", &[], &[]).unwrap();

    let install = Scratch::new("tar-collision-install");
    tar::unpack(&archive_path, install.path(), "collisionmod", "0.1.0").unwrap();

    // The user's file is installed as ordinary module content, untouched.
    let user_file = install.path().join("collisionmod").join("baler.toml");
    assert_eq!(std::fs::read_to_string(&user_file).unwrap(), "user's own data, not baler's");

    // The project's real baler.toml still lands in .dist-info, and is still valid.
    let dist_info = install.path().join("collisionmod-0.1.0.dist-info");
    let real_toml = std::fs::read_to_string(dist_info.join("baler.toml")).unwrap();
    let parsed: BalerToml = ::toml::from_str(&real_toml).unwrap();
    assert_eq!(parsed.project.name, "collisionmod");
}

#[test]
fn tar_read_toml_errors_on_non_baler_archive() {
    let scratch = Scratch::new("tar-not-baler");
    let not_baler = scratch.path().join("plain.tar.gz");

    // A tarball with no baler.toml inside at all.
    let file = std::fs::File::create(&not_baler).unwrap();
    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut archive = ::tar::Builder::new(enc);
    let readme = scratch.path().join("README.md");
    std::fs::write(&readme, "hello").unwrap();
    archive.append_path_with_name(&readme, "top/README.md").unwrap();
    archive.finish().unwrap();

    assert!(tar::read_toml(&not_baler).is_err());
}
