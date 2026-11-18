use anyhow::{Context, Result};

use crate::installed::{self, Listing};
use crate::paths::resolve_install_dir;

/// `baler list`: the modules installed in the install directory.
pub fn run() -> Result<()> {
    let install_dir = resolve_install_dir().context("Failed to resolve install directory")?;
    let listing = installed::list(&install_dir)?;

    for problem in &listing.problems {
        eprintln!("warning: could not read the record of {problem}");
    }
    print!("{}", render(&listing, &install_dir.display().to_string()));
    Ok(())
}

/// The table `baler list` prints, one line for each module. `location`
/// is the install directory, named only when nothing is installed.
pub fn render(listing: &Listing, location: &str) -> String {
    if listing.modules.is_empty() {
        return format!("No modules installed in {location}.\n");
    }

    let header = ["NAME", "VERSION", "LICENSE", "R"];
    let mut rows: Vec<[String; 4]> = vec![header.map(str::to_owned)];
    for module in &listing.modules {
        rows.push([
            module.name.clone(),
            module.version.clone(),
            module.toml.project.license.clone(),
            module.toml.project.r_version.clone(),
        ]);
    }

    let widths: Vec<usize> = (0..header.len())
        .map(|column| rows.iter().map(|row| row[column].chars().count()).max().unwrap_or(0))
        .collect();

    let mut out = String::new();
    for row in &rows {
        let cells: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        out.push_str(cells.join("  ").trim_end());
        out.push('\n');
    }
    out
}
