use baler_core::formats::tar;
use baler_core::installed;
use baler_core::ops::{list, show};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn module_toml(name: &str, version: &str, license: &str) -> String {
    format!(
        r#"[project]
name = "{name}"
version = "{version}"
description = "The {name} module"
authors = [
    "Jane Doe",
    {{ name = "John Smith", email = "john@example.com" }},
]
license = "{license}"
r_version = ">=4.0.0"
keywords = ["one", "two"]

[dependencies]
dplyr = "*"
cli = ">=3.0.0"

[dependencies.baler]
other = {{ git = "https://github.com/user/other", version = ">=1.0.0" }}
"#
    )
}

/// Puts a module in `install_dir` the way an install leaves it: the
/// module directory, and a `.dist-info` folder holding baler.toml.
fn fake_install(install_dir: &Path, name: &str, version: &str, license: &str) -> PathBuf {
    let module = install_dir.join(name);
    std::fs::create_dir_all(&module).unwrap();
    std::fs::write(module.join("__init__.r"), "box::use()\n").unwrap();
    let dist_info = install_dir.join(format!("{name}-{version}.dist-info"));
    std::fs::create_dir_all(&dist_info).unwrap();
    std::fs::write(dist_info.join("baler.toml"), module_toml(name, version, license)).unwrap();
    dist_info
}

fn set_written(path: &Path, when: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

fn names(dirs: &[PathBuf]) -> Vec<String> {
    let mut found: Vec<String> = dirs
        .iter()
        .map(|d| d.file_name().unwrap().to_str().unwrap().to_owned())
        .collect();
    found.sort();
    found
}

// ---- module names ----

#[test]
fn plain_module_names_are_accepted() {
    for name in ["rmod", "foo-bar", "foo_bar", "Foo.Bar", "m2"] {
        installed::validate_module_name(name).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn names_that_leave_the_install_directory_are_refused() {
    for name in ["", ".", "..", "a/b", "../x", "/etc", "./rmod", "rmod/", "a\\..\\b"] {
        let attempt = installed::validate_module_name(name);
        // A backslash is only a separator on Windows. Elsewhere it is an
        // ordinary character, and a name holding one stays inside the
        // install directory, so it is not worth refusing there.
        if name.contains('\\') && !cfg!(windows) {
            continue;
        }
        assert!(attempt.is_err(), "'{name}' should be refused");
    }
}

// ---- dist-info folders ----

#[test]
fn dist_info_dirs_for_does_not_mix_up_names_that_share_a_prefix() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "foo", "1.0.0", "MIT");
    fake_install(dir.path(), "foo-bar", "2.0.0", "MIT");

    let foo = installed::dist_info_dirs_for(dir.path(), "foo").unwrap();
    let foo_bar = installed::dist_info_dirs_for(dir.path(), "foo-bar").unwrap();

    assert_eq!(names(&foo), ["foo-1.0.0.dist-info"]);
    assert_eq!(names(&foo_bar), ["foo-bar-2.0.0.dist-info"]);
}

#[test]
fn pre_release_versions_are_read_whole_and_not_mistaken_for_part_of_the_name() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "rmod", "0.3.0-dev", "MIT");
    fake_install(dir.path(), "foo-bar", "1.0.0-rc.1", "MIT");
    fake_install(dir.path(), "foo", "2.0.0", "MIT");

    // Each name finds exactly its own folder.
    let rmod = installed::dist_info_dirs_for(dir.path(), "rmod").unwrap();
    let foo_bar = installed::dist_info_dirs_for(dir.path(), "foo-bar").unwrap();
    let foo = installed::dist_info_dirs_for(dir.path(), "foo").unwrap();
    assert_eq!(names(&rmod), ["rmod-0.3.0-dev.dist-info"]);
    assert_eq!(names(&foo_bar), ["foo-bar-1.0.0-rc.1.dist-info"]);
    assert_eq!(names(&foo), ["foo-2.0.0.dist-info"]);

    // And a listing reads each one as the module it is.
    let listing = installed::list(dir.path()).unwrap();
    let found: Vec<(&str, &str)> = listing
        .modules
        .iter()
        .map(|m| (m.name.as_str(), m.version.as_str()))
        .collect();
    assert_eq!(found, [("foo", "2.0.0"), ("foo-bar", "1.0.0-rc.1"), ("rmod", "0.3.0-dev")]);
}

#[test]
fn a_dev_version_is_cleaned_up_when_the_release_replaces_it() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "rmod", "0.3.0-dev", "MIT");

    assert_eq!(installed::remove_dist_infos(dir.path(), "rmod").unwrap(), 1);
    assert!(installed::dist_info_dirs_for(dir.path(), "rmod").unwrap().is_empty());
}

