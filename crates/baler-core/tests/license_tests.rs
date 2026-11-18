use baler_core::license::{is_unset, validate_spdx, warnings};
use baler_core::project_files::{
    find_project_files, is_license_file_name, is_readme_file_name, stored_readme_name,
    ProjectFiles,
};
use std::path::{Path, PathBuf};

fn with_license_file() -> ProjectFiles {
    ProjectFiles {
        licenses: vec![PathBuf::from("LICENSE")],
        ..ProjectFiles::default()
    }
}

// ---- the license field ----

#[test]
fn common_spdx_expressions_are_valid() {
    for expression in [
        "MIT",
        "Apache-2.0",
        "GPL-3.0-or-later",
        "GPL-2.0-only",
        "BSD-3-Clause",
        "MIT OR Apache-2.0",
        "MIT AND Zlib",
        "GPL-2.0-only WITH Classpath-exception-2.0",
        "LicenseRef-custom",
    ] {
        assert!(validate_spdx(expression).is_ok(), "{expression} should be valid");
    }
}

#[test]
fn r_style_and_misspelled_licenses_are_not_spdx() {
    for expression in ["GPL (>= 3)", "MIT + file LICENSE", "mit", "MIT/Apache-2.0", "Unknown", ""] {
        assert!(validate_spdx(expression).is_err(), "{expression:?} should be refused");
    }
}

#[test]
fn a_deprecated_identifier_is_refused_with_the_reason() {
    let reason = validate_spdx("GPL-3.0").unwrap_err();
    assert!(reason.contains("deprecated"), "{reason}");
}

#[test]
fn unset_means_empty_or_unknown_in_any_case() {
    for license in ["", "  ", "Unknown", "unknown", " UNKNOWN "] {
        assert!(is_unset(license), "{license:?}");
    }
    for license in ["MIT", "Unknowable"] {
        assert!(!is_unset(license), "{license:?}");
    }
}

// ---- the warnings `baler bundle` prints ----

#[test]
fn a_valid_license_with_a_file_has_nothing_to_warn_about() {
    assert!(warnings("MIT", &with_license_file()).is_empty());
    assert!(warnings(" MIT ", &with_license_file()).is_empty());
}

#[test]
fn an_unset_license_is_named_and_an_example_given() {
    let found = warnings("Unknown", &with_license_file());
    assert_eq!(found.len(), 1);
    assert!(found[0].contains("\"Unknown\""), "{}", found[0]);
    assert!(found[0].contains("\"MIT\""), "{}", found[0]);
}

#[test]
fn an_invalid_license_shows_what_was_written_and_why_it_failed() {
    let found = warnings("GPL (>= 3)", &with_license_file());
    assert_eq!(found.len(), 1);
    assert!(found[0].contains("GPL (>= 3)"), "{}", found[0]);
    assert!(found[0].contains("not a valid SPDX expression"), "{}", found[0]);
    assert!(found[0].contains("unknown term"), "{}", found[0]);
}

#[test]
fn a_missing_license_file_is_its_own_warning() {
    let found = warnings("MIT", &ProjectFiles::default());
    assert_eq!(found.len(), 1);
    assert!(found[0].contains("No license file"), "{}", found[0]);
    assert!(found[0].contains("COPYING"), "{}", found[0]);
}

#[test]
fn an_unset_license_with_no_file_is_one_message_that_says_nothing_is_declared() {
    for license in ["Unknown", "unknown", "", "  "] {
        let found = warnings(license, &ProjectFiles::default());
        assert_eq!(found.len(), 1, "{license:?}: {found:?}");
        assert!(found[0].contains("No license is declared"), "{}", found[0]);
        assert!(found[0].contains("\"MIT\""), "{}", found[0]);
    }
}

#[test]
fn an_empty_license_is_called_empty_and_not_shown_as_two_quotes() {
    let found = warnings("", &ProjectFiles::default());
    assert!(found[0].contains("is empty"), "{}", found[0]);
}

#[test]
fn an_unset_license_with_a_file_names_the_file_and_asks_for_the_identifier() {
    let files = ProjectFiles {
        licenses: vec![PathBuf::from("/project/COPYING")],
        ..ProjectFiles::default()
    };
    let found = warnings("Unknown", &files);
    assert_eq!(found.len(), 1);
    assert!(found[0].contains("Found COPYING"), "{}", found[0]);
    assert!(found[0].contains("Set it to the SPDX identifier"), "{}", found[0]);
    assert!(!found[0].contains("No license is declared"), "{}", found[0]);
}

#[test]
fn a_deprecated_gpl_identifier_names_the_two_replacements() {
    let found = warnings("GPL-3.0", &with_license_file());
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("deprecated"), "{}", found[0]);
    assert!(found[0].contains("\"GPL-3.0-only\""), "{}", found[0]);
    assert!(found[0].contains("\"GPL-3.0-or-later\""), "{}", found[0]);
    assert!(!found[0].contains("not a valid SPDX expression"), "{}", found[0]);
}

#[test]
fn a_deprecated_identifier_outside_the_gpl_family_points_at_the_list() {
    let found = warnings("eCos-2.0", &with_license_file());
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("Replace `eCos-2.0`"), "{}", found[0]);
    assert!(found[0].contains("https://spdx.org/licenses/"), "{}", found[0]);
    assert!(!found[0].contains("-only"), "{}", found[0]);
}

