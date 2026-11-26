//! How the machine settings are layered, and what a config file may hold.
//!
//! Everything here goes through pure functions and temp files, never the
//! process environment, which is shared by all the tests in a binary.

use baler_core::settings::{
    Setting, SettingsLayer, Source, expand_file_path, layer_from_file, merge, parse_bool,
    set_in_file, unset_in_file,
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
fn install_deps_is_off_when_no_layer_sets_it() {
    let settings = merge(layers(layer(None, None), layer(None, None), layer(None, None)), default_lib).unwrap();
    assert_eq!(settings.install_deps, Setting { value: false, source: Source::Default });
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
        (None, None, None, false, Source::Default),
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

// ---- writing the config file ----

fn config_in(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join(".baler").join("config.toml")
}

fn file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn set_creates_the_file_and_its_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_in(&dir);
    let lib = abs("mods");

    set_in_file(&path, "lib", lib.to_str().unwrap()).unwrap();
    set_in_file(&path, "install-deps", "false").unwrap();

    assert_eq!(layer_from_file(&path).unwrap(), layer(Some(false), Some(&lib)));
}

#[test]
fn set_replaces_a_value_instead_of_adding_a_second_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_in(&dir);

    set_in_file(&path, "install-deps", "false").unwrap();
    set_in_file(&path, "install-deps", "true").unwrap();

    assert_eq!(layer_from_file(&path).unwrap().install_deps, Some(true));
    assert_eq!(std::fs::read_to_string(&path).unwrap().matches("install-deps").count(), 1);
}

#[test]
fn set_keeps_the_comments_and_the_other_keys() {
    let (dir, path) = write_config("# machine settings\ninstall-deps = false # CI has no network\n");

    set_in_file(&path, "lib", abs("mods").to_str().unwrap()).unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("# machine settings"), "{text}");
    assert!(text.contains("# CI has no network"), "{text}");
    assert_eq!(layer_from_file(&path).unwrap().install_deps, Some(false));
    drop(dir);
}

#[test]
fn set_stores_a_relative_lib_as_an_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_in(&dir);

    set_in_file(&path, "lib", "mods").unwrap();

    let stored = layer_from_file(&path).unwrap().lib.unwrap();
    assert!(stored.is_absolute());
    assert_eq!(stored, std::env::current_dir().unwrap().join("mods"));
}

#[test]
fn set_keeps_a_leading_tilde_as_typed() {
    let Some(home) = dirs::home_dir() else { return };
    let dir = tempfile::tempdir().unwrap();
    let path = config_in(&dir);

    set_in_file(&path, "lib", "~/mods").unwrap();

    assert!(std::fs::read_to_string(&path).unwrap().contains("~/mods"));
    assert_eq!(layer_from_file(&path).unwrap().lib.unwrap(), home.join("mods"));
}

#[test]
fn set_refuses_a_bad_value_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_in(&dir);

    assert!(set_in_file(&path, "install-deps", "maybe").is_err());
    assert!(set_in_file(&path, "lib", "~someone/mods").is_err());

    assert!(!path.exists());
    assert!(!path.parent().unwrap().exists());
}

#[test]
fn set_refuses_an_unknown_key_and_names_the_known_ones() {
    let dir = tempfile::tempdir().unwrap();
    let message = format!("{:#}", set_in_file(&config_in(&dir), "libb", "x").unwrap_err());
    assert!(message.contains("libb"), "{message}");
    assert!(message.contains("install-deps") && message.contains("lib"), "{message}");
}

#[test]
fn set_does_not_touch_a_file_that_is_already_invalid() {
    let (_dir, path) = write_config("install-dep = false\n");

    assert!(set_in_file(&path, "install-deps", "true").is_err());

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "install-dep = false\n");
}

#[test]
fn set_leaves_no_temporary_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_in(&dir);

    set_in_file(&path, "install-deps", "false").unwrap();

    assert_eq!(file_names(path.parent().unwrap()), vec!["config.toml".to_owned()]);
}

