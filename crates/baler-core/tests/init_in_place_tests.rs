//! `baler init` with no name: initializing a directory that already
//! exists. An empty directory is scaffolded; any other gets `baler.toml`
//! and nothing else, and no code in it is ever read. Every project here
//! is `<unique temp dir>/<dir_name>`, so the directory's own name (which
//! the module name is worked out from) is under each test's control, and
//! no test touches the working directory.

use baler_core::baler_toml::BalerToml;
use baler_core::ops::init::{run_in_place, InPlaceOptions};
use baler_native::NativeLang;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

struct Project {
    parent: PathBuf,
    root: PathBuf,
}

impl Project {
    fn new(dir_name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let parent = std::env::temp_dir()
            .join(format!("baler-in-place-{}-{n}", std::process::id()));
        let root = parent.join(dir_name);
        fs::create_dir_all(&root).unwrap();
        Self { parent, root }
    }

    fn write(&self, rel: &str, contents: &str) {
        let path = self.root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.root.join(rel)).unwrap()
    }

    fn exists(&self, rel: &str) -> bool {
        self.root.join(rel).exists()
    }

    /// Every file under the project, as `/`-separated relative paths.
    fn files(&self) -> BTreeSet<String> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(root, &path, out);
                } else {
                    let rel = path.strip_prefix(root).unwrap();
                    out.insert(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let mut out = BTreeSet::new();
        walk(&self.root, &self.root, &mut out);
        out
    }

    fn init(&self) -> anyhow::Result<()> {
        run_in_place(&self.root, &InPlaceOptions::default())
    }

    fn init_with(&self, opts: InPlaceOptions) -> anyhow::Result<()> {
        run_in_place(&self.root, &opts)
    }

    fn manifest(&self) -> BalerToml {
        BalerToml::from_dir(&self.root).expect("generated baler.toml should load")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.parent);
    }
}

