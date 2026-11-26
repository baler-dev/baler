use anyhow::{Context, Result};
use baler_core::settings::{self, SettingsLayer, Source};
use std::path::Path;

pub struct ConfigArgs {
    pub key: Option<String>,
    pub value: Option<String>,
    pub unset: bool,
    pub show_origin: bool,
}

pub fn run(args: ConfigArgs) -> Result<()> {
    match (args.key, args.value) {
        (None, _) => list(args.show_origin),
        (Some(key), Some(value)) => set(&key, &value),
        (Some(key), None) if args.unset => unset(&key),
        (Some(key), None) => get(&key),
    }
}

fn get(key: &str) -> Result<()> {
    let current = settings::current()?;
    let (value, _) = current.get(key)?;
    println!("{value}");
    Ok(())
}

fn list(show_origin: bool) -> Result<()> {
    let current = settings::current()?;
    for key in settings::KEYS {
        let (value, source) = current.get(key)?;
        if show_origin {
            println!("{key} = {value}  ({source})");
        } else {
            println!("{key} = {value}");
        }
    }
    Ok(())
}

fn set(key: &str, value: &str) -> Result<()> {
    let path = settings::user_config_path()?;
    settings::set_in_file(&path, key, value)?;
    println!("Wrote {key} to {}", path.display());
    if key == "lib" {
        // Read back the stored value, so `~` and relative paths are already resolved.
        let lib = settings::layer_from_file(&path)?.lib.context("lib was not saved")?;
        let renviron = settings::renviron_path()?;
        settings::set_renviron_lib(&renviron, Some(&lib))
            .with_context(|| format!("lib is saved in {}, but {} was not updated", path.display(), renviron.display()))?;
        println!("Set BALER_LIB in {}, which {{box}} reads. Restart R to use it.", renviron.display());
    }
    warn_if_overridden(key, &path)
}

fn unset(key: &str) -> Result<()> {
    let path = settings::user_config_path()?;
    if settings::unset_in_file(&path, key)? {
        println!("Removed {key} from {}", path.display());
    } else {
        println!("{key} is not set in {}", path.display());
    }
    if key == "lib" {
        let renviron = settings::renviron_path()?;
        settings::set_renviron_lib(&renviron, None)
            .with_context(|| format!("Could not update {}", renviron.display()))?;
    }
    warn_if_overridden(key, &path)
}

/// An environment variable beats the file, so say so when what was just
/// written is not the value that applies.
fn warn_if_overridden(key: &str, path: &Path) -> Result<()> {
    let after = settings::load(SettingsLayer::default())?;
    let (value, source) = after.get(key)?;
    if matches!(source, Source::Env(_)) {
        eprintln!(
            "warning: {key} is still {value}, from {source}, which takes precedence over {}",
            path.display()
        );
    }
    Ok(())
}
