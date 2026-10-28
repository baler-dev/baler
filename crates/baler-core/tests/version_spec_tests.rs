use baler_core::version::VersionSpec;
use semver::Version;

fn spec(s: &str) -> VersionSpec {
    VersionSpec::parse(s).unwrap()
}

fn ver(s: &str) -> Version {
    Version::parse(s).unwrap()
}

#[test]
fn and_keeps_both_bounds() {
    let merged = spec(">=1.0.0").and(&spec("<2.0.0"));
    assert!(merged.matches(&ver("1.5.0")));
    assert!(!merged.matches(&ver("0.9.0")));
    assert!(!merged.matches(&ver("2.0.0")));
}

#[test]
fn and_with_star_is_the_other_spec() {
    let merged = spec("*").and(&spec(">=1.0.0"));
    assert!(!merged.matches(&ver("0.1.0")));
    assert!(merged.matches(&ver("1.0.0")));
}

#[test]
fn and_of_identical_specs_does_not_repeat_comparators() {
    let merged = spec(">=3.0.0").and(&spec(">=3.0.0"));
    assert_eq!(merged.to_string(), ">=3.0.0");
}

#[test]
fn and_is_unsatisfiable_for_disjoint_ranges() {
    let merged = spec(">=2.0.0").and(&spec("<1.0.0"));
    let candidates = vec![ver("3.0.0"), ver("1.5.0"), ver("0.5.0")];
    assert!(VersionSpec::resolve(&[merged], &candidates).is_none());
}

#[test]
fn merged_specs_mean_the_same_to_the_r_requirement_parser() {
    let specs = ["*", ">=1.0.0", "^1.2.3", "~1.2", "=1.2.3", ">=1.0.0, <2.0.0", "1.2.3"];
    let candidates = ["0.9.0", "1.0.0", "1.2.3", "1.9.9", "2.0.0"];
    for text in specs {
        let shown = spec(text).to_string();
        let r_req = baler_core::ver::VerReq::parse(&shown)
            .unwrap_or_else(|error| panic!("{text:?} shown as {shown:?}: {error:#}"));
        for candidate in candidates {
            let r_ver = baler_core::ver::Ver::parse(candidate).unwrap();
            assert_eq!(
                spec(text).matches(&ver(candidate)),
                r_req.matches(&r_ver),
                "{text:?} against {candidate}"
            );
        }
    }
}

#[test]
fn merged_spec_survives_a_display_and_parse_round_trip() {
    let merged = spec(">=1.0.0").and(&spec("<2.0.0")).and(&spec("*"));
    let reparsed = spec(&merged.to_string());
    assert!(reparsed.matches(&ver("1.2.3")));
    assert!(!reparsed.matches(&ver("2.0.0")));
}
