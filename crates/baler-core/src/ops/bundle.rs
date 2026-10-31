use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use crate::baler_toml::BalerToml;
use crate::formats::tar;
use crate::lockfile;
use crate::ops::resolve;

/// Bundle without insisting on a current lock: a stale `baler.lock` is
/// reported as a warning. See `run_with`.
pub fn run(path: &str) -> Result<()> {
    run_with(path, false)
}

/// `binary` and `keep_source` come from `[compiled-code]` in
/// baler.toml, not from flags. `keep_source` without `binary` is
/// rejected when the manifest is parsed (`BalerToml::from_dir`).
///
/// A bundle ships `baler.lock` as it is, and whoever installs the bundle
/// has no project to run `baler lock` in. So a lock that doesn't match
/// `baler.toml` is caught here, on the author's side. The check is offline
/// and only looks at what `baler.toml` declares (see
/// `resolve::lock_problems_for`). With `locked: false` it warns and bundles
/// anyway. With `locked: true`, in the style of `cargo publish --locked`,
/// it fails, and a missing or unreadable lock fails too.
pub fn run_with(path: &str, locked: bool) -> Result<()> {
    bundle_project(path, locked, &[])
}

/// `run_with` plus `--merge`: bundles the project as usual, which compiles
/// for this platform when `binary = true`, and also brings in the
/// binaries of the bundles in `archives`, made on other platforms. Each
/// archive must come from the same files as the project. Needs
/// `binary = true` under `[compiled-code]`, since a bundle only ships
/// binaries then.
///
/// Like a normal bundle, it writes `<name>_<version>.tar.gz` in the
/// current directory and replaces an archive already there, so list it
/// among `archives` to keep what it holds.
pub fn merge_with(path: &str, locked: bool, archives: &[String]) -> Result<()> {
    bundle_project(path, locked, archives)
}

fn bundle_project(path: &str, locked: bool, merge: &[String]) -> Result<()> {
    let project_root = PathBuf::from(path);

    if !project_root.exists() {
        bail!("Path does not exist: {}", project_root.display());
    }
    if !project_root.is_dir() {
        bail!("Path is not a directory: {}", project_root.display());
    }

    let toml = BalerToml::from_dir(&project_root)?;
    check_lock_before_bundling(&project_root, &toml, locked)?;
    let src_path = toml.resolve_src_dir(&project_root)?;
    let meta = &toml.project;

    let (binary, keep_source) = toml
        .compiled_code
        .as_ref()
        .map(|c| (c.binary, c.keep_source))
        .unwrap_or((false, false));

    // Checked before compiling, so a mistake does not cost a build.
    if !merge.is_empty() {
        if !binary {
            bail!(
                "--merge needs `binary = true` under [compiled-code] in baler.toml, since only then does a bundle ship binaries."
            );
        }
        for archive in merge {
            if !Path::new(archive).is_file() {
                bail!("Archive does not exist: {}", archive);
            }
        }
    }

    // Set when a binary was compiled here: names the `.lib/<tag>/` folder
    // this platform's binaries are stored under in the bundle.
    let mut platform_tag: Option<String> = None;

    if binary {
        // Compiled here for the .lib/ artifacts it produces. The platform
        // it compiled for names the folder those artifacts are stored
        // under in the bundle.
        let compiled =
            crate::ops::compile::run(&project_root, crate::ops::compile::CompileMode::Normal)?;
        match compiled.first() {
            Some(first) => {
                platform_tag = Some(tar::platform_tag(&first.target_triple, &first.r_version));
            }
            None => eprintln!(
                "warning: [compiled-code] sets `binary = true`, but '{}' has no \
                 native code to compile. Bundling without a binary.",
                meta.name
            ),
        }
    }

    // .lib/ is dot-prefixed, so the archive writer's own hidden-file
    // filter excludes it from a plain bundle automatically, no
    // explicit exclusion needed there. `binary = true` needs the
    // opposite: force it back in despite the dot, since shipping it is
    // the whole point. `binary = true` without `keep_source`
    // additionally excludes native source; a machine none of the
    // bundled binaries fit then has nothing to fall back to, and
    // install fails with a message naming the platforms (see
    // `install/native.rs`). The platform each binary was built for is
    // recorded in the `.abi.json` sidecar that ships next to it.
    //
    // Binaries are stored per platform (`.lib/<tag>/`). If the output
    // archive already exists and holds other platforms' binaries for the
    // same files, they are kept, so running `baler bundle` on each OS
    // against the same archive builds one bundle for all of them.
    let (exclude, force_include): (Vec<PathBuf>, Vec<PathBuf>) = if binary {
        let lib_dirs: Vec<PathBuf> = toml
            .resolve_native_dirs(&project_root)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|d| d.parent().map(|p| p.join(".lib")))
            .collect();
        let src_exclude = if keep_source {
            Vec::new()
        } else {
            toml.resolve_native_dirs(&project_root).unwrap_or_default()
        };
        (src_exclude, lib_dirs)
    } else {
        (Vec::new(), Vec::new())
    };

    let cwd = std::env::current_dir()
        .context("Failed to get current working directory")?;

    let output_path = cwd.join(format!("{}_{}.tar.gz", meta.name, meta.version));

    let spec = tar::BundleSpec {
        src_path: &src_path,
        project_root: &project_root,
        output_path: &output_path,
        name: &meta.name,
        version: &meta.version,
        exclude: &exclude,
        force_include: &force_include,
    };
    let platforms = if !merge.is_empty() {
        let inputs: Vec<PathBuf> = merge.iter().map(PathBuf::from).collect();
        tar::merge_bundles(&spec, &inputs, platform_tag.as_deref())
    } else {
        match &platform_tag {
            Some(tag) => tar::bundle_with_binary(&spec, tag),
            None => tar::bundle(
                &src_path,
                &project_root,
                &output_path,
                &meta.name,
                &meta.version,
                &exclude,
                &force_include,
            )
            .map(|()| Vec::new()),
        }
    }
    .with_context(|| format!("Failed to bundle: {}", src_path.display()))?;

    println!(
        "Bundled '{}' ({}) -> {}",
        meta.name,
        meta.version,
        output_path.display()
    );
    if !merge.is_empty() {
        println!("Merged {} archive(s).", merge.len());
    }
    if !platforms.is_empty() {
        println!("Binaries in the bundle: {}", platforms.join(", "));
    }

    Ok(())
}