#[test]
fn remove_dist_infos_takes_every_version_of_that_module_only() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "foo", "1.0.0", "MIT");
    fake_install(dir.path(), "foo", "2.0.0", "MIT");
    fake_install(dir.path(), "foo-bar", "1.0.0", "MIT");

    let removed = installed::remove_dist_infos(dir.path(), "foo").unwrap();

    assert_eq!(removed, 2);
    assert!(installed::dist_info_dirs_for(dir.path(), "foo").unwrap().is_empty());
    assert_eq!(installed::dist_info_dirs_for(dir.path(), "foo-bar").unwrap().len(), 1);
    assert!(dir.path().join("foo").is_dir(), "the module directory is not its business");
}

#[test]
fn a_missing_install_directory_holds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("never-created");

    assert!(installed::dist_info_dirs_for(&missing, "foo").unwrap().is_empty());
    assert_eq!(installed::remove_dist_infos(&missing, "foo").unwrap(), 0);
    let listing = installed::list(&missing).unwrap();
    assert!(listing.modules.is_empty());
    assert!(listing.problems.is_empty());
}

// ---- listing ----

#[test]
fn list_is_sorted_by_name_and_reads_each_modules_record() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "zeta", "0.3.0", "MIT");
    fake_install(dir.path(), "alpha", "1.2.3", "Apache-2.0");

    let listing = installed::list(dir.path()).unwrap();

    let found: Vec<(&str, &str, &str)> = listing
        .modules
        .iter()
        .map(|m| (m.name.as_str(), m.version.as_str(), m.toml.project.license.as_str()))
        .collect();
    assert_eq!(found, [("alpha", "1.2.3", "Apache-2.0"), ("zeta", "0.3.0", "MIT")]);
}

#[test]
fn list_ignores_a_dist_info_folder_with_no_module_directory() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "kept", "1.0.0", "MIT");
    // What baler 0.1.0 left behind after `baler remove`.
    let ghost = fake_install(dir.path(), "ghost", "1.0.0", "MIT");
    std::fs::remove_dir_all(dir.path().join("ghost")).unwrap();
    assert!(ghost.is_dir());

    let listing = installed::list(dir.path()).unwrap();

    let found: Vec<&str> = listing.modules.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(found, ["kept"]);
}

#[test]
fn list_ignores_a_module_directory_baler_has_no_record_of() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "kept", "1.0.0", "MIT");
    std::fs::create_dir_all(dir.path().join("handmade")).unwrap();

    let listing = installed::list(dir.path()).unwrap();

    let found: Vec<&str> = listing.modules.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(found, ["kept"]);
}

#[test]
fn list_uses_the_most_recently_written_dist_info_when_old_versions_linger() {
    let dir = tempfile::tempdir().unwrap();
    let old = fake_install(dir.path(), "rmod", "0.1.0", "MIT");
    let new = fake_install(dir.path(), "rmod", "0.2.0", "MIT");
    let now = SystemTime::now();
    set_written(&old.join("baler.toml"), now - Duration::from_secs(3600));
    set_written(&new.join("baler.toml"), now);

    let listing = installed::list(dir.path()).unwrap();
    assert_eq!(listing.modules.len(), 1);
    assert_eq!(listing.modules[0].version, "0.2.0");

    // A downgrade writes the older version last, so it wins, whatever
    // the version numbers say.
    set_written(&old.join("baler.toml"), now + Duration::from_secs(3600));
    let listing = installed::list(dir.path()).unwrap();
    assert_eq!(listing.modules[0].version, "0.1.0");
}

#[test]
fn list_reports_an_unreadable_record_and_still_lists_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "fine", "1.0.0", "MIT");
    let broken = fake_install(dir.path(), "broken", "1.0.0", "MIT");
    std::fs::write(broken.join("baler.toml"), "this is = not [valid").unwrap();

    let listing = installed::list(dir.path()).unwrap();

    let found: Vec<&str> = listing.modules.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(found, ["fine"]);
    assert_eq!(listing.problems.len(), 1);
    assert!(listing.problems[0].starts_with("broken:"), "{:?}", listing.problems);
}

#[test]
fn find_names_what_is_wrong_when_a_module_is_not_there() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "fine", "1.0.0", "MIT");
    std::fs::create_dir_all(dir.path().join("handmade")).unwrap();

    let missing = installed::find(dir.path(), "nothere").unwrap_err().to_string();
    assert!(missing.contains("'nothere' is not installed"), "{missing}");

    let unrecorded = installed::find(dir.path(), "handmade").unwrap_err().to_string();
    assert!(unrecorded.contains("no record"), "{unrecorded}");

    let escaping = installed::find(dir.path(), "..").unwrap_err().to_string();
    assert!(escaping.contains("is not a module name"), "{escaping}");

    assert_eq!(installed::find(dir.path(), "fine").unwrap().version, "1.0.0");
}

