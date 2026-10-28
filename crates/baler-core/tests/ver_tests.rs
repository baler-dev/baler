use baler_core::ver::{Ver, VerReq};
use semver::Version;

fn ver(text: &str) -> Ver {
    Ver::parse(text).unwrap()
}

fn req(text: &str) -> VerReq {
    VerReq::parse(text).unwrap()
}

fn accepts(requirement: &str, candidate: &str) -> bool {
    req(requirement).matches(&ver(candidate))
}

#[test]
fn dash_and_dot_are_the_same_version() {
    assert_eq!(ver("4.0-3"), ver("4.0.3"));
}

#[test]
fn as_str_keeps_the_published_text() {
    assert_eq!(ver("4.0-3").as_str(), "4.0-3");
    assert_eq!(ver("1.2.3.9000").to_string(), "1.2.3.9000");
}

#[test]
fn a_fourth_component_sorts_above_the_release() {
    assert!(ver("1.2.3.9000") > ver("1.2.3"));
    assert!(ver("1.2.3.9000") < ver("1.2.4"));
}

#[test]
fn a_shorter_version_sorts_below_its_extension() {
    assert!(ver("1.0") < ver("1.0.0"));
}

#[test]
fn components_compare_as_numbers() {
    assert!(ver("1.10") > ver("1.9"));
    assert!(ver("0.3-2") > ver("0.3-1"));
}

#[test]
fn malformed_versions_are_rejected() {
    for text in ["", "1..2", "1.a", "+1.2", "-1", "1.2.", "1.2.3-beta"] {
        assert!(Ver::parse(text).is_err(), "{text:?} should not parse");
    }
}

#[test]
fn dash_versions_work_in_requirements() {
    assert!(accepts(">= 0.3-1", "0.3-1"));
    assert!(accepts(">= 0.3-1", "0.3.2"));
    assert!(!accepts(">= 0.3-1", "0.2.9"));
}

#[test]
fn four_component_bounds_are_exact() {
    assert!(accepts(">= 1.2.3.4", "1.2.3.4"));
    assert!(!accepts(">= 1.2.3.4", "1.2.3"));
}

#[test]
fn a_less_than_bound_excludes_the_bare_major() {
    assert!(accepts("< 2", "1.9.9"));
    assert!(!accepts("< 2", "2.0"));
    assert!(!accepts("< 2", "2"));
}

#[test]
fn exact_pins_use_double_or_single_equals() {
    assert!(accepts("== 1.0", "1.0"));
    assert!(!accepts("== 1.0", "1.0.0"));
    assert!(accepts("=1.2.3", "1.2.3"));
}

#[test]
fn caret_follows_cargo_rules() {
    assert!(accepts("^1.2.3", "1.9.0"));
    assert!(!accepts("^1.2.3", "2.0.0"));
    assert!(!accepts("^1.2.3", "1.2.2"));
    assert!(accepts("^0.2.3", "0.2.9"));
    assert!(!accepts("^0.2.3", "0.3.0"));
    assert!(accepts("^0.0.3", "0.0.3"));
    assert!(!accepts("^0.0.3", "0.0.4"));
    assert!(!accepts("^0.0", "0.1.0"));
    assert!(accepts("^0", "0.9"));
    assert!(!accepts("^0", "1.0"));
}

#[test]
fn a_bare_version_is_a_caret() {
    assert_eq!(req("1.2.3"), req("^1.2.3"));
}

#[test]
fn tilde_follows_cargo_rules() {
    assert!(accepts("~1.2.3", "1.2.9"));
    assert!(!accepts("~1.2.3", "1.3.0"));
    assert!(accepts("~1", "1.9"));
    assert!(!accepts("~1", "2.0"));
}

#[test]
fn exactly_accepts_one_version() {
    let only = VerReq::exactly(&ver("1.2"));
    assert!(only.matches(&ver("1.2")));
    assert!(!only.matches(&ver("1.2.0")));
    assert_eq!(only.to_string(), "== 1.2");
}

#[test]
fn star_and_empty_accept_anything() {
    assert!(accepts("*", "0.0.1"));
    assert!(accepts("", "99.9"));
    assert_eq!(req("*"), VerReq::any());
}

#[test]
fn operators_without_versions_are_rejected() {
    assert!(VerReq::parse(">=").is_err());
    assert!(VerReq::parse(">= abc").is_err());
}

#[test]
fn and_intersects_and_deduplicates() {
    let merged = req(">= 1.0").and(&req("< 2")).and(&req(">= 1.0"));
    assert_eq!(merged.to_string(), ">= 1.0, < 2");
    assert!(merged.matches(&ver("1.5")));
    assert!(!merged.matches(&ver("2.0")));
}

#[test]
fn disjoint_requirements_pick_nothing() {
    let merged = req(">= 2.0").and(&req("< 1.0"));
    let candidates = vec![ver("3.0"), ver("1.5"), ver("0.5")];
    assert!(merged.highest(&candidates).is_none());
}

#[test]
fn highest_ignores_candidate_order_and_understands_dev_versions() {
    let candidates = vec![ver("1.2.2"), ver("1.2.3.9000"), ver("1.2.3")];
    assert_eq!(req("*").highest(&candidates), Some(&ver("1.2.3.9000")));
    assert_eq!(req("< 1.2.3.1").highest(&candidates), Some(&ver("1.2.3")));
}

#[test]
fn semver_shape_folds_extra_components_into_the_pre_release() {
    assert_eq!(ver("1.2.3.9000").to_semver().to_string(), "1.2.3-9000");
    assert_eq!(ver("0.3-1").to_semver().to_string(), "0.3.1");
    assert_eq!(ver("1").to_semver().to_string(), "1.0.0");
    assert_eq!(ver("1.2.3.4.5").to_semver().to_string(), "1.2.3-4.5");
}

#[test]
fn zero_padded_components_are_accepted() {
    assert_eq!(ver("1.0-01").parts(), &[1, 0, 1]);
    assert_eq!(ver("1.0-01").to_semver().to_string(), "1.0.1");
}

#[test]
fn lock_spellings_read_back_as_r_versions() {
    let pinned = Version::parse("1.2.3-9000").unwrap();
    assert_eq!(Ver::from_semver(&pinned).unwrap(), ver("1.2.3.9000"));
    let not_r = Version::parse("1.2.3-beta").unwrap();
    assert!(Ver::from_semver(&not_r).is_err());
}

#[test]
fn display_round_trips() {
    for text in [">= 0.3-1", ">= 1.2.3, < 2", "== 1.0", "*"] {
        let first = req(text);
        assert_eq!(req(&first.to_string()), first);
    }
    assert_eq!(req("^1.2.3").to_string(), ">= 1.2.3, < 2");
}
