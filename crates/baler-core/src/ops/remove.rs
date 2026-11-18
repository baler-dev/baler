use anyhow::{bail, Context, Result};
use std::io::{self, Write};
use std::path::Path;

use crate::installed;
use crate::paths::resolve_install_dir;

pub fn run(name: &str, force: bool) -> Result<()> {
    let install_dir = resolve_install_dir()
        .context("Failed to resolve install directory")?;

    remove_from(&install_dir, name, force)
}

/// Removes the module `name` from `install_dir`: its directory, and the
/// `.dist-info` folder of every version baler kept for it.
///
/// A `.dist-info` folder with no module directory is what baler 0.1.0
/// left behind when it removed a module. Removing the name cleans those
/// up too, instead of reporting that nothing is installed.
pub fn remove_from(install_dir: &Path, name: &str, force: bool) -> Result<()> {
    installed::validate_module_name(name)?;

    let module_path = install_dir.join(name);

    if !module_path.exists() {
        let leftovers = installed::remove_dist_infos(install_dir, name)?;
        if leftovers == 0 {
            bail!(
                "Module '{}' is not installed ({} not found).",
                name,
                module_path.display()
            );
        }
        println!(
            "'{}' is not installed, but metadata left over from an earlier removal was cleaned up.",
            name
        );
        return Ok(());
    }

    if !force {
        let confirmed = prompt_confirm(&format!(
            "Remove '{}'? This cannot be undone. [y/N] ",
            name
        ))?;

        if !confirmed {
            println!("Aborted.");
            return Ok(());
        }
    }

    std::fs::remove_dir_all(&module_path)
        .with_context(|| format!("Failed to remove: {}", module_path.display()))?;
    installed::remove_dist_infos(install_dir, name)?;

    println!("Removed '{}' from {}.", name, module_path.display());

    Ok(())
}

fn prompt_confirm(message: &str) -> Result<bool> {
    print!("{}", message);
    io::stdout().flush().context("Failed to flush stdout")?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("Failed to read input")?;

    Ok(matches!(input.trim().to_lowercase().as_str(), "y" | "yes"))
}
