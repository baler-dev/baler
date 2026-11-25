//! How the machine settings are layered, and what a config file may hold.
//!
//! Everything here goes through pure functions and temp files, never the
//! process environment, which is shared by all the tests in a binary.

use baler_core::settings::{
    Setting, SettingsLayer, Source, expand_file_path, layer_from_file, merge, parse_bool,
};
use std::path::{Path, PathBuf};

fn layer(install_deps: Option<bool>, lib: Option<&Path>) -> SettingsLayer {
    SettingsLayer { install_deps, lib: lib.map(Path::to_path_buf) }
}

fn default_lib() -> anyhow::Result<PathBuf> {
    Ok(std::env::temp_dir().join("default-modules"))
}

fn abs(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
}

fn file_source() -> Source {
    Source::File(abs("config.toml"))
}

/// Flag, then env, then file, as `load` orders them.
fn layers(flag: SettingsLayer, env: SettingsLayer, file: SettingsLayer) -> Vec<(Source, SettingsLayer)> {
    vec![(Source::Flag, flag), (Source::Env("BALER_X"), env), (file_source(), file)]
}

// ---- install-deps ----

#[test]
fn install_deps_is_on_when_no_layer_sets_it() {
    let settings = merge(layers(layer(None, None), layer(None, None), layer(None, None)), default_lib).unwrap();
    assert_eq!(settings.install_deps, Setting { value: true, source: Source::Default });
}

#[test]
fn install_deps_follows_the_highest_layer_that_sets_it() {
    let cases = [
        // (flag, env, file, expected value, expected source)
        (Some(false), Some(true), Some(true), false, Source::Flag),
        (Some(true), Some(false), Some(false), true, Source::Flag),
        (None, Some(false), Some(true), false, Source::Env("BALER_X")),
        (None, Some(true), Some(false), true, Source::Env("BALER_X")),
        (None, None, Some(false), false, file_source()),
        (None, None, Some(true), true, file_source()),
        (None, None, None, true, Source::Default),
    ];
    for (flag, env, file, value, source) in cases {
        let settings = merge(
            layers(layer(flag, None), layer(env, None), layer(file, None)),
            default_lib,
        )
        .unwrap();
        assert_eq!(
            settings.install_deps,
            Setting { value, source },
            "flag={flag:?} env={env:?} file={file:?}"
        );
    }
}

// ---- lib ----

#[test]
fn lib_falls_back_to_the_default() {
    let settings = merge(layers(layer(None, None), layer(None, None), layer(None, None)), default_lib).unwrap();
    assert_eq!(settings.lib, Setting { value: default_lib().unwrap(), source: Source::Default });
}

#[test]
fn lib_follows_the_highest_layer_that_sets_it() {
    let flag = abs("from-flag");
    let env = abs("from-env");
    let file = abs("from-file");
    let cases = [
        (Some(&flag), Some(&env), Some(&file), flag.clone(), Source::Flag),
        (None, Some(&env), Some(&file), env.clone(), Source::Env("BALER_X")),
        (None, None, Some(&file), file.clone(), file_source()),
    ];
    for (f, e, c, expected, source) in cases {
        let settings = merge(
            layers(layer(None, f.map(|p| p.as_path())), layer(None, e.map(|p| p.as_path())), layer(None, c.map(|p| p.as_path()))),
            default_lib,
        )
        .unwrap();
        assert_eq!(settings.lib, Setting { value: expected, source });
    }
}

#[test]
fn the_default_lib_is_not_looked_up_when_a_layer_sets_one() {
    let flag = abs("from-flag");
    let settings = merge(
        layers(layer(None, Some(&flag)), layer(None, None), layer(None, None)),
        || anyhow::bail!("the default must not be asked for"),
    )
    .unwrap();
    assert_eq!(settings.lib.value, flag);
}

#[test]
fn a_layer_decides_only_the_settings_it_sets() {
    let flag_lib = abs("from-flag");
    let settings = merge(
        layers(layer(None, Some(&flag_lib)), layer(None, None), layer(Some(false), None)),
        default_lib,
    )
    .unwrap();
    assert_eq!(settings.lib.source, Source::Flag);
    assert_eq!(settings.install_deps, Setting { value: false, source: file_source() });
}

// ---- booleans ----

#[test]
fn booleans_accept_the_usual_spellings() {
    for text in ["true", "TRUE", "1", "yes", "on", " true "] {
        assert!(parse_bool(text).unwrap(), "{text:?}");
    }
    for text in ["false", "False", "0", "no", "off"] {
        assert!(!parse_bool(text).unwrap(), "{text:?}");
    }
}

#[test]
fn a_misspelled_boolean_is_an_error() {
    let message = format!("{:#}", parse_bool("maybe").unwrap_err());
    assert!(message.contains("maybe"), "{message}");
}

// ---- config file ----

fn write_config(contents: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, contents).unwrap();
    (dir, path)
}

#[test]
fn a_missing_config_file_sets_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = layer_from_file(&dir.path().join("config.toml")).unwrap();
    assert_eq!(loaded, SettingsLayer::default());
}

#[test]
fn an_empty_config_file_sets_nothing() {
    let (_dir, path) = write_config("");
    assert_eq!(layer_from_file(&path).unwrap(), SettingsLayer::default());
}

#[test]
fn a_config_file_is_read() {
    let lib = abs("mods");
    let (_dir, path) = write_config(&format!(
        "install-deps = false\nlib = '{}'\n",
        lib.display()
    ));
    let loaded = layer_from_file(&path).unwrap();
    assert_eq!(loaded, layer(Some(false), Some(&lib)));
}

#[test]
fn an_unknown_key_in_a_config_file_is_an_error() {
    let (_dir, path) = write_config("install-dep = false\n");
    let message = format!("{:#}", layer_from_file(&path).unwrap_err());
    assert!(message.contains("install-dep"), "{message}");
    assert!(message.contains(&path.display().to_string()), "{message}");
}

#[test]
fn a_wrongly_typed_value_in_a_config_file_is_an_error() {
    let (_dir, path) = write_config("install-deps = \"yes\"\n");
    assert!(layer_from_file(&path).is_err());
}

#[test]
fn a_relative_lib_in_a_config_file_is_an_error() {
    let (_dir, path) = write_config("lib = 'mods'\n");
    let message = format!("{:#}", layer_from_file(&path).unwrap_err());
    assert!(message.contains("absolute"), "{message}");
}

#[test]
fn a_leading_tilde_is_the_home_directory() {
    let Some(home) = dirs::home_dir() else { return };
    assert_eq!(expand_file_path(Path::new("~/mods")).unwrap(), home.join("mods"));
    assert_eq!(expand_file_path(Path::new("~")).unwrap(), home);
}

#[test]
fn another_users_tilde_is_not_expanded() {
    assert!(expand_file_path(Path::new("~someone/mods")).is_err());
}
