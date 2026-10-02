use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::baler_toml::Extras;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

enum Status {
    Ok,
    Missing,
    BadConstraint(String),
    Mismatch(semver::Version),
    Unreadable,
}

/// Checks each `[extras.external]` entry: is the tool on PATH, and
/// does the version printed by `<tool> --version` satisfy the declared
/// constraint. Prints a note to stderr for anything off, never fails
/// and never blocks the caller. An external tool (a linter, Quarto) is
/// a development nicety, not something the module needs in order to
/// run.
///
/// Only pass the root project's own `extras`. This runs the tools, so
/// a fetched module's `[extras.external]` must never reach it.
///
/// The version is read from the first dotted number in the combined
/// stdout and stderr, padded to three components (`3.1` is `3.1.0`).
/// A tool that prints its version some other way is reported as
/// unreadable, not as a mismatch.
pub fn check_external_tools(extras: Option<&Extras>) {
    let Some(tools) = extras.and_then(|e| e.external.as_ref()) else {
        return;
    };

    for (name, dep) in tools {
        let declared = dep.version();
        match check_tool(name, declared) {
            Status::Ok => {}
            Status::Missing => eprintln!(
                "  [extras.external] '{name}' ({declared}) not found on PATH. \
                 Not required to run this module, but declared as a \
                 recommended tool for working on it."
            ),
            Status::BadConstraint(reason) => eprintln!(
                "  [extras.external] '{name}' has an invalid version \
                 constraint '{declared}': {reason}"
            ),
            Status::Mismatch(found) => eprintln!(
                "  [extras.external] '{name}' is {found}, but baler.toml \
                 declares {declared}."
            ),
            Status::Unreadable => eprintln!(
                "  [extras.external] '{name}' is on PATH, but its version \
                 could not be read from `{name} --version`, so {declared} \
                 was not checked."
            ),
        }
    }
}

fn check_tool(name: &str, declared: &str) -> Status {
    let Some(path) = find_on_path(name) else {
        return Status::Missing;
    };
    let req = match semver::VersionReq::parse(declared) {
        Ok(req) => req,
        Err(e) => return Status::BadConstraint(e.to_string()),
    };
    let Some(output) = run_version(&path) else {
        return Status::Unreadable;
    };
    let Some(found) = parse_version(&output) else {
        return Status::Unreadable;
    };
    if req.matches(&found) {
        Status::Ok
    } else {
        Status::Mismatch(found)
    }
}

fn run_version(path: &Path) -> Option<String> {
    let mut child = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let start = Instant::now();
    loop {
        match child.try_wait().ok()? {
            Some(_) => break,
            None if start.elapsed() > PROBE_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }

    let mut text = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut text);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut text);
    }
    Some(text)
}

/// Reads the first dotted number out of a tool's `--version` output,
/// padded to three components (`3.1` is `3.1.0`). `None` when there is
/// no dotted number at all.
pub fn parse_version(text: &str) -> Option<semver::Version> {
    let token = text
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .find(|t| t.starts_with(|c: char| c.is_ascii_digit()) && t.contains('.'))?;
    let mut parts = token.split('.').map_while(|p| p.parse::<u64>().ok());
    let major = parts.next()?;
    let minor = parts.next().unwrap_or(0);
    let patch = parts.next().unwrap_or(0);
    Some(semver::Version::new(major, minor, patch))
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let suffixes = suffixes();
    std::env::split_paths(&path_var).find_map(|dir| {
        suffixes
            .iter()
            .map(|suffix| dir.join(format!("{name}{suffix}")))
            .find(|candidate| is_runnable(candidate))
    })
}

fn suffixes() -> Vec<String> {
    if cfg!(windows) {
        let raw = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
        raw.split(';')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .chain(std::iter::once(String::new()))
            .collect()
    } else {
        vec![String::new()]
    }
}

#[cfg(unix)]
fn is_runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_runnable(path: &Path) -> bool {
    path.is_file()
}
