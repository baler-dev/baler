//! `baler box`: installs and inspects the patched {box} that baler modules
//! are loaded with.
//!
//! The pre-built {box} archives ship on the same GitHub release as `baler`
//! (built by `build-box.yml`, kept fresh by `refresh-box.yml`). This command
//! picks the one that matches the R and the platform on this machine, checks
//! it against its published SHA-256, and puts it into the R library. It reuses
//! `baler-core`'s binary installer, so R itself is only asked for its version
//! and its library path, and `R CMD INSTALL` is never run.

mod asset;
mod description;
mod release;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use baler_core::cran::binary_install::install_binary_package;
use baler_core::paths::{detect_r_platform, resolve_r_lib_dir};
use tempfile::TempDir;

use asset::Target;
use description::{BoxState, Source};

const SOURCE_BUILD_HINT: &str =
    "Rscript -e 'pak::pak(\"joshuamarie/box@feature/baler-support\")'";

pub struct BoxInstallArgs {
    pub tag: Option<String>,
    pub force: bool,
}

pub struct BoxStatusArgs {
    pub check: bool,
}

/// What `acquire` found: either {box} is already current, or a verified
/// archive is waiting in a temp directory that lives as long as this value.
enum Acquired {
    UpToDate(Source),
    Ready(Ready),
}

struct Ready {
    release_tag: String,
    asset_name: String,
    source_text: String,
    archive: PathBuf,
    _dir: TempDir,
}

pub fn install(args: BoxInstallArgs) -> Result<()> {
    if let Some(tag) = &args.tag {
        release::check_tag(tag)?;
    }

    let platform = detect_r_platform()?;
    let target = Target::from_platform(&platform)?;
    let lib = resolve_r_lib_dir()?;
    println!("{}, library {}", target.label(), lib.display());

    let state = description::inspect(&lib);
    check_replaceable(&state, &lib, args.force)?;
    let installed_sha = match &state {
        BoxState::Installed(installed) => installed.source.as_ref().map(|s| s.sha.clone()),
        _ => None,
    };

    let tag = args.tag.as_deref();
    let sha = installed_sha.as_deref();
    let acquired = match acquire(&target, tag, sha, args.force) {
        Err(e) if e.downcast_ref::<release::Retryable>().is_some() => {
            println!("{e}. Trying once more...");
            acquire(&target, tag, sha, args.force)?
        }
        other => other?,
    };

    let ready = match acquired {
        Acquired::UpToDate(source) => {
            println!(
                "{{box}} from {}@{} is already installed. Use --force to install it again.",
                source.repo,
                source.short_sha()
            );
            return Ok(());
        }
        Acquired::Ready(ready) => ready,
    };

    println!("Installing {} from release {}...", ready.asset_name, ready.release_tag);
    install_binary_package(&ready.archive, &lib, "box", &target.arch)
        .with_context(|| install_failure_note(&lib))?;

    let marker = lib.join("box").join(description::SOURCE_FILE);
    if let Err(e) = std::fs::write(&marker, &ready.source_text) {
        eprintln!(
            "warning: could not write {} ({e}). {{box}} works, but `baler box install` will \
             not know which commit it came from.",
            marker.display()
        );
    }

    match description::inspect(&lib) {
        BoxState::Installed(installed) if installed.support.is_some() => {
            println!(
                "Installed {{box}} {} with baler support. Check it with `baler box status`.",
                installed.version.as_deref().unwrap_or("(unknown version)")
            );
        }
        _ => eprintln!(
            "warning: the installed {{box}} does not declare `{}` in its DESCRIPTION, so it may \
             not load baler modules. Was the release built from the baler fork?",
            description::SUPPORT_FIELD
        ),
    }
    Ok(())
}

pub fn status(args: BoxStatusArgs) -> Result<()> {
    let lib = resolve_r_lib_dir()?;
    println!("Library: {}", lib.display());

    let supported = match description::inspect(&lib) {
        BoxState::Missing => {
            println!("{{box}}: not installed. Run `baler box install`.");
            false
        }
        BoxState::Damaged(dir) => {
            println!(
                "{{box}}: {} exists but is not a readable R package. Run `baler box install \
                 --force` to replace it.",
                dir.display()
            );
            false
        }
        BoxState::Installed(installed) => {
            let version = installed.version.as_deref().unwrap_or("unknown");
            match installed.support {
                Some(level) => println!("{{box}}: {version}, baler support level {level}"),
                None => println!(
                    "{{box}}: {version}, without baler support. Run `baler box install` to get \
                     the patched build."
                ),
            }
            match &installed.source {
                Some(source) => println!(
                    "Built from: {}@{} ({})",
                    source.repo,
                    source.git_ref,
                    source.short_sha()
                ),
                None => println!("Built from: unknown, it was not installed by `baler box install`"),
            }
            installed.support.is_some()
        }
    };

    if args.check && !supported {
        bail!("No {{box}} with baler support is installed in {}", lib.display());
    }
    Ok(())
}

/// Replacing {box} deletes `<lib>/box` first, so an upstream {box} (or a
/// directory that is not a package at all) is only replaced when asked for.
fn check_replaceable(state: &BoxState, lib: &Path, force: bool) -> Result<()> {
    if force {
        return Ok(());
    }
    match state {
        BoxState::Installed(installed) if !installed.is_baler_build() => bail!(
            "{{box}} {} is already installed in {} and it is not the baler build (its \
             DESCRIPTION has no `{}`). Installing would replace it. Run again with --force to \
             replace it, or set BALER_R_LIB to another library.",
            installed.version.as_deref().unwrap_or("(unknown version)"),
            lib.display(),
            description::SUPPORT_FIELD
        ),
        BoxState::Damaged(dir) => bail!(
            "{} exists but is not a readable R package. Run again with --force to replace it, \
             or set BALER_R_LIB to another library.",
            dir.display()
        ),
        _ => Ok(()),
    }
}

