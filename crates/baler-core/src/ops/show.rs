use anyhow::{Context, Result};

use crate::baler_toml::{Author, DEFAULT_CRAN_MIRROR};
use crate::installed::{self, InstalledModule};
use crate::paths::resolve_install_dir;

/// `baler show <name>`: what one installed module declared, and where its
/// files are.
pub fn run(name: &str) -> Result<()> {
    let install_dir = resolve_install_dir().context("Failed to resolve install directory")?;
    let module = installed::find(&install_dir, name)?;
    print!("{}", render(&module));
    Ok(())
}

/// The text `baler show` prints.
pub fn render(module: &InstalledModule) -> String {
    let project = &module.toml.project;
    let mut out = format!("{} {}\n", module.name, module.version);
    if !project.description.trim().is_empty() {
        out.push_str(&format!("{}\n", project.description.trim()));
    }
    out.push('\n');

    let mut rows: Vec<(&str, String)> = vec![("License", project.license.clone())];
    rows.push(("License file", files_or_none(&module.license_files())));
    rows.push(("README", files_or_none(&module.readme_file().into_iter().collect::<Vec<_>>())));
    if !project.authors.is_empty() {
        let authors: Vec<String> = project.authors.iter().map(author_line).collect();
        rows.push(("Authors", authors.join(", ")));
    }
    rows.push(("R", project.r_version.clone()));
    if let Some(repository) = project.repository.as_deref().filter(|r| !r.trim().is_empty()) {
        rows.push(("Repository", repository.to_owned()));
    }
    if !project.keywords.is_empty() {
        rows.push(("Keywords", project.keywords.join(", ")));
    }
    rows.push(("Location", module.module_dir.display().to_string()));
    rows.push(("Metadata", module.dist_info.display().to_string()));
    rows.push(("Locked", yes_no(module.lock_file().is_some())));
    let native = if module.has_compiled_code() { "need compilation" } else { "none" };
    rows.push(("Native code", native.to_owned()));

    let width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0) + 1;
    for (label, value) in &rows {
        let label = format!("{label}:");
        out.push_str(&format!("{label:<width$} {value}\n"));
    }

    let dependencies = &module.toml.dependencies;
    if !dependencies.packages.is_empty() {
        out.push_str("\nR packages:\n");
        for (name, dep) in &dependencies.packages {
            match dep.explicit_repo().filter(|repo| *repo != DEFAULT_CRAN_MIRROR) {
                Some(repo) => out.push_str(&format!("  {} ({}) [{}]\n", name, dep.version(), repo)),
                None => out.push_str(&format!("  {} ({})\n", name, dep.version())),
            }
        }
    }
    if let Some(modules) = dependencies.baler.as_ref().filter(|m| !m.is_empty()) {
        out.push_str("\nbaler modules:\n");
        for (name, dep) in modules {
            out.push_str(&format!("  {} ({})\n", name, dep.version()));
        }
    }

    out
}

fn yes_no(value: bool) -> String {
    if value { "yes" } else { "no" }.to_owned()
}

fn author_line(author: &Author) -> String {
    match author.email() {
        Some(email) => format!("{} <{}>", author.name(), email),
        None => author.name().to_owned(),
    }
}

/// The file names, or `none` when there are no files: a module installed
/// by baler 0.1.0 kept neither a license nor a README.
fn files_or_none(files: &[std::path::PathBuf]) -> String {
    let names: Vec<String> = files
        .iter()
        .filter_map(|path| path.file_name().and_then(|n| n.to_str()).map(str::to_owned))
        .collect();
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}