#[test]
fn every_deprecated_identifier_in_an_expression_is_named_once() {
    let found = warnings("GPL-2.0 OR LGPL-2.1", &with_license_file());
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("Replace `GPL-2.0`"), "{}", found[0]);
    assert!(found[0].contains("Replace `LGPL-2.1`"), "{}", found[0]);
}

#[test]
fn a_lowercase_license_is_invalid_and_not_called_deprecated() {
    let found = warnings("mit", &with_license_file());
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("not a valid SPDX expression"), "{}", found[0]);
    assert!(!found[0].contains("deprecated"), "{}", found[0]);
}

#[test]
fn a_custom_license_ref_with_a_file_is_silent_and_without_one_asks_for_the_text() {
    assert!(warnings("LicenseRef-Proprietary", &with_license_file()).is_empty());

    let found = warnings("LicenseRef-Proprietary", &ProjectFiles::default());
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("No license file"), "{}", found[0]);
}

#[test]
fn an_invalid_license_and_a_missing_file_are_both_reported() {
    let found = warnings("GPL (>= 3)", &ProjectFiles::default());
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].contains("not a valid SPDX expression"), "{}", found[0]);
    assert!(found[1].contains("No license file"), "{}", found[1]);
}

// ---- finding the files ----

fn touch(dir: &Path, name: &str) {
    if let Some(parent) = Path::new(name).parent() {
        std::fs::create_dir_all(dir.join(parent)).unwrap();
    }
    std::fs::write(dir.join(name), name).unwrap();
}

fn file_names(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| p.file_name().unwrap().to_str().unwrap().to_owned())
        .collect()
}

#[test]
fn license_file_names_match_without_regard_to_case() {
    for name in ["LICENSE", "license", "License.md", "LICENCE.txt", "COPYING", "copying.md"] {
        assert!(is_license_file_name(name), "{name}");
    }
    for name in ["LICENSE-MIT", "licenses", "NOTICE", "README.md", "my-license.txt"] {
        assert!(!is_license_file_name(name), "{name}");
    }
}

#[test]
fn readme_file_names_match_any_ending_in_any_case() {
    for name in ["README", "readme.md", "README.txt", "Readme.rst", "README.qmd"] {
        assert!(is_readme_file_name(name), "{name}");
    }
    for name in ["README-dev.md", "LICENSE", "readme.tar.gz", "notes.md"] {
        assert!(!is_readme_file_name(name), "{name}");
    }
}

#[test]
fn find_project_files_lists_licenses_sorted_and_only_from_the_root() {
    let dir = tempfile::tempdir().unwrap();
    touch(dir.path(), "LICENSE.md");
    touch(dir.path(), "COPYING");
    touch(dir.path(), "sub/LICENSE");
    touch(dir.path(), "baler.toml");

    let found = find_project_files(dir.path(), None);

    assert_eq!(file_names(&found.licenses), ["COPYING", "LICENSE.md"]);
    assert!(found.readme.is_none());
    assert!(found.problems.is_empty());
}

#[test]
fn an_undeclared_readme_prefers_md_then_txt_then_no_ending() {
    let dir = tempfile::tempdir().unwrap();
    touch(dir.path(), "README");
    touch(dir.path(), "README.txt");
    assert_eq!(file_names(&find_project_files(dir.path(), None).readme.into_iter().collect::<Vec<_>>()), ["README.txt"]);

    touch(dir.path(), "readme.md");
    assert_eq!(file_names(&find_project_files(dir.path(), None).readme.into_iter().collect::<Vec<_>>()), ["readme.md"]);
}

#[test]
fn a_declared_readme_wins_over_one_found_by_name() {
    let dir = tempfile::tempdir().unwrap();
    touch(dir.path(), "README.md");
    touch(dir.path(), "docs/intro.md");

    let found = find_project_files(dir.path(), Some("docs/intro.md"));

    assert_eq!(found.readme, Some(dir.path().join("docs/intro.md")));
    assert!(found.problems.is_empty());
}

#[test]
fn a_declared_readme_that_is_missing_is_a_problem_not_a_silent_fallback() {
    let dir = tempfile::tempdir().unwrap();
    touch(dir.path(), "README.md");

    let found = find_project_files(dir.path(), Some("docs/gone.md"));

    assert!(found.readme.is_none(), "must not quietly ship a different file");
    assert_eq!(found.problems.len(), 1);
    assert!(found.problems[0].contains("docs/gone.md"), "{:?}", found.problems);
}

#[test]
fn a_declared_readme_cannot_reach_outside_the_project() {
    let parent = tempfile::tempdir().unwrap();
    touch(parent.path(), "secret.md");
    let project = parent.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    for declared in ["../secret.md", "docs/../../secret.md", "/etc/passwd", ""] {
        let found = find_project_files(&project, Some(declared));
        assert!(found.readme.is_none(), "{declared:?} must be refused");
        assert_eq!(found.problems.len(), 1, "{declared:?}");
    }
}

#[test]
fn the_stored_readme_name_keeps_only_the_ending() {
    assert_eq!(stored_readme_name(Path::new("docs/intro.md")), "README.md");
    assert_eq!(stored_readme_name(Path::new("readme.txt")), "README.txt");
    assert_eq!(stored_readme_name(Path::new("README")), "README");
}
