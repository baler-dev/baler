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
    let parsed = parse_dep_field("R (>= 3.5), stats, cli(>=3.0.0), rlang (>= 1.1.0)");
    assert_eq!(
        parsed,
        [
            ("cli".to_owned(), ">=3.0.0".to_owned()),
            ("rlang".to_owned(), ">= 1.1.0".to_owned()),
        ]
    );
}
