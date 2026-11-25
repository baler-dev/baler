//! Settings that belong to the machine, not to a module: where installed
//! modules live, and whether `baler install` also installs dependencies.
//!
//! Each setting is looked up in layers, highest priority first:
//!
//! 1. a command line flag (`--install-deps`, `--no-install-deps`)
//! 2. an environment variable (`BALER_LIB`, `BALER_INSTALL_DEPS`)
//! 3. the user config file, `~/.baler/config.toml`
//! 4. the built-in default
//!
//! A layer only decides the settings it sets. These are settings of the
//! environment, never of resolution, so they must not change what
//! `baler lock` writes.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::io::Write;
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
        .unwrap_or(Setting { value: false, source: Source::Default });

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

/// The settings `baler config` knows, in the order `list` shows them.
pub const KEYS: [&str; 2] = ["install-deps", "lib"];

fn unknown_key(key: &str) -> anyhow::Error {
    anyhow::anyhow!("Unknown setting '{key}'. The settings are: {}", KEYS.join(", "))
}

impl Settings {
    /// The value of the setting named `key` as text, and where it came from.
    pub fn get(&self, key: &str) -> Result<(String, &Source)> {
        match key {
            "install-deps" => Ok((self.install_deps.value.to_string(), &self.install_deps.source)),
            "lib" => Ok((self.lib.value.display().to_string(), &self.lib.source)),
            other => Err(unknown_key(other)),
        }
    }
}

/// Sets `key` in the config file at `path`, creating the file and its
/// directory when they are missing. Everything else in the file, comments
/// included, is kept as it was. The value is checked before anything is
/// written, and so is the whole file after the change, so a bad file is
/// never made worse.
pub fn set_in_file(path: &Path, key: &str, value: &str) -> Result<()> {
    let item = match key {
        "install-deps" => {
            let on = parse_bool(value).with_context(|| format!("Invalid value for {key}"))?;
            toml_edit::value(on)
        }
        "lib" => toml_edit::value(lib_as_stored(value).with_context(|| format!("Invalid value for {key}"))?),
        other => return Err(unknown_key(other)),
    };
    let mut document = read_document(path)?;
    document[key] = item;
    write_checked(path, &document.to_string())
}

/// Removes `key` from the config file at `path`, so the layers below it
/// decide again. Says whether the key was there. A comment directly above
/// the key goes with it, since it describes that key.
pub fn unset_in_file(path: &Path, key: &str) -> Result<bool> {
    if !KEYS.contains(&key) {
        return Err(unknown_key(key));
    }
    let mut document = read_document(path)?;
    if document.remove(key).is_none() {
        return Ok(false);
    }
    write_checked(path, &document.to_string())?;
    Ok(true)
}

/// A `~/...` path is stored as typed, so the file still works on another
/// machine. Any other path is stored absolute, relative to the current
/// directory, since the file has no current directory of its own.
fn lib_as_stored(value: &str) -> Result<String> {
    let path = Path::new(value);
    // Anything that starts with ~ must be ~ or ~/..., not ~name/..., which
    // would otherwise end up as a directory called ~name under the current one.
    let stored = if value.starts_with('~') {
        expand_file_path(path)?;
        path.to_path_buf()
    } else {
        absolute(path)?
    };
    stored.to_str().map(str::to_owned).context("The path is not valid text")
}

fn read_document(path: &Path) -> Result<toml_edit::DocumentMut> {
    match std::fs::read_to_string(path) {
        Ok(contents) => contents
            .parse::<toml_edit::DocumentMut>()
            .with_context(|| format!("Invalid config file {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml_edit::DocumentMut::new()),
        Err(e) => Err(e).with_context(|| format!("Could not read {}", path.display())),
    }
}

/// Checks `text` as a config file, then replaces `path` with it.
fn write_checked(path: &Path, text: &str) -> Result<()> {
    parse_layer(text, path)?;
    write_atomic(path, text)
}

/// Replaces `path` through a temporary file next to it, so a crash cannot
/// leave half a file.
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let parent = path.parent().with_context(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent).with_context(|| format!("Could not create {}", parent.display()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("Could not write in {}", parent.display()))?;
    temp.write_all(text.as_bytes()).with_context(|| format!("Could not write {}", path.display()))?;
    temp.persist(path).map_err(|e| e.error).with_context(|| format!("Could not replace {}", path.display()))?;
    Ok(())
}

const RENVIRON_BEGIN: &str = "# >>> baler (managed by `baler config lib`) >>>";
const RENVIRON_END: &str = "# <<< baler <<<";

/// The `.Renviron` R reads at startup: `R_ENVIRON_USER` when it is set,
/// otherwise `.Renviron` in `R_USER` or in the home directory.
pub fn renviron_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("R_ENVIRON_USER").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let dir = match std::env::var_os("R_USER").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => crate::paths::home_dir()?,
    };
    Ok(dir.join(".Renviron"))
}

/// Makes the `.Renviron` at `path` set `BALER_LIB` to `lib`, or stop
/// setting it when `lib` is `None`. `{box}` reads `BALER_LIB`, and a
/// program cannot change the environment of an R session that is already
/// running, so this is how a saved `lib` reaches it. Only the block baler
/// wrote, between two marker comments, is ever changed.
pub fn set_renviron_lib(path: &Path, lib: Option<&Path>) -> Result<()> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("Could not read {}", path.display())),
    };
    let stripped = strip_block(&existing).with_context(|| format!("Invalid {}", path.display()))?;
    let mut text = match (&stripped, lib) {
        (None, None) => return Ok(()),
        (Some(rest), _) => rest.clone(),
        (None, Some(_)) => existing,
    };
    if let Some(lib) = lib {
        // R reads these values as they are, so a Windows path uses `/`.
        let lib = lib.to_str().context("The path is not valid text")?.replace('\\', "/");
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!("{RENVIRON_BEGIN}\nBALER_LIB=\"{lib}\"\n{RENVIRON_END}\n"));
    }
    write_atomic(path, &text)
}

/// `text` without baler's block, or `None` when it has no such block.
fn strip_block(text: &str) -> Result<Option<String>> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(start) = lines.iter().position(|line| line.trim() == RENVIRON_BEGIN) else {
        return Ok(None);
    };
    let end = lines[start..]
        .iter()
        .position(|line| line.trim() == RENVIRON_END)
        .map(|offset| start + offset)
        .context("The baler block has no closing line. Remove it by hand")?;
    let mut kept = lines[..start].to_vec();
    kept.extend(&lines[end + 1..]);
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    Ok(Some(out))
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
