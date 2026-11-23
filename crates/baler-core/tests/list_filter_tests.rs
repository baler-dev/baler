//! What `baler list` shows when it is given a pattern, keywords, or asked
//! for the names alone.

use baler_core::installed::Listing;
use baler_core::ops::list::{self, ListOptions};
use std::path::{Path, PathBuf};

/// Puts a module in `install_dir` the way an install leaves it, with the
/// given keywords in its baler.toml.
fn fake_install(install_dir: &Path, name: &str, keywords: &[&str]) -> PathBuf {
    let module = install_dir.join(name);
    std::fs::create_dir_all(&module).unwrap();
    std::fs::write(module.join("__init__.r"), "box::use()\n").unwrap();

    let keywords: Vec<String> = keywords.iter().map(|keyword| format!("\"{keyword}\"")).collect();
    let toml = format!(
        r#"[project]
name = "{name}"
version = "1.0.0"
description = "The {name} module"
authors = ["Jane Doe"]
license = "MIT"
r_version = ">=4.0.0"
keywords = [{}]
"#,
        keywords.join(", ")
    );
    let dist_info = install_dir.join(format!("{name}-1.0.0.dist-info"));
    std::fs::create_dir_all(&dist_info).unwrap();
    std::fs::write(dist_info.join("baler.toml"), toml).unwrap();
    dist_info
}

fn pattern(text: &str) -> ListOptions {
    ListOptions {
        pattern: Some(text.to_owned()),
        ..ListOptions::default()
    }
}

fn keywords(wanted: &[&str]) -> ListOptions {
    ListOptions {
        keywords: wanted.iter().map(|keyword| keyword.to_string()).collect(),
        ..ListOptions::default()
    }
}

fn names(listing: &Listing) -> Vec<&str> {
    listing.modules.iter().map(|module| module.name.as_str()).collect()
}

// ---- the pattern ----

#[test]
fn a_pattern_has_to_fit_the_whole_name() {
    let fits = [
        ("alpha", "alpha"),
        ("al*", "alpha"),
        ("*ha", "alpha"),
        ("*", "alpha"),
        ("**", "alpha"),
        ("*", ""),
        ("a*a", "alpha"),
        ("a?pha", "alpha"),
        ("*l*h*", "alpha"),
        ("foo-*", "foo-bar"),
        ("a*b*c", "aXbYbZc"),
        ("*a*b", "xaayb"),
        ("ALPHA", "alpha"),
        ("alpha", "ALPHA"),
    ];
    for (text, name) in fits {
        assert!(list::name_matches(text, name), "'{text}' should match '{name}'");
    }

    let misses = [
        ("alph", "alpha"),
        ("lph", "alpha"),
        ("al", "alpha"),
        ("alpha", "alpha2"),
        ("a?pha", "apha"),
        ("a?", "alpha"),
        ("?", ""),
        ("*b", "alpha"),
        ("a*b*c", "aXbY"),
    ];
    for (text, name) in misses {
        assert!(!list::name_matches(text, name), "'{text}' should not match '{name}'");
    }
}

#[test]
fn a_pattern_with_many_stars_does_not_blow_up() {
    let text = format!("{}b", "*a".repeat(20));
    let name = "a".repeat(200);
    assert!(!list::name_matches(&text, &name));
}

#[test]
fn a_pattern_lists_only_the_modules_whose_name_fits() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["alpha", "alpine", "bravo"] {
        fake_install(dir.path(), name, &[]);
    }

    let listing = list::select(dir.path(), &pattern("al*")).unwrap();
    assert_eq!(names(&listing), ["alpha", "alpine"]);

    let listing = list::select(dir.path(), &pattern("alpha")).unwrap();
    assert_eq!(names(&listing), ["alpha"]);

    let listing = list::select(dir.path(), &pattern("*")).unwrap();
    assert_eq!(names(&listing), ["alpha", "alpine", "bravo"]);

    let listing = list::select(dir.path(), &pattern("zz*")).unwrap();
    assert!(listing.modules.is_empty());
}

#[test]
fn a_broken_record_is_reported_only_when_the_pattern_asks_for_that_module() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "fine", &[]);
    let broken = fake_install(dir.path(), "broken", &[]);
    std::fs::write(broken.join("baler.toml"), "this is = not [valid").unwrap();

    let listing = list::select(dir.path(), &pattern("fi*")).unwrap();
    assert_eq!(names(&listing), ["fine"]);
    assert!(listing.problems.is_empty(), "{:?}", listing.problems);

    let listing = list::select(dir.path(), &pattern("bro*")).unwrap();
    assert!(listing.modules.is_empty());
    assert_eq!(listing.problems.len(), 1);
    assert!(listing.problems[0].starts_with("broken:"), "{:?}", listing.problems);
}

// ---- the keywords ----

#[test]
fn a_keyword_lists_the_modules_that_carry_it_whatever_its_case() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "alpha", &["Stats", "plots"]);
    fake_install(dir.path(), "bravo", &["plots"]);
    fake_install(dir.path(), "charlie", &[]);

    let listing = list::select(dir.path(), &keywords(&["stats"])).unwrap();
    assert_eq!(names(&listing), ["alpha"]);

    let listing = list::select(dir.path(), &keywords(&["  STATS "])).unwrap();
    assert_eq!(names(&listing), ["alpha"]);

    let listing = list::select(dir.path(), &keywords(&["plots"])).unwrap();
    assert_eq!(names(&listing), ["alpha", "bravo"]);
}