// ---- what `baler list` and `baler show` print ----

#[test]
fn list_output_lines_columns_up_and_has_no_trailing_spaces() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "alpha", "0.1.0", "MIT");
    fake_install(dir.path(), "bravo-long", "0.2.0", "Apache-2.0");

    let text = list::render(&installed::list(dir.path()).unwrap(), "unused");
    let lines: Vec<&str> = text.lines().collect();

    assert_eq!(lines.len(), 3, "{text}");
    assert!(lines[0].starts_with("NAME"), "{text}");
    assert!(lines[1].starts_with("alpha"), "{text}");
    assert!(lines[2].starts_with("bravo-long"), "{text}");
    for column in ["VERSION", "LICENSE", "R"] {
        // rfind, since the one-letter header `R` is also inside `VERSION`.
        let at = lines[0].rfind(column).unwrap();
        let cell = match column {
            "VERSION" => ("0.1.0", "0.2.0"),
            "LICENSE" => ("MIT", "Apache-2.0"),
            _ => (">=4.0.0", ">=4.0.0"),
        };
        assert_eq!(lines[1].find(cell.0), Some(at), "{column} column, {text}");
        assert_eq!(lines[2].find(cell.1), Some(at), "{column} column, {text}");
    }
    assert!(lines.iter().all(|line| line == &line.trim_end()), "{text:?}");
}

#[test]
fn list_output_says_where_it_looked_when_nothing_is_installed() {
    let dir = tempfile::tempdir().unwrap();
    let text = list::render(&installed::list(dir.path()).unwrap(), "/some/where");
    assert_eq!(text, "No modules installed in /some/where.\n");
}

/// The text after `label:` on the line that starts with it.
fn value_of<'a>(text: &'a str, label: &str) -> &'a str {
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{label}:")))
        .unwrap_or_else(|| panic!("no '{label}:' line in:\n{text}"))
        .trim()
}

#[test]
fn show_prints_what_the_module_declared() {
    let dir = tempfile::tempdir().unwrap();
    let dist_info = fake_install(dir.path(), "rmod", "0.2.0", "MIT");
    std::fs::write(dist_info.join("LICENSE"), "MIT License").unwrap();
    std::fs::write(dist_info.join("README.md"), "# rmod").unwrap();
    std::fs::write(dist_info.join("baler.lock"), "").unwrap();

    let text = show::render(&installed::find(dir.path(), "rmod").unwrap());

    assert!(text.starts_with("rmod 0.2.0\nThe rmod module\n"), "{text}");
    assert_eq!(value_of(&text, "License"), "MIT");
    assert_eq!(value_of(&text, "License file"), "LICENSE");
    assert_eq!(value_of(&text, "README"), "README.md");
    assert_eq!(value_of(&text, "Authors"), "Jane Doe, John Smith <john@example.com>");
    assert_eq!(value_of(&text, "R"), ">=4.0.0");
    assert_eq!(value_of(&text, "Keywords"), "one, two");
    assert_eq!(value_of(&text, "Locked"), "yes");
    assert_eq!(value_of(&text, "Native code"), "none");
    assert!(value_of(&text, "Location").ends_with("rmod"), "{text}");
    assert!(value_of(&text, "Metadata").ends_with("rmod-0.2.0.dist-info"), "{text}");
    assert!(text.contains("R packages:\n  cli (>=3.0.0)\n  dplyr (*)\n"), "{text}");
    assert!(text.contains("baler modules:\n  other (>=1.0.0)\n"), "{text}");
}

#[test]
fn show_says_none_for_a_module_installed_without_a_license_or_readme() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "old", "0.1.0", "MIT");

    let text = show::render(&installed::find(dir.path(), "old").unwrap());

    assert_eq!(value_of(&text, "License file"), "none");
    assert_eq!(value_of(&text, "README"), "none");
    assert_eq!(value_of(&text, "Locked"), "no");
}

#[test]
fn show_notices_compiled_code() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "fast", "1.0.0", "MIT");
    let lib = dir.path().join("fast").join("src").join(".lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("fast.so"), "").unwrap();

    let text = show::render(&installed::find(dir.path(), "fast").unwrap());

    assert_eq!(value_of(&text, "Native code"), "need compilation");
}

// ---- license and README through a bundle ----

fn project_with_source(root: &Path, name: &str, license: &str, extra_toml: &str) -> PathBuf {
    std::fs::write(
        root.join("baler.toml"),
        format!("{}{extra_toml}", module_toml(name, "0.1.0", license)),
    )
    .unwrap();
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("__init__.R"), "box::use()\n").unwrap();
    src
}

