//! Settings that belong to the machine, not to a module: where installed
//! modules live, and whether `baler install` also installs dependencies.
//!
//! Each setting is looked up in layers, highest priority first:
//!
//! 1. a command line flag (`--lib`, `--install-deps`, `--no-install-deps`)
//! 2. an environment variable (`BALER_LIB`, `BALER_INSTALL_DEPS`)
//! 3. the user config file, `~/.baler/config.toml`
//! 4. the built-in default
//!
//! A layer only decides the settings it sets. These are settings of the
//! environment, never of resolution, so they must not change what
//! `baler lock` writes.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const LIB_ENV: &str = "BALER_LIB";
pub const INSTALL_DEPS_ENV: &str = "BALER_INSTALL_DEPS";
const CONFIG_FILE: &str = "config.toml";

/// What one layer says. `None` means the layer does not set it.
#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct SettingsLayer {
    pub install_deps: Option<bool>,
    pub lib: Option<PathBuf>,
}

/// The layer a setting's value came from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Flag,
    Env(&'static str),
    File(PathBuf),
    Default,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Flag => write!(f, "command line"),
            Source::Env(name) => write!(f, "environment ({name})"),
            Source::File(path) => write!(f, "{}", path.display()),
            Source::Default => write!(f, "default"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Setting<T> {
    pub value: T,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub install_deps: Setting<bool>,
    pub lib: Setting<PathBuf>,
}

/// Picks, for each setting, the value of the first layer that sets it.
/// `layers` is ordered from the highest priority to the lowest.
/// `default_lib` is only called when no layer sets `lib`.
pub fn merge(
    layers: Vec<(Source, SettingsLayer)>,
    default_lib: impl FnOnce() -> Result<PathBuf>,
) -> Result<Settings> {
    let install_deps = layers
        .iter()
        .find_map(|(source, layer)| {
            layer.install_deps.map(|value| Setting { value, source: source.clone() })
        })
        .unwrap_or(Setting { value: true, source: Source::Default });

    let lib = match layers.iter().find_map(|(source, layer)| {
        layer.lib.clone().map(|value| Setting { value, source: source.clone() })
    }) {
        Some(setting) => setting,
        None => Setting { value: default_lib()?, source: Source::Default },
    };

    Ok(Settings { install_deps, lib })
}

/// Reads `BALER_LIB` and `BALER_INSTALL_DEPS`. Empty values count as unset.
pub fn layer_from_env() -> Result<SettingsLayer> {
    let lib = match std::env::var_os(LIB_ENV) {
        Some(value) if !value.is_empty() => Some(absolute(Path::new(&value))?),
        _ => None,
    };

    let install_deps = match std::env::var_os(INSTALL_DEPS_ENV) {
        Some(value) if !value.is_empty() => {
            let text = value.to_str().with_context(|| format!("{INSTALL_DEPS_ENV} is not valid text"))?;
            Some(parse_bool(text).with_context(|| format!("Invalid {INSTALL_DEPS_ENV}"))?)
        }
        _ => None,
    };

    Ok(SettingsLayer { install_deps, lib })
}

/// Reads the config file at `path`. A missing file is an empty layer.
/// Unknown keys are an error, so a typo is not silently ignored.
pub fn layer_from_file(path: &Path) -> Result<SettingsLayer> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SettingsLayer::default()),
        Err(e) => return Err(e).with_context(|| format!("Could not read {}", path.display())),
    };
    parse_layer(&contents, path)
}

/// Parses the text of a config file. `path` is only used in error messages.
pub fn parse_layer(contents: &str, path: &Path) -> Result<SettingsLayer> {
    let mut layer: SettingsLayer = toml::from_str(contents)
        .with_context(|| format!("Invalid config file {}", path.display()))?;
    if let Some(lib) = layer.lib.take() {
        let expanded = expand_file_path(&lib)
            .with_context(|| format!("Invalid `lib` in {}", path.display()))?;
        layer.lib = Some(expanded);
    }
    Ok(layer)
}

/// `~/.baler/config.toml`.
pub fn user_config_path() -> Result<PathBuf> {
    Ok(crate::paths::baler_home_dir()?.join(CONFIG_FILE))
}

/// Turns a path given on the command line or in the environment into an
/// absolute one, relative to the current directory.
pub fn absolute(path: &Path) -> Result<PathBuf> {
    std::path::absolute(path).with_context(|| format!("Could not make '{}' absolute", path.display()))
}

/// Paths in a config file have no current directory to be relative to, so
/// they must be absolute once a leading `~` is expanded.
pub fn expand_file_path(path: &Path) -> Result<PathBuf> {
    let expanded = match path.strip_prefix("~") {
        Ok(rest) => crate::paths::home_dir()?.join(rest),
        Err(_) => path.to_path_buf(),
    };
    if !expanded.is_absolute() {
        bail!("'{}' is not an absolute path. Use a full path, or start it with ~/", path.display());
    }
    Ok(expanded)
}

pub fn parse_bool(text: &str) -> Result<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        other => bail!("'{other}' is not a boolean. Use true or false"),
    }
}

/// Builds the settings from the command line layer and everything below it.
pub fn load(cli: SettingsLayer) -> Result<Settings> {
    // No home directory only matters when the config file or the default
    // install directory is needed, so it is not an error here.
    let file = match user_config_path() {
        Ok(path) => layer_from_file(&path)
            .map(|layer| (Source::File(path), layer))?,
        Err(_) => (Source::Default, SettingsLayer::default()),
    };
    let env = layer_from_env()?;
    let layers = vec![
        (Source::Flag, cli),
        (Source::Env(LIB_ENV), SettingsLayer { install_deps: None, lib: env.lib }),
        (Source::Env(INSTALL_DEPS_ENV), SettingsLayer { install_deps: env.install_deps, lib: None }),
        file,
    ];
    merge(layers, crate::paths::default_install_dir)
}

static CURRENT: OnceLock<Settings> = OnceLock::new();

/// Called once by the CLI with the flags it was given. A later call keeps
/// the first value.
pub fn init(cli: SettingsLayer) -> Result<&'static Settings> {
    let settings = load(cli)?;
    Ok(CURRENT.get_or_init(|| settings))
}

/// The settings `init` built. Without `init`, as in library use and in
/// tests, they are built from the environment and config file each call.
pub fn current() -> Result<Settings> {
    match CURRENT.get() {
        Some(settings) => Ok(settings.clone()),
        None => load(SettingsLayer::default()),
    }
}
