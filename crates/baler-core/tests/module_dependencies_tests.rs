use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result};
use baler_core::baler_toml::{BalerToml, ModuleDep, ModuleDepSpec, ModuleSource};
use baler_core::ops::module_graph::{
    plan_module_installs, resolve_dependencies, resolve_transitive, ModuleFetcher,
};

/// Pretend module sources, keyed by how the source prints, and a count of
/// how many times anything was fetched.
struct Fake {
    manifests: HashMap<String, String>,
    fetches: Cell<usize>,
}

impl Fake {
    fn new() -> Self {
        Self { manifests: HashMap::new(), fetches: Cell::new(0) }
    }

    fn module(mut self, git: &str, tag: &str, manifest: String) -> Self {
        self.manifests.insert(format!("{git}@{tag}"), manifest);
        self
    }
}

impl ModuleFetcher for Fake {
    fn fetch(&self, source: &ModuleSource) -> Result<BalerToml> {
        self.fetches.set(self.fetches.get() + 1);
        let text = self
            .manifests
            .get(&source.to_string())
            .with_context(|| format!("no module at {source}"))?;
        toml::from_str(text).context("bad manifest")
    }
}

fn manifest(name: &str, version: &str, packages: &[(&str, &str)], modules: &[(&str, &str)]) -> String {
    let mut text = format!(
        "[project]\nname = \"{name}\"\nversion = \"{version}\"\ndescription = \"x\"\n\
         authors = []\nlicense = \"MIT\"\nr_version = \">=4.0\"\n\n[dependencies]\n"
    );
    for (package, spec) in packages {
        text.push_str(&format!("{package} = {spec}\n"));
    }
    text.push_str("\n[dependencies.baler]\n");
    for (module, git) in modules {
        text.push_str(&format!("{module} = {{ git = \"{git}\", tag = \"v1\" }}\n"));
    }
    text
}

const GEOMETRY: &str = "https://github.com/u/geometry";
const CORE: &str = "https://github.com/u/core";

fn module_dep(git: &str, version: Option<&str>) -> ModuleDep {
    ModuleDep::Extended(ModuleDepSpec {
        version: version.map(str::to_owned),
        git: Some(git.to_owned()),
        tag: Some("v1".to_owned()),
        ..Default::default()
    })
}

fn root_with_module(packages: &[(&str, &str)]) -> BalerToml {
    let text = manifest("statmodule", "0.1.0", packages, &[("geometry", GEOMETRY)]);
    toml::from_str(&text).unwrap()
}

#[test]
fn the_packages_of_a_module_dependency_join_the_plan() {
    let fake = Fake::new().module(
        GEOMETRY,
        "v1",
        manifest("geometry", "0.2.0", &[("purrr", "\">=1.0\"")], &[]),
    );
    let plan = resolve_transitive(&root_with_module(&[("cli", "\">=3.6\"")]), &fake).unwrap();
    assert!(plan.packages.contains_key("cli"));
    assert!(plan.packages.contains_key("purrr"));
    assert_eq!(plan.modules["geometry"], "0.2.0");
}

#[test]
fn constraints_on_one_package_from_two_modules_are_combined() {
    let fake = Fake::new()
        .module(
            GEOMETRY,
            "v1",
            manifest("geometry", "0.2.0", &[("cli", "\">=3.0\"")], &[("core", CORE)]),
        )
        .module(CORE, "v1", manifest("core", "1.0.0", &[("cli", "\"<4\"")], &[]));
    let plan = resolve_transitive(&root_with_module(&[]), &fake).unwrap();
    let spec = &plan.packages["cli"].version_spec;
    assert!(spec.contains(">=3.0") && spec.contains("<4"), "{spec}");
}