fn bundle_and_unpack(root: &Path, src: &Path, name: &str) -> (PathBuf, tempfile::TempDir) {
    let archive_dir = tempfile::tempdir().unwrap();
    let archive = archive_dir.path().join(format!("{name}_0.1.0.tar.gz"));
    tar::bundle(src, root, &archive, name, "0.1.0", &[], &[]).unwrap();
    let install = tempfile::tempdir().unwrap();
    tar::unpack(&archive, install.path(), name, "0.1.0").unwrap();
    (install.path().join(format!("{name}-0.1.0.dist-info")), install)
}

#[test]
fn a_bundle_carries_the_license_and_readme_into_the_dist_info_folder() {
    let project = tempfile::tempdir().unwrap();
    let src = project_with_source(project.path(), "mymod", "MIT", "");
    std::fs::write(project.path().join("LICENSE"), "MIT License text").unwrap();
    std::fs::write(project.path().join("README.md"), "# mymod").unwrap();

    let (dist_info, install) = bundle_and_unpack(project.path(), &src, "mymod");

    assert_eq!(std::fs::read_to_string(dist_info.join("LICENSE")).unwrap(), "MIT License text");
    assert_eq!(std::fs::read_to_string(dist_info.join("README.md")).unwrap(), "# mymod");
    assert!(dist_info.join("baler.toml").is_file());
    // They are metadata, so neither lands among the module's own files
    // nor loose in the install directory where modules share one root.
    assert!(!install.path().join("LICENSE").exists());
    assert!(!install.path().join("README.md").exists());
    assert!(!install.path().join("mymod").join("LICENSE").exists());
}

#[test]
fn several_license_files_are_all_kept_whatever_their_case() {
    let project = tempfile::tempdir().unwrap();
    let src = project_with_source(project.path(), "mymod", "MIT OR Apache-2.0", "");
    std::fs::write(project.path().join("License-MIT.txt"), "not a license name").unwrap();
    std::fs::write(project.path().join("LICENSE.md"), "markdown license").unwrap();
    std::fs::write(project.path().join("copying"), "copying text").unwrap();

    let (dist_info, _install) = bundle_and_unpack(project.path(), &src, "mymod");

    assert!(dist_info.join("LICENSE.md").is_file());
    assert!(dist_info.join("copying").is_file());
    assert!(!dist_info.join("License-MIT.txt").exists(), "only the known names count");
}

#[test]
fn a_bundle_without_license_or_readme_still_installs_as_before() {
    let project = tempfile::tempdir().unwrap();
    let src = project_with_source(project.path(), "mymod", "MIT", "");

    let (dist_info, _install) = bundle_and_unpack(project.path(), &src, "mymod");

    let kept: Vec<String> = std::fs::read_dir(&dist_info)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_str().unwrap().to_owned())
        .collect();
    assert_eq!(kept, ["baler.toml"]);
}

#[test]
fn a_declared_readme_is_stored_as_readme_wherever_it_was_kept() {
    let project = tempfile::tempdir().unwrap();
    let src = project_with_source(project.path(), "mymod", "MIT", "");
    let toml = std::fs::read_to_string(project.path().join("baler.toml")).unwrap();
    std::fs::write(
        project.path().join("baler.toml"),
        toml.replace("license = \"MIT\"", "license = \"MIT\"\nreadme = \"docs/intro.md\""),
    )
    .unwrap();
    std::fs::create_dir_all(project.path().join("docs")).unwrap();
    std::fs::write(project.path().join("docs").join("intro.md"), "# intro").unwrap();
    // Not the declared one, so it must not win over it.
    std::fs::write(project.path().join("README.md"), "# not this one").unwrap();

    let (dist_info, _install) = bundle_and_unpack(project.path(), &src, "mymod");

    assert_eq!(std::fs::read_to_string(dist_info.join("README.md")).unwrap(), "# intro");
}

#[test]
fn a_module_directory_called_license_stays_a_module_directory() {
    let project = tempfile::tempdir().unwrap();
    let src = project_with_source(project.path(), "license", "MIT", "");
    // A file named like a license inside the module is module content.
    std::fs::write(src.join("LICENSE"), "shipped inside the module").unwrap();
    std::fs::write(project.path().join("LICENSE"), "the project's license").unwrap();

    let (dist_info, install) = bundle_and_unpack(project.path(), &src, "license");

    assert!(install.path().join("license").is_dir(), "the module directory");
    assert_eq!(
        std::fs::read_to_string(install.path().join("license").join("LICENSE")).unwrap(),
        "shipped inside the module"
    );
    assert_eq!(
        std::fs::read_to_string(dist_info.join("LICENSE")).unwrap(),
        "the project's license"
    );
}
