use baler_core::cran::packages::{
    description_deps, parse_archive_listing, parse_dcf, parse_dep_field, RVersion,
};

#[test]
fn versions_keep_their_published_text_and_r_order() {
    let dev = RVersion::parse("1.2.3.9000").unwrap();
    assert_eq!(dev.as_str(), "1.2.3.9000");
    assert_eq!(dev.semver().to_string(), "1.2.3-9000");
    assert!(dev.ver() > RVersion::parse("1.2.3").unwrap().ver());

    let dashed = RVersion::parse("0.3-1").unwrap();
    assert_eq!(dashed.as_str(), "0.3-1");
    assert_eq!(dashed.semver().to_string(), "0.3.1");
}

#[test]
fn zero_padded_versions_parse() {
    let padded = RVersion::parse("1.0-01").unwrap();
    assert_eq!(padded.as_str(), "1.0-01");
    assert_eq!(padded.ver().parts(), &[1, 0, 1]);
}

#[test]
fn archive_listing_is_sorted_in_r_order() {
    let html = r#"<a href="pkg_1.2.2.tar.gz">x</a>
        <a href="pkg_1.2.3.9000.tar.gz">x</a>
        <a href="pkg_1.2.3.tar.gz">x</a>
        <a href="pkg_1.10.0.tar.gz">x</a>
        <a href="other_9.9.9.tar.gz">x</a>"#;
    let listed: Vec<String> = parse_archive_listing(html, "pkg")
        .iter()
        .map(|version| version.as_str().to_owned())
        .collect();
    assert_eq!(listed, ["1.10.0", "1.2.3.9000", "1.2.3", "1.2.2"]);
}

#[test]
fn index_keeps_zero_padded_packages_and_skips_unreadable_ones() {
    let text = "Package: a\nVersion: 1.0-01\nImports: cli (>= 3.0.0), rlang, stats\n\n\
                Package: b\nVersion: oops\n\n\
                Package: c\nVersion: 2.0\n";
    let index = parse_dcf(text.as_bytes()).unwrap();
    assert_eq!(index["a"].version.as_str(), "1.0-01");
    assert_eq!(
        index["a"].deps,
        [("cli".to_owned(), ">= 3.0.0".to_owned()), ("rlang".to_owned(), "*".to_owned())]
    );
    assert!(!index.contains_key("b"));
    assert!(index.contains_key("c"));
}

fn tarball_with(path: &str, contents: &str) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append_data(&mut header, path, contents.as_bytes()).unwrap();
    let tar_bytes = builder.into_inner().unwrap();
    let mut encoder =
        flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut encoder, &tar_bytes).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn archived_dependencies_come_from_the_tarball_description() {
    let description = [
        "Package: pkg",
        "Version: 0.9.0",
        "Imports: cli (>= 2.0.0),",
        "    rlang",
        "LinkingTo: cpp11",
        "Suggests: testthat",
        "",
    ]
    .join("\n");
    let tarball = tarball_with("pkg/DESCRIPTION", &description);
    assert_eq!(
        description_deps(&tarball, "pkg").unwrap(),
        [
            ("cli".to_owned(), ">= 2.0.0".to_owned()),
            ("rlang".to_owned(), "*".to_owned()),
            ("cpp11".to_owned(), "*".to_owned()),
        ]
    );
}

#[test]
fn a_tarball_without_a_description_is_an_error() {
    let tarball = tarball_with("pkg/R/code.R", "x = 1\n");
    assert!(description_deps(&tarball, "pkg").is_err());
}

#[test]
fn dependency_fields_drop_base_packages_and_keep_constraints() {
    let parsed = parse_dep_field("R (>= 3.5), stats, cli(>=3.0.0), rlang (>= 1.1.0)").unwrap();
    assert_eq!(
        parsed,
        [
            ("cli".to_owned(), ">=3.0.0".to_owned()),
            ("rlang".to_owned(), ">= 1.1.0".to_owned()),
        ]
    );
}

#[test]
fn a_dependency_field_wrapped_inside_an_entry_is_read_whole() {
    let text = [
        "Package: dplyr",
        "Version: 1.2.1",
        "Imports: cli (>= 3.6.2), generics, glue (>= 1.3.2), lifecycle (>=",
        "        1.0.3), magrittr (>= 1.5), methods, pillar (>= 1.9.0), R6, rlang (>=",
        "        1.3.0), tibble (>= 3.2.0), tidyselect (>= 1.2.0), utils, vctrs",
        "        (>= 0.6.4)",
        "Suggests: testthat",
        "",
    ]
    .join("\n");
    let index = parse_dcf(text.as_bytes()).unwrap();
    let deps: Vec<(&str, &str)> = index["dplyr"]
        .deps
        .iter()
        .map(|(name, constraint)| (name.as_str(), constraint.as_str()))
        .collect();
    assert_eq!(
        deps,
        [
            ("cli", ">= 3.6.2"),
            ("generics", "*"),
            ("glue", ">= 1.3.2"),
            ("lifecycle", ">= 1.0.3"),
            ("magrittr", ">= 1.5"),
            ("pillar", ">= 1.9.0"),
            ("R6", "*"),
            ("rlang", ">= 1.3.0"),
            ("tibble", ">= 3.2.0"),
            ("tidyselect", ">= 1.2.0"),
            ("vctrs", ">= 0.6.4"),
        ]
    );
}

#[test]
fn wrapping_a_field_at_any_space_does_not_change_what_it_means() {
    let field =
        "cli (>= 3.6.2), generics, glue (>= 1.3.2), lifecycle (>= 1.0.3), vctrs (>= 0.6.4), R6";
    let one_line = format!("Package: p\nVersion: 1.0\nImports: {field}\n");
    let wrapped = format!(
        "Package: p\nVersion: 1.0\nImports: {}\n",
        field.split(' ').collect::<Vec<_>>().join("\n    ")
    );
    let plain = parse_dcf(one_line.as_bytes()).unwrap();
    let torn = parse_dcf(wrapped.as_bytes()).unwrap();
    assert_eq!(plain["p"].deps.len(), 6);
    assert_eq!(torn["p"].deps, plain["p"].deps);
}

#[test]
fn an_entry_that_is_not_a_package_name_is_an_error() {
    assert!(parse_dep_field("cli, 1.0.3)").is_err());
}

#[test]
fn a_record_with_a_malformed_dependency_field_is_skipped_and_the_rest_are_kept() {
    let text = "Package: a\nVersion: 1.0\nImports: cli, 1.0.3)\n\nPackage: b\nVersion: 2.0\nImports: cli\n";
    let index = parse_dcf(text.as_bytes()).unwrap();
    assert!(!index.contains_key("a"));
    assert!(index.contains_key("b"));
}
