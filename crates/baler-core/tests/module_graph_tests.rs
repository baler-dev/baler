use std::collections::{BTreeMap, HashMap};

use anyhow::Result;

use baler_core::baler_toml::{BalerToml, Dependencies, ModuleDep, ModuleMeta};
use baler_core::ops::module_graph::{resolve_transitive, ModuleFetcher};

struct MockFetcher {
    by_source: HashMap<String, BalerToml>,
}

impl ModuleFetcher for MockFetcher {
    fn fetch(&self, source: &str) -> Result<BalerToml> {
        self.by_source
            .get(source)
            .map(toml_clone)
            .ok_or_else(|| anyhow::anyhow!("no mock module registered for '{source}'"))
    }
}

// BalerToml doesn't derive Clone (ModuleMeta doesn't need to in
// production code), so the mock rebuilds a fresh copy per fetch call
// instead. Only used in this test file.
fn toml_clone(t: &BalerToml) -> BalerToml {
    BalerToml {
        project: ModuleMeta {
            name: t.project.name.clone(),
            version: t.project.version.clone(),
            description: t.project.description.clone(),
            readme: t.project.readme.clone(),
            authors: t.project.authors.clone(),
            license: t.project.license.clone(),
            r_version: t.project.r_version.clone(),
            repository: t.project.repository.clone(),
            keywords: t.project.keywords.clone(),
            src: t.project.src.clone(),
            dependencies: t.project.dependencies.clone(),
        },
        development: None,
        compiled_code: None,
        tool: None,
    }
}

fn minimal_toml(name: &str, version: &str, module_deps: Option<BTreeMap<String, ModuleDep>>) -> BalerToml {
    BalerToml {
        project: ModuleMeta {
            name: name.to_owned(),
            version: version.to_owned(),
            description: String::new(),
            readme: None,
            authors: vec![],
            license: "Unknown".to_owned(),
            r_version: "4.0.0".to_owned(),
            repository: None,
            keywords: Vec::new(),
            src: None,
            dependencies: Dependencies { packages: BTreeMap::new(), baler: module_deps },
        },
        development: None,
        compiled_code: None,
        tool: None,
    }
}

#[test]
fn walks_a_two_level_chain() {
    let mut b_deps = BTreeMap::new();
    b_deps.insert(
        "b".to_owned(),
        ModuleDep::Extended { version: "*".to_owned(), source: Some("gh:x/b".to_owned()) },
    );
    let root = minimal_toml("root", "0.1.0", Some(b_deps));

    let b = minimal_toml("b", "1.0.0", None);

    let fetcher = MockFetcher {
        by_source: HashMap::from([("gh:x/b".to_owned(), b)]),
    };

    let plan = resolve_transitive(&root, &fetcher).expect("resolution should succeed");
    assert_eq!(plan.modules.get("b").map(String::as_str), Some("1.0.0"));
}

#[test]
fn detects_a_cycle_instead_of_hanging() {
    let mut a_deps = BTreeMap::new();
    a_deps.insert(
        "b".to_owned(),
        ModuleDep::Extended { version: "*".to_owned(), source: Some("gh:x/b".to_owned()) },
    );
    let root = minimal_toml("a", "0.1.0", Some(a_deps.clone()));

    // b depends back on a — a cycle.
    let mut b_deps = BTreeMap::new();
    b_deps.insert(
        "a".to_owned(),
        ModuleDep::Extended { version: "*".to_owned(), source: Some("gh:x/a".to_owned()) },
    );
    let b = minimal_toml("b", "1.0.0", Some(b_deps));

    let fetcher = MockFetcher {
        by_source: HashMap::from([
            ("gh:x/b".to_owned(), b),
            ("gh:x/a".to_owned(), minimal_toml("a", "0.1.0", Some(a_deps))),
        ]),
    };

    let err = resolve_transitive(&root, &fetcher).expect_err("a cycle must error, not hang");
    assert!(err.to_string().contains("cycle"));
}

#[test]
fn missing_source_is_a_hard_error() {
    let mut deps = BTreeMap::new();
    deps.insert("b".to_owned(), ModuleDep::Simple("*".to_owned()));
    let root = minimal_toml("root", "0.1.0", Some(deps));

    let fetcher = MockFetcher { by_source: HashMap::new() };

    let err = resolve_transitive(&root, &fetcher).expect_err("no source should be a hard error");
    assert!(err.to_string().contains("no source declared"));
}
