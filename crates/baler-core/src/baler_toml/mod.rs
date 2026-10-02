use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use baler_native::detect::find_native_dirs;

mod author;
mod compiled_code;
mod dependencies;
mod extras;
mod project;
mod template;
mod tool;

pub use author::Author;
pub use compiled_code::{CompiledCode, NativePath};
pub use dependencies::{
    Dependencies, ModuleDep, ModuleDepSpec, ModuleSource, PackageDep, DEFAULT_CRAN_MIRROR,
};
pub use extras::{Extras, ExternalToolDep};
pub use project::ModuleMeta;
pub use tool::{TestConfig, ToolConfig};

/// `box` accepts either case for a module's `.r`/`.R` extension, so
/// baler's own entry-point check shouldn't hardcode one, a module
/// scaffolded with either convention, or hand-authored either way,
/// must resolve the same regardless of which case the author used or
/// which filesystem baler itself happens to be running on.
fn find_init_file(dir: &Path) -> Option<PathBuf> {
    for name in ["__init__.r", "__init__.R"] {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

// ---- BalerToml (For metadata file) ----

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BalerToml {
    pub project: ModuleMeta,
    /// `[dependencies]` and `[dependencies.baler]`. Runtime deps: R
    /// packages loaded by this module's code, and box modules it
    /// imports. Top-level, a sibling of `[project]`, not nested under
    /// it, matching where `[compiled-code]` and `[tool]` already sit.
    #[serde(default)]
    pub dependencies: Dependencies,
    /// `[extras]`, `[extras.baler]`, and `[extras.external]`. Deps
    /// only needed outside normal runtime use: tests and mocks
    /// (`packages`/`baler`, same shape as `[dependencies]`), plus
    /// external CLI tools like Quarto or a linter (`external`, which
    /// has no `[dependencies]` counterpart at all). Named after
    /// Julia's `Project.toml` `[extras]`, not Cargo's
    /// `dev-dependencies`, since that name is already spoken for by
    /// the Cargo workspace this tool itself is built with.
    pub extras: Option<Extras>,
    #[serde(rename = "compiled-code")]
    pub compiled_code: Option<CompiledCode>,
    pub tool: Option<ToolConfig>,
}

impl BalerToml {
    pub fn from_dir(project_root: &Path) -> Result<Self> {
        let toml_path = project_root.join("baler.toml");
        let contents = std::fs::read_to_string(&toml_path)
            .with_context(|| format!(
                "Could not read baler.toml at {}. \
                 Run `baler init` to create one.",
                toml_path.display()
            ))?;
        let parsed: Self = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse baler.toml at {}", toml_path.display()))?;
        parsed.project.semver()
            .with_context(|| format!("Invalid `version` in {}", toml_path.display()))?;
        parsed.validate_module_deps()
            .with_context(|| format!("Invalid dependency in {}", toml_path.display()))?;
        parsed.validate_external_tools()
            .with_context(|| format!("Invalid external tool in {}", toml_path.display()))?;
        Ok(parsed)
    }

    /// Fails early on a module dependency whose keys contradict each
    /// other (`git` together with `url`, a `tag` without `git`, ...),
    /// instead of waiting until something tries to fetch it.
    fn validate_module_deps(&self) -> Result<()> {
        let tables = [
            self.dependencies.baler.as_ref(),
            self.extras.as_ref().and_then(|e| e.baler.as_ref()),
        ];
        for (name, dep) in tables.into_iter().flatten().flatten() {
            dep.source()
                .with_context(|| format!("Module dependency '{name}'"))?;
        }
        Ok(())
    }

    /// An `[extras.external]` key is looked up as a command name on
    /// PATH, so it must be a bare name. A path separator would make
    /// `Path::join` discard or escape the PATH directory it is joined to.
    fn validate_external_tools(&self) -> Result<()> {
        let Some(tools) = self.extras.as_ref().and_then(|e| e.external.as_ref()) else {
            return Ok(());
        };
        for name in tools.keys() {
            if name.is_empty() || name.contains(['/', '\\']) {
                bail!("External tool '{name}' must be a bare command name, not a path.");
            }
        }
        Ok(())
    }

    pub fn resolve_src_dir(&self, project_root: &Path) -> Result<PathBuf> {
        if let Some(src) = &self.project.src {
            let dir = project_root.join(src);
            if !dir.is_dir() {
                bail!("`src` path '{}' is not a directory.", dir.display());
            }
            if find_init_file(&dir).is_none() {
                bail!(
                    "No `__init__.r` (or `__init__.R`) found in `src` directory '{}'.\n\
                     `__init__.r` is required as the module entry point.",
                    dir.display()
                );
            }
            return Ok(dir);
        }

        let dir = project_root.join(&self.project.name);
        if !dir.is_dir() {
            bail!(
                "Source directory '{}' not found in '{}'.\n\
                 The source directory must match the module name '{}', \
                 or set `src` in baler.toml to point to the correct directory.",
                self.project.name,
                project_root.display(),
                self.project.name,
            );
        }
        if find_init_file(&dir).is_none() {
            bail!(
                "No `__init__.r` (or `__init__.R`) found in '{}'.\n\
                 `__init__.r` is required as the module entry point, \
                 similar to NAMESPACE in R packages.",
                dir.display()
            );
        }
        Ok(dir)
    }

    /// Every native code location this module actually has.
    /// `[compiled-code].path`, when set, is resolved relative to the
    /// module's own source directory, not the project root, a module
    /// can write `path = "cpp"` for one location or
    /// `path = ["cpp", "extra/src"]` for several, both resolved
    /// against that module's own source tree. Without it, this scans
    /// the whole module source tree for compiled-code dirs.
    pub fn resolve_native_dirs(&self, project_root: &Path) -> Result<Vec<PathBuf>> {
        let src_dir = self.resolve_src_dir(project_root)?;
        Ok(self.native_dirs_under(&src_dir))
    }

    /// Same resolution `resolve_native_dirs` does, but rooted directly
    /// at a known source directory instead of a project root that
    /// still needs `resolve_src_dir`'s name-matching. Used post-install,
    /// where the unpacked module directory already *is* the source
    /// dir, there is no project root wrapping it to resolve against.
    pub fn native_dirs_under(&self, src_dir: &Path) -> Vec<PathBuf> {
        if let Some(path) = self.compiled_code.as_ref().and_then(|n| n.path.as_ref()) {
            return path.as_paths().into_iter().map(|p| src_dir.join(p)).collect();
        }
        find_native_dirs(src_dir)
    }

    pub fn has_native_code(&self, project_root: &Path) -> Result<bool> {
        Ok(!self.resolve_native_dirs(project_root)?.is_empty())
    }
}
