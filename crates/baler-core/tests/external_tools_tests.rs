use baler_core::ops::external_tools::parse_version;
use semver::Version;

fn v(major: u64, minor: u64, patch: u64) -> Option<Version> {
    Some(Version::new(major, minor, patch))
}

#[test]
fn parses_plain_and_prefixed_versions() {
    assert_eq!(parse_version("1.4.550\n"), v(1, 4, 550));
    assert_eq!(parse_version("quarto 1.4.550"), v(1, 4, 550));
    assert_eq!(parse_version("pandoc v3.1.11.1"), v(3, 1, 11));
}

#[test]
fn pads_short_and_date_versions() {
    assert_eq!(parse_version("tool 3.1"), v(3, 1, 0));
    assert_eq!(parse_version("2024.09"), v(2024, 9, 0));
}

#[test]
fn skips_numbers_without_a_dot() {
    assert_eq!(parse_version("Copyright 2020 Foo. version 2.5.1"), v(2, 5, 1));
}

#[test]
fn ignores_prerelease_suffix() {
    assert_eq!(parse_version("tool 3.10.2-beta.1"), v(3, 10, 2));
}

#[test]
fn none_when_no_dotted_number() {
    assert_eq!(parse_version(""), None);
    assert_eq!(parse_version("no version here 42"), None);
}
