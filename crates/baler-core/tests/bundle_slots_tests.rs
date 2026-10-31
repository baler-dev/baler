use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

use baler_core::formats::tar::{bundle_with_binary, merge_bundles, platform_tag, unpack, BundleSpec};

const LINUX: &str = "x86_64-pc-linux-gnu_r4.4";
const MAC: &str = "aarch64-apple-darwin20_r4.4";

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str) -> Project {
        let root = std::env::temp_dir().join(format!("baler-slots-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("foo").join("src").join(".lib")).unwrap();
        std::fs::write(root.join("baler.toml"), "[project]\nname = \"foo\"\nversion = \"1.0.0\"\n").unwrap();
        std::fs::write(root.join("foo").join("__init__.R"), "x = 1\n").unwrap();
        Project { root }
    }

    fn lib(&self) -> PathBuf {
        self.root.join("foo").join("src").join(".lib")
    }

    fn archive(&self) -> PathBuf {
        self.root.join("foo_1.0.0.tar.gz")
    }

    /// What `baler compile` leaves in `.lib/` on the current machine.
    fn compile(&self, content: &str) {
        std::fs::write(self.lib().join("cpp.so"), content).unwrap();
        std::fs::write(self.lib().join("cpp.so.abi.json"), "{}\n").unwrap();
    }

    fn edit_source(&self, content: &str) {
        std::fs::write(self.root.join("foo").join("__init__.R"), content).unwrap();
    }

    fn bundle(&self, tag: &str) -> anyhow::Result<Vec<String>> {
        self.bundle_to(tag, &self.archive())
    }

    fn bundle_to(&self, tag: &str, archive: &std::path::Path) -> anyhow::Result<Vec<String>> {
        let src = self.root.join("foo");
        let force_include = vec![self.lib()];
        bundle_with_binary(
            &BundleSpec {
                src_path: &src,
                project_root: &self.root,
                output_path: archive,
                name: "foo",
                version: "1.0.0",
                exclude: &[],
                force_include: &force_include,
            },
            tag,
        )
    }

    /// What `baler bundle --merge` passes: the module's files and no `.lib/`.
    fn merge(&self, inputs: &[PathBuf]) -> anyhow::Result<Vec<String>> {
        let src = self.root.join("foo");
        let archive = self.archive();
        merge_bundles(
            &BundleSpec {
                src_path: &src,
                project_root: &self.root,
                output_path: &archive,
                name: "foo",
                version: "1.0.0",
                exclude: &[],
                force_include: &[],
            },
            inputs,
        )
    }

    /// A single-platform archive of the current files, as one CI job makes.
    fn platform_archive(&self, file: &str, tag: &str, content: &str) -> PathBuf {
        self.compile(content);
        let path = self.root.join(file);
        self.bundle_to(tag, &path).unwrap();
        path
    }

    fn entries(&self) -> BTreeSet<String> {
        let gz = flate2::read::GzDecoder::new(File::open(self.archive()).unwrap());
        let mut archive = tar::Archive::new(gz);
        archive
            .entries()
            .unwrap()
            .map(|entry| entry.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    fn read(&self, name: &str) -> String {
        let gz = flate2::read::GzDecoder::new(File::open(self.archive()).unwrap());
        let mut archive = tar::Archive::new(gz);
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.path().unwrap().to_string_lossy() == name {
                let mut text = String::new();
                entry.read_to_string(&mut text).unwrap();
                return text;
            }
        }
        panic!("{name} is not in the archive");
    }
}

fn binary_entry(tag: &str) -> String {
    format!("foo_1.0.0/foo/src/.lib/{tag}/cpp.so")
}

#[test]
fn first_bundle_stores_the_binary_under_its_platform() {
    let project = Project::new("first");
    project.compile("linux build");

    assert_eq!(project.bundle(LINUX).unwrap(), vec![LINUX.to_owned()]);

    let entries = project.entries();
    assert!(entries.contains(&binary_entry(LINUX)));
    assert!(entries.contains(&format!("foo_1.0.0/foo/src/.lib/{LINUX}/cpp.so.abi.json")));
    assert!(!entries.contains("foo_1.0.0/foo/src/.lib/cpp.so"));
}

#[test]
fn second_platform_is_added_to_the_same_archive() {
    let project = Project::new("second");
    project.compile("linux build");
    project.bundle(LINUX).unwrap();

    project.compile("mac build");
    let tags = project.bundle(MAC).unwrap();

    assert_eq!(tags, vec![MAC.to_owned(), LINUX.to_owned()]);
    assert_eq!(project.read(&binary_entry(LINUX)), "linux build");
    assert_eq!(project.read(&binary_entry(MAC)), "mac build");
    assert!(project.entries().contains("foo_1.0.0/baler.toml"));
}

#[test]
fn bundling_the_same_platform_again_replaces_its_binary() {
    let project = Project::new("replace");
    project.compile("first build");
    project.bundle(LINUX).unwrap();

    project.compile("second build");
    assert_eq!(project.bundle(LINUX).unwrap(), vec![LINUX.to_owned()]);

    assert_eq!(project.read(&binary_entry(LINUX)), "second build");
}

#[test]
fn changed_source_is_refused_when_another_platform_would_be_lost() {
    let project = Project::new("refused");
    project.compile("linux build");
    project.bundle(LINUX).unwrap();
    project.compile("mac build");
    project.bundle(MAC).unwrap();
    let before = std::fs::read(project.archive()).unwrap();

    project.edit_source("x = 2\n");
    let err = project.bundle(LINUX).unwrap_err().to_string();

    assert!(err.contains(MAC), "{err}");
    assert_eq!(std::fs::read(project.archive()).unwrap(), before);
}

#[test]
fn changed_source_replaces_an_archive_with_only_this_platform() {
    let project = Project::new("only-self");
    project.compile("linux build");
    project.bundle(LINUX).unwrap();

    project.edit_source("x = 2\n");
    assert_eq!(project.bundle(LINUX).unwrap(), vec![LINUX.to_owned()]);
    assert_eq!(project.read("foo_1.0.0/foo/__init__.R"), "x = 2\n");
}

#[test]
fn an_unreadable_archive_is_replaced() {
    let project = Project::new("unreadable");
    project.compile("linux build");
    std::fs::write(project.archive(), b"not a gzip file").unwrap();

    assert_eq!(project.bundle(LINUX).unwrap(), vec![LINUX.to_owned()]);
    assert!(project.entries().contains(&binary_entry(LINUX)));
}

#[test]
fn no_partial_file_is_left_behind() {
    let project = Project::new("partial");
    project.compile("linux build");
    project.bundle(LINUX).unwrap();

    let partial = project.root.join("foo_1.0.0.tar.gz.partial");
    assert!(!partial.exists());
}

#[test]
fn unpack_puts_each_platform_in_its_own_folder() {
    let project = Project::new("unpack");
    project.compile("linux build");
    project.bundle(LINUX).unwrap();
    project.compile("mac build");
    project.bundle(MAC).unwrap();

    let install = project.root.join("install");
    unpack(&project.archive(), &install, "foo", "1.0.0").unwrap();

    let lib = install.join("foo").join("src").join(".lib");
    assert_eq!(std::fs::read_to_string(lib.join(LINUX).join("cpp.so")).unwrap(), "linux build");
    assert_eq!(std::fs::read_to_string(lib.join(MAC).join("cpp.so")).unwrap(), "mac build");
}

#[test]
fn platform_tag_is_a_single_safe_path_component() {
    assert_eq!(platform_tag("x86_64-pc-linux-gnu", "4.4"), LINUX);
    let odd = platform_tag("a/b c", "4.4\\x");
    assert!(!odd.contains('/') && !odd.contains('\\') && !odd.contains(' '), "{odd}");
}

#[test]
fn merge_combines_single_platform_archives() {
    let project = Project::new("merge");
    let linux = project.platform_archive("linux.tar.gz", LINUX, "linux build");
    let mac = project.platform_archive("mac.tar.gz", MAC, "mac build");

    let tags = project.merge(&[linux, mac]).unwrap();

    assert_eq!(tags, vec![MAC.to_owned(), LINUX.to_owned()]);
    assert_eq!(project.read(&binary_entry(LINUX)), "linux build");
    assert_eq!(project.read(&binary_entry(MAC)), "mac build");
    assert!(project.entries().contains("foo_1.0.0/baler.toml"));
    assert!(project.entries().contains("foo_1.0.0/foo/__init__.R"));
}

#[test]
fn merge_may_replace_one_of_its_inputs() {
    let project = Project::new("merge-in-place");
    project.compile("linux build");
    project.bundle(LINUX).unwrap();
    let mac = project.platform_archive("mac.tar.gz", MAC, "mac build");

    project.merge(&[project.archive(), mac]).unwrap();

    assert_eq!(project.read(&binary_entry(LINUX)), "linux build");
    assert_eq!(project.read(&binary_entry(MAC)), "mac build");
}

#[test]
fn merge_refuses_an_archive_made_from_other_files() {
    let project = Project::new("merge-stale");
    let linux = project.platform_archive("linux.tar.gz", LINUX, "linux build");
    project.edit_source("x = 2\n");
    let mac = project.platform_archive("mac.tar.gz", MAC, "mac build");

    let err = project.merge(&[linux, mac]).unwrap_err().to_string();

    assert!(err.contains("linux.tar.gz"), "{err}");
    assert!(!project.archive().exists());
}

#[test]
fn merge_refuses_a_platform_that_two_archives_hold() {
    let project = Project::new("merge-duplicate");
    let first = project.platform_archive("first.tar.gz", LINUX, "one");
    let second = project.platform_archive("second.tar.gz", LINUX, "two");

    let err = project.merge(&[first, second]).unwrap_err().to_string();

    assert!(err.contains(LINUX), "{err}");
    assert!(err.contains("first.tar.gz") && err.contains("second.tar.gz"), "{err}");
}

#[test]
fn merge_refuses_archives_without_binaries() {
    let project = Project::new("merge-empty");
    project.compile("linux build");
    let plain = project.root.join("plain.tar.gz");
    // An archive of the same files made without `.lib/`.
    let src = project.root.join("foo");
    baler_core::formats::tar::bundle(&src, &project.root, &plain, "foo", "1.0.0", &[], &[]).unwrap();

    let err = project.merge(&[plain]).unwrap_err().to_string();

    assert!(err.contains("binary"), "{err}");
}

#[test]
fn merge_fails_on_an_unreadable_input_and_keeps_the_existing_archive() {
    let project = Project::new("merge-unreadable");
    project.compile("linux build");
    project.bundle(LINUX).unwrap();
    let before = std::fs::read(project.archive()).unwrap();
    let broken = project.root.join("broken.tar.gz");
    std::fs::write(&broken, b"not a gzip file").unwrap();

    assert!(project.merge(&[broken]).is_err());
    assert_eq!(std::fs::read(project.archive()).unwrap(), before);
}

#[test]
fn merge_needs_at_least_one_archive() {
    let project = Project::new("merge-none");
    assert!(project.merge(&[]).is_err());
}