fn check_lock_before_bundling(project_root: &Path, toml: &BalerToml, locked: bool) -> Result<()> {
    let lock = match lockfile::read(project_root) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            if locked {
                bail!(
                    "--locked needs a {}, and there isn't one. Run `baler lock` first.",
                    lockfile::LOCK_FILE_NAME
                );
            }
            return Ok(());
        }
        Err(e) => {
            if locked {
                return Err(e.context(format!(
                    "--locked needs a readable {}",
                    lockfile::LOCK_FILE_NAME
                )));
            }
            eprintln!(
                "warning: {} can't be read ({:#}). It will be bundled as it is.",
                lockfile::LOCK_FILE_NAME,
                e
            );
            return Ok(());
        }
    };

    let problems = resolve::lock_problems_for(toml, &lock)?;
    if problems.is_empty() {
        return Ok(());
    }

    let list = problems.join("\n  - ");
    if locked {
        bail!(
            "{} is out of date with baler.toml:\n  - {}\nRun `baler lock`, then bundle again.",
            lockfile::LOCK_FILE_NAME,
            list
        );
    }
    eprintln!(
        "warning: {} is out of date with baler.toml:\n  - {}\n\
         It will be bundled as it is, and installing the bundle will fail. \
         Run `baler lock` first, or pass --locked to make this an error.",
        lockfile::LOCK_FILE_NAME,
        list
    );
    Ok(())
}

/// Used by `install` when bundling a GitHub-downloaded module.
pub fn bundle_to(project_root: &Path, output_path: &Path) -> Result<()> {
    let toml = BalerToml::from_dir(project_root)?;
    let src_path = toml.resolve_src_dir(project_root)?;
    let meta = &toml.project;

    tar::bundle(
        &src_path,
        project_root,
        output_path,
        &meta.name,
        &meta.version,
        &[],
        &[],
    )
}