fn error_text(result: anyhow::Result<()>) -> String {
    format!("{:#}", result.expect_err("init should have been rejected"))
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

// ---- empty directory: scaffold ----

#[test]
fn empty_dir_is_scaffolded_with_the_name_minus_proj() {
    let p = Project::new("weather-proj");
    p.init().unwrap();

    assert_eq!(
        p.files(),
        set(&[
            "baler.toml",
            "README.md",
            "weather/__init__.r",
            "weather/hello.r",
            "weather/add.r",
        ])
    );
    let toml = p.manifest();
    assert_eq!(toml.project.name, "weather");
    assert_eq!(toml.project.src, None);
    assert_eq!(toml.resolve_src_dir(&p.root).unwrap(), p.root.join("weather"));
    assert!(p.read("README.md").starts_with("# weather"));
}

#[test]
fn empty_dir_uses_its_own_name() {
    let p = Project::new("mymod");
    p.init().unwrap();
    assert_eq!(p.manifest().project.name, "mymod");
    assert!(p.exists("mymod/__init__.r"));
}

#[test]
fn hidden_entries_do_not_make_a_directory_non_empty() {
    // A fresh `git init` directory is still a fresh project.
    let p = Project::new("mymod");
    p.write(".git/HEAD", "ref: refs/heads/main\n");
    p.write(".gitignore", "*.so\n");
    p.init().unwrap();
    assert!(p.exists("mymod/__init__.r"), "should have scaffolded");
    assert_eq!(p.read(".gitignore"), "*.so\n");
}

#[test]
fn native_flag_scaffolds_an_empty_directory() {
    let p = Project::new("calc-proj");
    p.init_with(InPlaceOptions { native: Some(NativeLang::Cpp), ..Default::default() }).unwrap();

    assert!(p.exists("calc/src/Makevars"));
    assert!(p.exists("calc/hook.r"));
    let toml = p.read("baler.toml");
    assert!(toml.contains("[compiled-code]"));
    assert!(toml.contains("build_deps = { Rcpp = \"*\" }"));
    assert!(p.manifest().compiled_code.is_some());
}

// ---- any other directory: baler.toml only ----

#[test]
fn non_empty_directory_gets_a_manifest_and_nothing_else() {
    let p = Project::new("analysis");
    p.write("R/clean.R", "clean <- function() 1\n");
    p.write("scripts/run.R", "source('R/clean.R')\n");
    let before = p.files();

    p.init().unwrap();

    let added: BTreeSet<String> = p.files().difference(&before).cloned().collect();
    assert_eq!(added, set(&["baler.toml"]));
    assert_eq!(p.read("R/clean.R"), "clean <- function() 1\n");
    assert!(!p.exists("analysis"), "no example module may appear");
    assert!(!p.exists("README.md"), "no README is added to an existing project");

    let toml = p.manifest();
    assert_eq!(toml.project.name, "analysis");
    assert_eq!(toml.project.src, None);
}

#[test]
fn src_flag_points_the_manifest_at_a_directory() {
    let p = Project::new("analysis");
    p.write("R/clean.R", "");
    p.init_with(InPlaceOptions { src: Some("R"), ..Default::default() }).unwrap();

    assert_eq!(p.files(), set(&["R/clean.R", "baler.toml"]));
    let toml = p.manifest();
    assert_eq!(toml.project.src.as_deref(), Some("R"));
    assert_eq!(toml.project.name, "analysis");
}

#[test]
fn src_flag_accepts_a_nested_directory() {
    let p = Project::new("analysis");
    p.write("inst/mymod/__init__.r", "");
    p.init_with(InPlaceOptions { src: Some("inst/mymod"), ..Default::default() }).unwrap();
    let toml = p.manifest();
    assert_eq!(toml.project.src.as_deref(), Some("inst/mymod"));
    assert_eq!(toml.resolve_src_dir(&p.root).unwrap(), p.root.join("inst").join("mymod"));
}

#[test]
fn existing_module_named_like_the_project_needs_no_flags() {
    // The shape of examples/modules/convert-proj.
    let p = Project::new("convert-proj");
    p.write("convert/__init__.R", "box::use(./mass)\n");
    p.write("convert/mass/__init__.R", "# nested module\n");
    p.write("README.md", "mine\n");
    let before = p.files();

    p.init().unwrap();

    let added: BTreeSet<String> = p.files().difference(&before).cloned().collect();
    assert_eq!(added, set(&["baler.toml"]));
    assert_eq!(p.read("convert/__init__.R"), "box::use(./mass)\n");
    assert_eq!(p.read("README.md"), "mine\n");

    let toml = p.manifest();
    assert_eq!(toml.project.name, "convert");
    assert_eq!(toml.project.src, None);
    assert_eq!(toml.resolve_src_dir(&p.root).unwrap(), p.root.join("convert"));
}

#[test]
fn name_and_src_flags_together() {
    let p = Project::new("whatever");
    p.write("lib/__init__.r", "");
    p.init_with(InPlaceOptions { name: Some("other"), src: Some("lib"), ..Default::default() })
        .unwrap();

    let toml = p.manifest();
    assert_eq!(toml.project.name, "other");
    assert_eq!(toml.project.src.as_deref(), Some("lib"));
    assert!(!p.exists("other"), "no new source directory should appear");
}

#[test]
fn src_that_does_not_exist_yet_still_writes_the_manifest_with_the_default() {
    // Nothing is looked for, so a missing default `src` is only a note.
    let p = Project::new("analysis");
    p.write("notes.txt", "x\n");
    p.init().unwrap();
    assert!(p.exists("baler.toml"));
    assert!(!p.exists("analysis"));
}

// ---- names ----

#[test]
fn invalid_directory_name_is_rejected_before_anything_is_written() {
    let p = Project::new("my-project");
    let err = error_text(p.init());
    assert!(err.contains("--name"), "{err}");
    assert!(p.files().is_empty(), "nothing should have been written");
}

#[test]
fn name_flag_overrides_an_invalid_directory_name() {
    let p = Project::new("my-project");
    p.init_with(InPlaceOptions { name: Some("mymod"), ..Default::default() }).unwrap();
    assert_eq!(p.manifest().project.name, "mymod");
    assert!(p.exists("mymod/__init__.r"));
}

#[test]
fn invalid_name_flag_is_rejected() {
    let p = Project::new("fine");
    let err = error_text(p.init_with(InPlaceOptions { name: Some("bad-name"), ..Default::default() }));
    assert!(err.contains("not a valid module name"), "{err}");
    assert!(p.files().is_empty());
}

// ---- refusals ----

#[test]
fn src_flag_must_stay_inside_the_project() {
    let p = Project::new("proj");
    p.write("mymod/__init__.r", "");
    for bad in ["..", "../x", "/tmp", "."] {
        let err = error_text(p.init_with(InPlaceOptions { src: Some(bad), ..Default::default() }));
        assert!(err.contains("--src"), "{bad}: {err}");
    }
    let err = error_text(p.init_with(InPlaceOptions { src: Some("nope"), ..Default::default() }));
    assert!(err.contains("not a directory"), "{err}");
    assert!(!p.exists("baler.toml"));
}

#[test]
fn existing_baler_toml_is_never_overwritten() {
    let p = Project::new("mymod");
    p.write("baler.toml", "# mine\n");
    let err = error_text(p.init());
    assert!(err.contains("already exists"), "{err}");
    assert_eq!(p.read("baler.toml"), "# mine\n");
    assert_eq!(p.files(), set(&["baler.toml"]));
}

#[test]
fn native_flag_is_rejected_in_a_non_empty_directory() {
    let p = Project::new("mymod");
    p.write("mymod/__init__.r", "mine\n");
    let err = error_text(
        p.init_with(InPlaceOptions { native: Some(NativeLang::Cpp), ..Default::default() }),
    );
    assert!(err.contains("--native"), "{err}");
    assert_eq!(p.read("mymod/__init__.r"), "mine\n");
    assert!(!p.exists("baler.toml"));
}

#[test]
fn a_failed_run_leaves_the_directory_ready_for_another_attempt() {
    // Fortran can't be scaffolded. The failure must not leave a baler.toml
    // behind, or the retry would be refused.
    let p = Project::new("mymod");
    let err = error_text(
        p.init_with(InPlaceOptions { native: Some(NativeLang::Fortran), ..Default::default() }),
    );
    assert!(err.contains("Fortran"), "{err}");
    assert!(!p.exists("baler.toml"));
    assert!(!p.exists("mymod"), "the half-made module directory must be removed");
    p.init().expect("a retry without --native should work");
    assert!(p.exists("mymod/__init__.r"), "and it should scaffold, the project being empty again");
}
