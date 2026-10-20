use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Result};
use baler_core::resolver::{dependency_order, solve, Node, Preferred, Solution, Universe};
use baler_core::ver::{Ver, VerReq};

fn ver(text: &str) -> Ver {
    Ver::parse(text).unwrap()
}

fn req(text: &str) -> VerReq {
    VerReq::parse(text).unwrap()
}

fn cran(name: &str) -> Node {
    Node::Cran(name.to_owned())
}

/// An in-memory universe that counts how often the slow call is made.
#[derive(Default)]
struct Fake {
    packages: BTreeMap<Node, Vec<(Ver, Vec<(Node, VerReq)>)>>,
    older_calls: Cell<usize>,
}

impl Fake {
    fn with(mut self, node: Node, version: &str, deps: &[(Node, &str)]) -> Self {
        let deps = deps.iter().map(|(n, r)| (n.clone(), req(r))).collect();
        self.packages.entry(node).or_default().push((ver(version), deps));
        self
    }

    fn newest(&self, node: &Node) -> Option<Ver> {
        self.packages.get(node)?.iter().map(|(v, _)| v.clone()).max()
    }
}

impl Universe for Fake {
    fn current(&self, node: &Node) -> Result<Option<Ver>> {
        Ok(self.newest(node))
    }

    fn older(&self, node: &Node) -> Result<Vec<Ver>> {
        self.older_calls.set(self.older_calls.get() + 1);
        let newest = self.newest(node);
        Ok(self
            .packages
            .get(node)
            .map(|all| all.iter().map(|(v, _)| v.clone()).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter(|v| Some(v) != newest.as_ref())
            .collect())
    }

    fn dependencies(&self, node: &Node, version: &Ver) -> Result<Vec<(Node, VerReq)>> {
        match self
            .packages
            .get(node)
            .and_then(|all| all.iter().find(|(v, _)| v == version))
        {
            Some((_, deps)) => Ok(deps.clone()),
            None => bail!("{node} {version} does not exist"),
        }
    }
}

fn solve_plain(universe: &Fake, requested: &[(Node, &str)]) -> Result<Solution> {
    let requested: Vec<(Node, VerReq)> =
        requested.iter().map(|(n, r)| (n.clone(), req(r))).collect();
    solve(universe, &requested, &Preferred::new())
}

fn picked(solution: &Solution, node: &Node) -> String {
    solution.versions[node].to_string()
}

#[test]
fn picks_the_newest_version_that_fits() {
    let fake = Fake::default()
        .with(cran("cli"), "3.6.2", &[])
        .with(cran("cli"), "3.5.0", &[]);
    let solution = solve_plain(&fake, &[(cran("cli"), ">= 3.0")]).unwrap();
    assert_eq!(picked(&solution, &cran("cli")), "3.6.2");
}

#[test]
fn backs_up_to_an_older_release_when_the_newest_conflicts() {
    // a 2.0 needs cli >= 3 but b needs cli < 3, so a must go back to 1.0.
    let fake = Fake::default()
        .with(cran("a"), "2.0", &[(cran("cli"), ">= 3.0")])
        .with(cran("a"), "1.0", &[])
        .with(cran("b"), "1.0", &[(cran("cli"), "< 3.0")])
        .with(cran("cli"), "3.6.2", &[])
        .with(cran("cli"), "2.9.0", &[]);
    let solution =
        solve_plain(&fake, &[(cran("a"), "*"), (cran("b"), "*")]).unwrap();
    assert_eq!(picked(&solution, &cran("a")), "1.0");
    assert_eq!(picked(&solution, &cran("cli")), "2.9.0");
}

#[test]
fn follows_the_dependencies_of_the_version_it_chose() {
    let fake = Fake::default()
        .with(cran("a"), "2.0", &[(cran("new-dep"), "*")])
        .with(cran("a"), "1.0", &[(cran("old-dep"), "*")])
        .with(cran("new-dep"), "1.0", &[])
        .with(cran("old-dep"), "1.0", &[]);
    let solution = solve_plain(&fake, &[(cran("a"), "< 2.0")]).unwrap();
    assert!(solution.versions.contains_key(&cran("old-dep")));
    assert!(!solution.versions.contains_key(&cran("new-dep")));
}

#[test]
fn shared_dependencies_are_intersected() {
    let fake = Fake::default()
        .with(cran("a"), "1.0", &[(cran("cli"), ">= 3.0")])
        .with(cran("b"), "1.0", &[(cran("cli"), "< 4")])
        .with(cran("cli"), "3.6.2", &[])
        .with(cran("cli"), "4.0.0", &[]);
    let solution =
        solve_plain(&fake, &[(cran("a"), "*"), (cran("b"), "*")]).unwrap();
    assert_eq!(picked(&solution, &cran("cli")), "3.6.2");
}

#[test]
fn a_dev_version_satisfies_a_lower_bound_on_its_release() {
    let fake = Fake::default()
        .with(cran("a"), "1.0", &[(cran("cli"), ">= 3.6.2")])
        .with(cran("cli"), "3.6.2.9000", &[]);
    let solution = solve_plain(&fake, &[(cran("a"), "*")]).unwrap();
    assert_eq!(picked(&solution, &cran("cli")), "3.6.2.9000");
}

#[test]
fn a_preferred_version_is_kept_while_it_fits() {
    let fake = Fake::default()
        .with(cran("cli"), "3.6.2", &[])
        .with(cran("cli"), "3.5.0", &[]);
    let preferred = Preferred::from([(cran("cli"), ver("3.5.0"))]);
    let requested = [(cran("cli"), req(">= 3.0"))];
    let solution = solve(&fake, &requested, &preferred).unwrap();
    assert_eq!(picked(&solution, &cran("cli")), "3.5.0");
}

#[test]
fn a_preferred_version_is_dropped_when_it_no_longer_fits() {
    let fake = Fake::default()
        .with(cran("cli"), "3.6.2", &[])
        .with(cran("cli"), "2.0.0", &[]);
    let preferred = Preferred::from([(cran("cli"), ver("2.0.0"))]);
    let requested = [(cran("cli"), req(">= 3.0"))];
    let solution = solve(&fake, &requested, &preferred).unwrap();
    assert_eq!(picked(&solution, &cran("cli")), "3.6.2");
}

#[test]
fn a_preferred_version_that_does_not_exist_is_an_error() {
    let fake = Fake::default().with(cran("cli"), "3.6.2", &[]);
    let preferred = Preferred::from([(cran("cli"), ver("3.5.0"))]);
    let requested = [(cran("cli"), req(">= 3.0"))];
    let error = solve(&fake, &requested, &preferred).unwrap_err().to_string();
    assert!(error.contains("cli 3.5.0"), "{error}");
}

#[test]
fn older_versions_are_only_listed_when_the_newest_does_not_fit() {
    let fake = Fake::default()
        .with(cran("cli"), "3.6.2", &[])
        .with(cran("cli"), "2.0.0", &[]);
    solve_plain(&fake, &[(cran("cli"), "*")]).unwrap();
    assert_eq!(fake.older_calls.get(), 0);

    solve_plain(&fake, &[(cran("cli"), "< 3.0")]).unwrap();
    assert_eq!(fake.older_calls.get(), 1);
}

#[test]
fn a_conflict_is_explained_in_terms_of_the_packages_involved() {
    let fake = Fake::default()
        .with(cran("a"), "1.0", &[(cran("cli"), "< 3.0")])
        .with(cran("b"), "1.0", &[(cran("cli"), ">= 4.0")])
        .with(cran("cli"), "4.0.0", &[])
        .with(cran("cli"), "2.0.0", &[]);
    let error = solve_plain(&fake, &[(cran("a"), "*"), (cran("b"), "*")])
        .unwrap_err()
        .to_string();
    for name in ["a", "b", "cli"] {
        assert!(error.contains(name), "{error}");
    }
}

#[test]
fn an_unknown_requested_package_is_reported_by_name() {
    let fake = Fake::default();
    let error = solve_plain(&fake, &[(cran("nope"), "*")]).unwrap_err().to_string();
    assert!(error.contains("'nope'"), "{error}");
}

#[test]
fn unreadable_metadata_names_the_version() {
    struct Broken;
    impl Universe for Broken {
        fn current(&self, _: &Node) -> Result<Option<Ver>> {
            Ok(Some(ver("1.0")))
        }
        fn older(&self, _: &Node) -> Result<Vec<Ver>> {
            Ok(Vec::new())
        }
        fn dependencies(&self, _: &Node, _: &Ver) -> Result<Vec<(Node, VerReq)>> {
            bail!("tarball is corrupt")
        }
    }
    let requested = [(cran("a"), VerReq::any())];
    let error = solve(&Broken, &requested, &Preferred::new()).unwrap_err().to_string();
    assert!(error.contains("a 1.0") && error.contains("tarball is corrupt"), "{error}");
}

#[test]
fn modules_and_packages_share_one_graph() {
    let module = Node::Module("geometry".to_owned());
    let core = Node::Module("core".to_owned());
    let fake = Fake::default()
        .with(module.clone(), "0.2.0", &[(core.clone(), "^1.0"), (cran("purrr"), ">= 1.0")])
        .with(core.clone(), "1.4.0", &[(cran("purrr"), "< 2.0")])
        .with(cran("purrr"), "1.0.2", &[]);
    let solution = solve_plain(&fake, &[(module.clone(), "*")]).unwrap();
    assert_eq!(picked(&solution, &core), "1.4.0");
    assert_eq!(picked(&solution, &cran("purrr")), "1.0.2");
    assert!(solution.edges[&module].contains(&core));
    assert!(solution.edges[&core].contains(&cran("purrr")));
}

#[test]
fn install_order_puts_dependencies_first() {
    let fake = Fake::default()
        .with(cran("a"), "1.0", &[(cran("b"), "*")])
        .with(cran("b"), "1.0", &[(cran("c"), "*")])
        .with(cran("c"), "1.0", &[]);
    let solution = solve_plain(&fake, &[(cran("a"), "*")]).unwrap();
    assert_eq!(solution.install_order(), [cran("c"), cran("b"), cran("a")]);
}

#[test]
fn dependency_order_ignores_edges_to_nodes_outside_the_set() {
    let edges = BTreeMap::from([(
        cran("a"),
        BTreeSet::from([cran("b"), cran("elsewhere")]),
    )]);
    let nodes = [cran("a"), cran("b")];
    assert_eq!(dependency_order(nodes.iter(), &edges), [cran("b"), cran("a")]);
}

#[test]
fn the_same_input_gives_the_same_solution() {
    let build = |order: &[usize]| {
        let entries = [
            (cran("a"), "1.0", vec![(cran("cli"), ">= 3.0")]),
            (cran("b"), "1.0", vec![(cran("cli"), "< 4")]),
            (cran("cli"), "3.6.2", vec![]),
            (cran("cli"), "3.5.0", vec![]),
        ];
        let mut fake = Fake::default();
        for index in order {
            let (node, version, deps) = &entries[*index];
            let deps: Vec<(Node, &str)> = deps.iter().map(|(n, r)| (n.clone(), *r)).collect();
            fake = fake.with(node.clone(), version, &deps);
        }
        solve_plain(&fake, &[(cran("a"), "*"), (cran("b"), "*")]).unwrap()
    };
    assert_eq!(build(&[0, 1, 2, 3]), build(&[3, 2, 1, 0]));
}

#[test]
fn a_requirement_accepts_exactly_the_versions_it_matches() {
    let reqs = [
        "*", ">= 1.0", "> 1.0", "<= 2.0", "< 2.0", "== 1.5", ">= 1.0, < 2.0", "^1.2.3",
        "~1.2", "^0.2.3", ">= 1.2.3.4",
    ];
    let versions = [
        "0.1", "0.2.3", "0.2.9", "0.3", "1.0", "1.0.0", "1.2", "1.2.3", "1.2.3.4",
        "1.2.3.9000", "1.2.9", "1.5", "1.9.9", "2.0", "2.0.0", "3.1",
    ];
    for text in reqs {
        for candidate in versions {
            let fake = Fake::default().with(cran("p"), candidate, &[]);
            let solved = solve_plain(&fake, &[(cran("p"), text)]).is_ok();
            assert_eq!(
                solved,
                req(text).matches(&ver(candidate)),
                "{text:?} against {candidate}"
            );
        }
    }
}
