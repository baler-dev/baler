use std::cell::RefCell;
use std::collections::BTreeMap;

use anyhow::{Context, Result};
use baler_core::baler_toml::{BalerToml, ModuleDep, ModuleDepSpec, ModuleSource};
use baler_core::lockfile::{BalerLock, LockedModule};
use baler_core::ops::module_graph::{
    plan_module_installs, resolve_dependencies_pinned, ModuleFetcher, PinnedFetcher,
};

const RCALC: &str = "https://github.com/baler-dev/baler";

/// A pretend GitHub: one module, and a record of every source it was asked for.
struct Fake {
    manifest: String,
    requested: RefCell<Vec<String>>,
}

impl Fake {
    fn new() -> Self {
        let manifest = "[project]\nname = \"rcalc\"\nversion = \"0.1.0\"\ndescription = \"x\"\n\
                        authors = []\nlicense = \"MIT\"\nr_version = \">=4.0\"\n";
        Self { manifest: manifest.to_owned(), requested: RefCell::new(Vec::new()) }
    }

    fn requested(&self) -> Vec<String> {
        self.requested.borrow().clone()
    }
}

impl ModuleFetcher for Fake {
    fn fetch(&self, source: &ModuleSource) -> Result<BalerToml> {
        Ok(self.fetch_pinned(source)?.0)
    }

    fn fetch_pinned(&self, source: &ModuleSource) -> Result<(BalerToml, Option<String>)> {
        self.requested.borrow_mut().push(source.to_string());
        let toml = toml::from_str(&self.manifest).context("bad manifest")?;
        Ok((toml, Some("abc1234".to_owned())))
    }
}

fn declared(git_ref: &str) -> ModuleSource {
    ModuleSource::Git {
        url: RCALC.to_owned(),
        git_ref: Some(git_ref.to_owned()),
        module_dir: None,
    }
}

fn module_dep(tag: &str) -> ModuleDep {
    ModuleDep::Extended(ModuleDepSpec {
        git: Some(RCALC.to_owned()),
        tag: Some(tag.to_owned()),
        ..Default::default()
    })
}

fn pin() -> LockedModule {
    LockedModule {
        name: "rcalc".to_owned(),
        version: "0.1.0".to_owned(),
        git: Some(RCALC.to_owned()),
        git_ref: Some("v1".to_owned()),
        module_dir: None,
        rev: Some("9fceb02".to_owned()),
        url: None,
    }
}

#[test]
fn a_locked_module_is_fetched_at_its_pinned_commit() {
    let fake = Fake::new();
    let locked = [pin()];
    let fetcher = PinnedFetcher { inner: &fake, locked: &locked };
    let (_, rev) = fetcher.fetch_pinned(&declared("v1")).unwrap();
    assert_eq!(fake.requested(), [format!("{RCALC}@9fceb02")]);
    assert_eq!(rev.as_deref(), Some("9fceb02"));
}

#[test]
fn a_source_that_changed_in_baler_toml_ignores_the_old_pin() {
    let fake = Fake::new();
    let locked = [pin()];
    let fetcher = PinnedFetcher { inner: &fake, locked: &locked };
    let (_, rev) = fetcher.fetch_pinned(&declared("v2")).unwrap();
    assert_eq!(fake.requested(), [format!("{RCALC}@v2")]);
    assert_eq!(rev.as_deref(), Some("abc1234"));
}

#[test]
fn a_walk_collects_the_commit_each_module_was_fetched_at() {
    let root: BalerToml = toml::from_str(&format!(
        "[project]\nname = \"statmodule\"\nversion = \"0.1.0\"\ndescription = \"x\"\n\
         authors = []\nlicense = \"MIT\"\nr_version = \">=4.0\"\n\n\
         [dependencies.baler]\nrcalc = {{ git = \"{RCALC}\", tag = \"v1\" }}\n"
    ))
    .unwrap();
    let fake = Fake::new();
    let (plan, pins) = resolve_dependencies_pinned(
        &root.dependencies.packages,
        root.dependencies.baler.as_ref(),
        &fake,
        false,
    )
    .unwrap();
    assert_eq!(plan.modules["rcalc"], "0.1.0");
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].name, "rcalc");
    assert_eq!(pins[0].source, declared("v1"));
    assert_eq!(pins[0].rev.as_deref(), Some("abc1234"));
}

#[test]
fn a_missing_module_is_installed_at_its_pinned_commit() {
    let fake = Fake::new();
    let lock = BalerLock::from_packages(Vec::new()).with_modules(vec![pin()]);
    let modules = BTreeMap::from([("rcalc".to_owned(), module_dep("v1"))]);
    let planned = plan_module_installs(&modules, &fake, &|_| Ok(false), Some(&lock)).unwrap();
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].source, declared("9fceb02"));
}

#[test]
fn a_missing_module_without_a_pin_is_installed_from_the_declared_ref() {
    let fake = Fake::new();
    let modules = BTreeMap::from([("rcalc".to_owned(), module_dep("v1"))]);
    let planned = plan_module_installs(&modules, &fake, &|_| Ok(false), None).unwrap();
    assert_eq!(planned[0].source, declared("v1"));
}
