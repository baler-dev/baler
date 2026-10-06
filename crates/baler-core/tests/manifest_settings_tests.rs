//! `[compiled-code]` bundle settings and typed `[extras.external]`
//! entries, checked through `BalerToml::from_dir` so the load-time
//! validation runs too.

use baler_core::baler_toml::{BalerToml, ExternalToolDep};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

const HEADER: &str = r#"
[project]
name = "demo"
version = "0.1.0"
description = "demo"
authors = ["Someone"]
license = "MIT"
r_version = ">=4.0.0"
"#;

/// Writes `HEADER` plus `extra` to a fresh temp dir and loads it.
fn load(extra: &str) -> anyhow::Result<BalerToml> {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir: PathBuf =
        std::env::temp_dir().join(format!("baler-manifest-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("baler.toml"), format!("{HEADER}\n{extra}")).unwrap();
    let result = BalerToml::from_dir(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn error_text(extra: &str) -> String {
    format!("{:#}", load(extra).expect_err("manifest should be rejected"))
}

fn external(extra: &str) -> std::collections::BTreeMap<String, ExternalToolDep> {
    load(extra)
        .expect("manifest should load")
        .extras
        .expect("[extras] present")
        .external
        .expect("[extras.external] present")
}

// ---- [compiled-code] ----

#[test]
fn compiled_code_flags_default_to_false() {
    let cc = load("[compiled-code]\npath = \"cpp\"\n").unwrap().compiled_code.unwrap();
    assert!(!cc.binary);
    assert!(!cc.keep_source);
}

#[test]
fn binary_and_keep_source_are_read() {
    let cc = load("[compiled-code]\nbinary = true\nkeep_source = true\n")
        .unwrap()
        .compiled_code
        .unwrap();
    assert!(cc.binary);
    assert!(cc.keep_source);
}

#[test]
fn binary_without_keep_source_is_fine() {
    let cc = load("[compiled-code]\nbinary = true\n").unwrap().compiled_code.unwrap();
    assert!(cc.binary);
    assert!(!cc.keep_source);
}

#[test]
fn keep_source_without_binary_is_rejected_at_load() {
    assert!(error_text("[compiled-code]\nkeep_source = true\n").contains("keep_source"));
}

// ---- [extras.external]: types ----

#[test]
fn bare_string_is_a_cli_entry() {
    let ext = external("[extras.external]\npandoc = \">=3.0\"\n");
    match &ext["pandoc"] {
        ExternalToolDep::Cli { version, hint } => {
            assert_eq!(version, ">=3.0");
            assert!(hint.is_none());
        }
        other => panic!("expected a cli entry, got {other:?}"),
    }
}

#[test]
fn table_without_type_defaults_to_cli() {
    let ext = external(
        "[extras.external.quarto]\nversion = \">=1.4\"\nhint = \"https://quarto.org\"\n",
    );
    match &ext["quarto"] {
        ExternalToolDep::Cli { version, hint } => {
            assert_eq!(version, ">=1.4");
            assert_eq!(hint.as_deref(), Some("https://quarto.org"));
        }
        other => panic!("expected a cli entry, got {other:?}"),
    }
}

#[test]
fn python_entry_reads_all_fields() {
    let ext = external(
        "[extras.external.numpy]\ntype = \"python\"\nversion = \">=1.24\"\n\
         python = \"python3.12\"\nindex = \"https://pypi.example/simple\"\n",
    );
    match &ext["numpy"] {
        ExternalToolDep::Python { version, python, index, git } => {
            assert_eq!(version, ">=1.24");
            assert_eq!(python.as_deref(), Some("python3.12"));
            assert_eq!(index.as_deref(), Some("https://pypi.example/simple"));
            assert!(git.is_none());
        }
        other => panic!("expected a python entry, got {other:?}"),
    }
}

#[test]
fn python_entry_accepts_git_alone() {
    let ext = external(
        "[extras.external.mylib]\ntype = \"python\"\nversion = \">=0.3\"\n\
         git = \"https://github.com/me/mylib\"\n",
    );
    assert!(matches!(&ext["mylib"], ExternalToolDep::Python { git: Some(_), .. }));
}

// ---- [extras.external]: rejected shapes ----

#[test]
fn index_and_git_together_are_rejected() {
    let err = error_text(
        "[extras.external.mylib]\ntype = \"python\"\nversion = \">=0.3\"\n\
         index = \"https://x/simple\"\ngit = \"https://github.com/me/mylib\"\n",
    );
    assert!(err.contains("index") && err.contains("git"), "{err}");
}

#[test]
fn unknown_type_is_rejected() {
    let err = error_text("[extras.external.x]\ntype = \"ruby\"\nversion = \"*\"\n");
    assert!(err.contains("ruby"), "{err}");
}

#[test]
fn unknown_key_is_rejected() {
    // a typo for `hint` must not be silently ignored
    assert!(load("[extras.external.x]\nversion = \"*\"\nhnit = \"oops\"\n").is_err());
}

#[test]
fn hint_on_a_python_entry_is_rejected() {
    assert!(load(
        "[extras.external.numpy]\ntype = \"python\"\nversion = \">=1\"\nhint = \"x\"\n"
    )
    .is_err());
}

#[test]
fn python_fields_on_a_cli_entry_are_rejected() {
    assert!(load("[extras.external.tool]\nversion = \">=1\"\nindex = \"https://x\"\n").is_err());
}

// ---- [extras.external]: key names ----

#[test]
fn cli_key_with_a_path_separator_is_rejected() {
    let err = error_text("[extras.external]\n\"bin/tool\" = \">=1.0\"\n");
    assert!(err.contains("bare command name"), "{err}");
}

#[test]
fn python_key_must_be_a_distribution_name() {
    for bad in ["not/valid", "-leading", "trailing-", "has space"] {
        let extra = format!(
            "[extras.external.\"{bad}\"]\ntype = \"python\"\nversion = \">=1\"\n"
        );
        assert!(load(&extra).is_err(), "{bad:?} should be rejected");
    }
}

#[test]
fn python_key_may_contain_dashes_underscores_and_dots() {
    for good in ["scikit-learn", "zope.interface", "typing_extensions", "numpy"] {
        let extra = format!(
            "[extras.external.\"{good}\"]\ntype = \"python\"\nversion = \">=1\"\n"
        );
        assert!(load(&extra).is_ok(), "{good:?} should be accepted");
    }
}