fn install_failure_note(lib: &Path) -> String {
    let base = format!("Failed to install {{box}} into {}", lib.display());
    if cfg!(windows) {
        format!("{base}. If an R session has {{box}} loaded, close it and try again.")
    } else {
        base
    }
}

/// Everything up to a verified archive on disk: the release, the matching
/// asset, its source file and its checksum. Nothing in the library is touched.
fn acquire(
    target: &Target,
    tag: Option<&str>,
    installed_sha: Option<&str>,
    force: bool,
) -> Result<Acquired> {
    let found = release::fetch_release(tag)?;
    let names = found.asset_names();

    let Some(asset_name) = asset::pick_asset(&names, target) else {
        bail!("{}", no_build_message(&found.tag_name, &names, target));
    };
    let ext = asset::archive_extension(asset_name)
        .context("pick_asset only returns names that have an archive extension")?;

    let archive_asset = found
        .find(asset_name)
        .with_context(|| format!("{asset_name} is not in release {}", found.tag_name))?;
    let checksum_name = format!("{asset_name}.sha256");
    let checksum_asset = found.find(&checksum_name).with_context(|| {
        format!(
            "Release {} has no {checksum_name}, so the download cannot be verified. \
             Nothing was installed.",
            found.tag_name
        )
    })?;
    let source_asset = found.find("box-source.txt").with_context(|| {
        format!(
            "Release {} has no box-source.txt, so it was published without {{box}}.",
            found.tag_name
        )
    })?;

    let source_text = release::download_text(&source_asset.browser_download_url)?;
    let source = description::parse_source(&source_text)?;
    if !force && installed_sha == Some(source.sha.as_str()) {
        return Ok(Acquired::UpToDate(source));
    }

    let dir = TempDir::new().context("Failed to create a temp directory")?;
    // R reads the archive type from the file's ending, so the ending stays.
    let archive = dir.path().join(format!("box.{ext}"));
    release::download_file(&archive_asset.browser_download_url, &archive)?;
    let checksum_text = release::download_text(&checksum_asset.browser_download_url)?;
    let expected = release::parse_checksum(&checksum_text)?;
    release::verify_sha256(&archive, &expected, asset_name)?;

    Ok(Acquired::Ready(Ready {
        release_tag: found.tag_name,
        asset_name: asset_name.to_owned(),
        source_text,
        archive,
        _dir: dir,
    }))
}

fn no_build_message(release_tag: &str, names: &[String], target: &Target) -> String {
    let detail = if !asset::has_any_build(names) {
        format!("Release {release_tag} was published without {{box}} (its build failed).")
    } else {
        let minors = asset::available_r_minors(names, target.os, &target.arch);
        if minors.is_empty() {
            format!(
                "Release {release_tag} has no {{box}} build for {}-{}.",
                target.os, target.arch
            )
        } else {
            let list = minors
                .iter()
                .map(|minor| format!("R {minor}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "Release {release_tag} has no {{box}} build for R {} on {}-{}. It has builds \
                 for {list}.",
                target.r_minor, target.os, target.arch
            )
        }
    };
    format!("{detail}\nBuild it from source instead (this needs a compiler):\n    {SOURCE_BUILD_HINT}")
}

#[cfg(test)]
mod tests {
    use super::description::InstalledBox;
    use super::*;

    fn lib() -> PathBuf {
        PathBuf::from("/some/lib")
    }

    fn installed(support: Option<u32>) -> BoxState {
        BoxState::Installed(InstalledBox {
            version: Some("1.2.3".to_owned()),
            support,
            source: None,
        })
    }

    #[test]
    fn an_upstream_box_is_not_replaced_unasked() {
        let error = check_replaceable(&installed(None), &lib(), false).unwrap_err();
        assert!(error.to_string().contains("--force"));
        assert!(check_replaceable(&installed(None), &lib(), true).is_ok());
    }

    #[test]
    fn the_baler_build_is_replaced_without_asking() {
        assert!(check_replaceable(&installed(Some(1)), &lib(), false).is_ok());
    }

    #[test]
    fn a_missing_box_needs_no_force() {
        assert!(check_replaceable(&BoxState::Missing, &lib(), false).is_ok());
    }

    #[test]
    fn a_damaged_directory_is_not_deleted_unasked() {
        let state = BoxState::Damaged(PathBuf::from("/some/lib/box"));
        assert!(check_replaceable(&state, &lib(), false).is_err());
        assert!(check_replaceable(&state, &lib(), true).is_ok());
    }

    #[test]
    fn the_message_names_the_r_versions_that_exist() {
        let names = vec![
            "box-source.txt".to_owned(),
            "box_1.2.3_R4.6_linux-x86_64.tar.gz".to_owned(),
        ];
        let target = Target::new("linux", "x86_64", "4.5");
        let message = no_build_message("v0.2.0", &names, &target);
        assert!(message.contains("R 4.6"));
        assert!(message.contains("pak::pak"));
    }

    #[test]
    fn the_message_knows_a_release_without_box() {
        let names = vec!["baler-installer.sh".to_owned()];
        let target = Target::new("linux", "x86_64", "4.5");
        let message = no_build_message("v0.1.0", &names, &target);
        assert!(message.contains("published without {box}"));
    }
}