#[test]
fn unset_removes_the_key_and_keeps_the_rest() {
    let (_dir, path) = write_config(
        "# where modules go\nlib = '/tmp/mods'\n# CI has no network\ninstall-deps = false\n",
    );

    assert!(unset_in_file(&path, "install-deps").unwrap());

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("# where modules go"), "{text}");
    assert!(!text.contains("install-deps"), "{text}");
    assert!(!text.contains("CI has no network"), "the comment above a key goes with it:\n{text}");
    assert_eq!(layer_from_file(&path).unwrap().install_deps, None);
    assert!(layer_from_file(&path).unwrap().lib.is_some());
}

#[test]
fn unset_of_a_key_that_is_not_there_changes_nothing() {
    let (_dir, path) = write_config("lib = '/tmp/mods'\n");

    assert!(!unset_in_file(&path, "install-deps").unwrap());

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "lib = '/tmp/mods'\n");
}

#[test]
fn unset_does_not_create_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_in(&dir);

    assert!(!unset_in_file(&path, "lib").unwrap());

    assert!(!path.exists());
}

#[test]
fn unset_refuses_an_unknown_key() {
    let dir = tempfile::tempdir().unwrap();
    assert!(unset_in_file(&config_in(&dir), "libb").is_err());
}

#[test]
fn a_setting_is_read_back_by_name() {
    let lib = abs("from-flag");
    let settings = merge(
        layers(layer(Some(false), Some(&lib)), layer(None, None), layer(None, None)),
        default_lib,
    )
    .unwrap();

    let (value, source) = settings.get("install-deps").unwrap();
    assert_eq!((value.as_str(), source), ("false", &Source::Flag));
    assert_eq!(settings.get("lib").unwrap().0, lib.display().to_string());
    assert!(settings.get("nope").is_err());
}

// ---- .Renviron ----

fn renviron_in(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("baler-renviron-{label}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(".Renviron")
}

#[test]
fn renviron_gets_a_marked_block_and_keeps_what_was_there() {
    let path = renviron_in("add");
    std::fs::write(&path, "A=1").unwrap();
    baler_core::settings::set_renviron_lib(&path, Some(Path::new("/mods"))).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("A=1\n"));
    assert!(text.contains("BALER_LIB=\"/mods\"\n"));
}

#[test]
fn renviron_block_is_replaced_not_repeated() {
    let path = renviron_in("replace");
    baler_core::settings::set_renviron_lib(&path, Some(Path::new("/one"))).unwrap();
    baler_core::settings::set_renviron_lib(&path, Some(Path::new("/two"))).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("BALER_LIB=").count(), 1);
    assert!(text.contains("/two"));
}

#[test]
fn renviron_block_is_removed_and_nothing_else() {
    let path = renviron_in("remove");
    std::fs::write(&path, "A=1\nB=2\n").unwrap();
    baler_core::settings::set_renviron_lib(&path, Some(Path::new("/mods"))).unwrap();
    baler_core::settings::set_renviron_lib(&path, None).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "A=1\nB=2\n");
}

#[test]
fn removing_from_a_missing_renviron_creates_nothing() {
    let path = renviron_in("missing");
    baler_core::settings::set_renviron_lib(&path, None).unwrap();
    assert!(!path.exists());
}

#[test]
fn a_block_without_its_closing_line_is_an_error_and_left_alone() {
    let path = renviron_in("broken");
    let broken = "A=1\n# >>> baler (managed by `baler config lib`) >>>\nBALER_LIB=\"/x\"\n";
    std::fs::write(&path, broken).unwrap();
    assert!(baler_core::settings::set_renviron_lib(&path, Some(Path::new("/y"))).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
}

#[test]
fn a_windows_path_is_written_with_forward_slashes() {
    let path = renviron_in("windows");
    baler_core::settings::set_renviron_lib(&path, Some(Path::new("C:\\Users\\me\\mods"))).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("BALER_LIB=\"C:/Users/me/mods\""));
}