#[test]
fn several_keywords_must_all_be_carried() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "alpha", &["stats", "plots"]);
    fake_install(dir.path(), "bravo", &["plots"]);

    let listing = list::select(dir.path(), &keywords(&["stats", "plots"])).unwrap();
    assert_eq!(names(&listing), ["alpha"]);

    let listing = list::select(dir.path(), &keywords(&["stats", "missing"])).unwrap();
    assert!(listing.modules.is_empty());
}

#[test]
fn a_keyword_is_a_whole_keyword_and_not_a_piece_of_one() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "alpha", &["statistics"]);

    let listing = list::select(dir.path(), &keywords(&["stat"])).unwrap();
    assert!(listing.modules.is_empty());
}

#[test]
fn a_pattern_and_a_keyword_both_have_to_hold() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "alpha", &["plots"]);
    fake_install(dir.path(), "bravo", &["plots"]);
    fake_install(dir.path(), "brook", &[]);

    let options = ListOptions {
        pattern: Some("b*".to_owned()),
        keywords: vec!["plots".to_owned()],
        names_only: false,
    };
    let listing = list::select(dir.path(), &options).unwrap();

    assert_eq!(names(&listing), ["bravo"]);
}

// ---- what is printed ----

#[test]
fn names_only_prints_one_bare_name_on_each_line() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "alpha", &[]);
    fake_install(dir.path(), "bravo", &[]);
    let options = ListOptions {
        names_only: true,
        ..ListOptions::default()
    };

    let listing = list::select(dir.path(), &options).unwrap();
    let text = list::render_with(&listing, "unused", &options);

    assert_eq!(text, "alpha\nbravo\n");
}

#[test]
fn names_only_prints_nothing_at_all_when_there_is_nothing_to_name() {
    let dir = tempfile::tempdir().unwrap();
    let options = ListOptions {
        pattern: Some("zz*".to_owned()),
        names_only: true,
        ..ListOptions::default()
    };

    let listing = list::select(dir.path(), &options).unwrap();

    assert_eq!(list::render_with(&listing, "/some/where", &options), "");
}

#[test]
fn a_filter_that_matches_nothing_says_so_instead_of_blaming_the_install_directory() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "alpha", &[]);
    let options = ListOptions {
        pattern: Some("zz*".to_owned()),
        keywords: vec!["stats".to_owned()],
        names_only: false,
    };

    let listing = list::select(dir.path(), &options).unwrap();
    let text = list::render_with(&listing, "/some/where", &options);

    assert_eq!(text, "No installed modules match the pattern 'zz*' and the keyword 'stats'.\n");
}

#[test]
fn without_a_filter_an_empty_install_directory_is_named_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let options = ListOptions::default();

    let listing = list::select(dir.path(), &options).unwrap();

    assert_eq!(
        list::render_with(&listing, "/some/where", &options),
        "No modules installed in /some/where.\n"
    );
}

#[test]
fn asking_for_nothing_in_particular_prints_the_same_table_as_before() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "alpha", &["stats"]);
    fake_install(dir.path(), "bravo", &[]);
    let options = ListOptions::default();

    let listing = list::select(dir.path(), &options).unwrap();

    assert_eq!(names(&listing), ["alpha", "bravo"]);
    assert_eq!(list::render_with(&listing, "unused", &options), list::render(&listing, "unused"));
}

#[test]
fn a_filtered_table_keeps_its_header_and_columns() {
    let dir = tempfile::tempdir().unwrap();
    fake_install(dir.path(), "alpha", &["stats"]);
    fake_install(dir.path(), "bravo", &[]);
    let options = keywords(&["stats"]);

    let listing = list::select(dir.path(), &options).unwrap();
    let text = list::render_with(&listing, "unused", &options);
    let lines: Vec<&str> = text.lines().collect();

    assert_eq!(lines.len(), 2, "{text}");
    assert_eq!(lines[0].split_whitespace().collect::<Vec<_>>(), ["NAME", "VERSION", "LICENSE", "R"]);
    assert_eq!(lines[1].split_whitespace().collect::<Vec<_>>(), ["alpha", "1.0.0", "MIT", ">=4.0.0"]);
}

// ---- bad input ----

#[test]
fn an_empty_pattern_or_keyword_is_refused() {
    assert!(ListOptions::default().check().is_ok());
    assert!(pattern("al*").check().is_ok());
    assert!(keywords(&["stats"]).check().is_ok());

    let empty_pattern = pattern("").check().unwrap_err().to_string();
    assert!(empty_pattern.contains("pattern is empty"), "{empty_pattern}");
    assert!(pattern("   ").check().is_err());

    let empty_keyword = keywords(&["stats", ""]).check().unwrap_err().to_string();
    assert!(empty_keyword.contains("--keyword is empty"), "{empty_keyword}");
}
