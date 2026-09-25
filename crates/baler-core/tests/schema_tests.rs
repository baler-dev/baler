#![cfg(feature = "schema")]

use baler_core::baler_toml::BalerToml;
use jsonschema::JSONSchema;

fn validates(toml_str: &str) -> bool {
    let schema = serde_json::to_value(schemars::schema_for!(BalerToml)).unwrap();
    let compiled = JSONSchema::compile(&schema).unwrap();
    let value: toml::Value = toml::from_str(toml_str).unwrap();
    let json = serde_json::to_value(value).unwrap();
    compiled.is_valid(&json)
}

const BASE: &str = r#"
[project]
name = "demo"
version = "0.1.0"
description = ""
authors = []
license = "MIT"
r_version = ">=4.0.0"
"#;

#[test]
fn accepts_bare_string_module_dep() {
    let toml_str = format!("{BASE}\n[project.dependencies.baler]\nother_module = \"1.0\"");
    assert!(validates(&toml_str), "bare string ModuleDep should validate");
}

#[test]
fn rejects_baler_named_package() {
    let toml_str = format!("{BASE}\n[project.dependencies]\nbaler = \"1.0\"");
    assert!(!validates(&toml_str), "a package literally named baler must not validate");
}

#[test]
fn accepts_realistic_full_config() {
    let toml_str = r#"
[project]
name = "demo"
version = "0.1.0"
description = "A demo module"
authors = [
    "Plain Name",
    { name = "Extended Name", email = "e@example.com" },
]
license = "MIT"
r_version = ">=4.0.0"
keywords = ["stats"]

[project.dependencies]
dplyr = "*"
ggplot2 = { version = ">=3.4.0" }

[project.dependencies.baler]
other_module = "1.0"
another_module = { version = "^2.0", source = "https://github.com/user/repo" }

[compiled-code]
path = ["cpp", "extra/src"]
build_deps = { Rcpp = "*" }
"#;
    assert!(validates(toml_str), "a realistic full config should validate");
}
