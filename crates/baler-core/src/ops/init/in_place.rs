use anyhow::{anyhow, bail, Context, Result};

use std::fs;
use std::path::Path;

use baler_native::{Backend, NativeLang};

use super::naming::{project_dir_name, validate_module_name};
use super::project_dir::{explicit_src, is_fresh};
use crate::baler_toml::{BalerToml, TemplateDefaults};

#[derive(Clone, Copy, Default)]
pub struct InPlaceOptions<'a> {
    /// Module name. Defaults to the project directory's own name, minus a
    /// `-proj` suffix (what `baler init <name>` appends).
    pub name: Option<&'a str>,
    /// The directory holding the module's source, relative to the project.
    /// Written to `src` in baler.toml. Defaults to a directory named after
    /// the module.
    pub src: Option<&'a str>,
    pub native: Option<NativeLang>,
    pub backend: Option<Backend>,
}

/// Initializes `project_root`, which already exists, like `uv init` or
/// `cargo init`. It never overwrites or deletes a file, and it never
/// looks at the code in the project.
///
/// - An empty directory (hidden entries such as `.git` don't count) is
///   set up the way `baler init <name>` does it, only in place:
///   `baler.toml`, `README.md`, and an example module in `<name>/`.
/// - Any other directory gets `baler.toml` and nothing else. `src` says
///   where the module's source is: a directory named after the module by
///   default, or whatever `--src` names.
///
/// Everything that can fail is checked before the first file is written,
/// and `baler.toml` is written last, so a failed run leaves the directory
/// ready for another attempt.
pub fn run_in_place(project_root: &Path, opts: &InPlaceOptions) -> Result<()> {
    if !project_root.is_dir() {
        bail!("'{}' is not a directory.", project_root.display());
    }
    if project_root.join("baler.toml").exists() {
        bail!(
            "baler.toml already exists in '{}', so there is nothing to initialize.",
            project_root.display()
        );
    }

    let name = match opts.name {
        Some(n) => {
            validate_module_name(n)?;
            n.to_owned()
        }
        None => {
            let n = project_dir_name(project_root).ok_or_else(|| {
                anyhow!("Could not work out a module name. Pass `--name <name>`.")
            })?;
            validate_module_name(&n)?;
            n
        }
    };
    let src = opts.src.map(|raw| explicit_src(project_root, raw)).transpose()?;
    let fresh = is_fresh(project_root)?;

    if opts.native.is_some() && !fresh {
        bail!(
            "`--native` scaffolds a new module, but this directory already has files in it. \
             Add the native code yourself (a `src/` directory with a Makevars) and leave \
             `--native` off."
        );
    }

    let module_rel = src.clone().unwrap_or_else(|| name.clone());
    let mut files: Vec<String> = Vec::new();

    if fresh {
        let module_dir = project_root.join(&name);
        fs::create_dir_all(&module_dir).with_context(|| {
            format!("Failed to create source directory: {}", module_dir.display())
        })?;
        let result = match opts.native {
            Some(lang) => baler_native::scaffold::scaffold(&module_dir, &name, lang, opts.backend),
            None => baler_native::scaffold::scaffold_pure_r(&module_dir),
        };
        let scaffolded = match result {
            Ok(written) => written,
            Err(e) => {
                // Only the directory created just above, inside a project that
                // had nothing in it. Left behind, it would make a retry think
                // the project already has files and skip the scaffold.
                let _ = fs::remove_dir_all(&module_dir);
                return Err(e.context(format!(
                    "Failed to scaffold the example module in {}",
                    module_dir.display()
                )));
            }
        };
        files.extend(scaffolded.into_iter().map(|f| format!("{name}/{f}")));
        fs::write(
            project_root.join("README.md"),
            format!("# {}\n\nA box module.\n", name),
        )
        .context("Failed to write README.md")?;
        files.insert(0, "README.md".to_string());
    }

    let manifest = BalerToml::default_template_with(
        &name,
        opts.native.map(|lang| (lang, opts.backend)),
        TemplateDefaults { src: (module_rel != name).then_some(module_rel.as_str()), ..Default::default() },
    );
    fs::write(project_root.join("baler.toml"), manifest).context("Failed to write baler.toml")?;
    files.insert(0, "baler.toml".to_string());

    println!("Initialized module '{}' in '{}'", name, project_root.display());
    for f in &files {
        println!("  {}", f);
    }
    println!();
    if fresh {
        println!(
            "Source directory: '{}/'\n\
             Rename it and set `src` in baler.toml if you prefer a different name.",
            module_rel
        );
    } else if project_root.join(&module_rel).is_dir() {
        println!("Source directory: '{}/'. No files in the project were changed.", module_rel);
    } else {
        eprintln!(
            "warning: the source directory '{}/' doesn't exist. Set `src` in baler.toml to the \
             directory holding your module, or run `baler init --src <dir>`.",
            module_rel
        );
    }

    Ok(())
}