#[test]
fn a_module_without_a_source_is_an_error_unless_it_is_skipped() {
    let text = "[project]\nname = \"statmodule\"\nversion = \"0.1.0\"\ndescription = \"x\"\n\
                authors = []\nlicense = \"MIT\"\nr_version = \">=4.0\"\n\n\
                [dependencies]\ncli = \"*\"\n\n[dependencies.baler]\ngeometry = \">=0.1\"\n";
    let root: BalerToml = toml::from_str(text).unwrap();
    let fake = Fake::new();

    let error = resolve_transitive(&root, &fake).unwrap_err();
    assert!(format!("{error:#}").contains("no source declared"));

    let plan = resolve_dependencies(
        &root.dependencies.packages,
        root.dependencies.baler.as_ref(),
        &fake,
        true,
    )
    .unwrap();
    assert!(plan.packages.contains_key("cli"));
    assert!(plan.modules.is_empty());
    assert_eq!(fake.fetches.get(), 0);
}

#[test]
fn a_module_that_names_no_repo_does_not_clash_with_one_that_does() {
    let fake = Fake::new().module(
        GEOMETRY,
        "v1",
        manifest("geometry", "0.2.0", &[("purrr", "\">=1.0\"")], &[]),
    );
    let root = root_with_module(&[(
        "purrr",
        "{ version = \"*\", repo = \"https://r-universe.example.org\" }",
    )]);
    let plan = resolve_transitive(&root, &fake).unwrap();
    assert_eq!(plan.packages["purrr"].repo, "https://r-universe.example.org");
}

#[test]
fn two_dependents_naming_different_repos_for_one_package_is_an_error() {
    let fake = Fake::new().module(
        GEOMETRY,
        "v1",
        manifest(
            "geometry",
            "0.2.0",
            &[("purrr", "{ version = \"*\", repo = \"https://other.example.org\" }")],
            &[],
        ),
    );
    let root = root_with_module(&[(
        "purrr",
        "{ version = \"*\", repo = \"https://r-universe.example.org\" }",
    )]);
    let error = resolve_transitive(&root, &fake).unwrap_err();
    assert!(error.to_string().contains("two repos"), "{error:#}");
}

fn not_installed(_: &str) -> Result<bool> {
    Ok(false)
}

#[test]
fn an_installed_module_is_left_alone_and_nothing_is_fetched() {
    let fake = Fake::new();
    let modules = BTreeMap::from([("geometry".to_owned(), module_dep(GEOMETRY, None))]);
    let planned = plan_module_installs(&modules, &fake, &|_| Ok(true), None).unwrap();
    assert!(planned.is_empty());
    assert_eq!(fake.fetches.get(), 0);
}

#[test]
fn a_missing_module_is_planned_with_the_version_its_source_declares() {
    let fake = Fake::new().module(GEOMETRY, "v1", manifest("geometry", "0.2.0", &[], &[]));
    let modules = BTreeMap::from([("geometry".to_owned(), module_dep(GEOMETRY, Some(">=0.1")))]);
    let planned = plan_module_installs(&modules, &fake, &not_installed, None).unwrap();
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].name, "geometry");
    assert_eq!(planned[0].version.to_string(), "0.2.0");
}

#[test]
fn a_source_whose_version_does_not_satisfy_the_entry_is_an_error() {
    let fake = Fake::new().module(GEOMETRY, "v1", manifest("geometry", "0.2.0", &[], &[]));
    let modules = BTreeMap::from([("geometry".to_owned(), module_dep(GEOMETRY, Some(">=1.0")))]);
    let error = plan_module_installs(&modules, &fake, &not_installed, None).unwrap_err();
    let text = error.to_string();
    assert!(text.contains("geometry") && text.contains("0.2.0"), "{text}");
}

#[test]
fn a_source_that_is_a_different_module_than_the_key_is_an_error() {
    let fake = Fake::new().module(GEOMETRY, "v1", manifest("shapes", "0.2.0", &[], &[]));
    let modules = BTreeMap::from([("geometry".to_owned(), module_dep(GEOMETRY, None))]);
    let error = plan_module_installs(&modules, &fake, &not_installed, None).unwrap_err();
    assert!(error.to_string().contains("'shapes'"), "{error:#}");
}

#[test]
fn a_missing_module_without_a_source_is_skipped() {
    let fake = Fake::new();
    let modules = BTreeMap::from([("geometry".to_owned(), ModuleDep::Simple(">=0.1".to_owned()))]);
    let planned = plan_module_installs(&modules, &fake, &not_installed, None).unwrap();
    assert!(planned.is_empty());
    assert_eq!(fake.fetches.get(), 0);
}
