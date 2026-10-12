use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::time::{Duration, Instant};

use pep440_rs::{Version as PepVersion, VersionSpecifiers};

use crate::baler_toml::{ExternalToolDep, Extras};

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// A cold interpreter start can be slow, so it gets more room than a
/// `--version` probe.
const PYTHON_TIMEOUT: Duration = Duration::from_secs(15);

/// Prints `{"name": "1.2.3" | null, ...}` for every name in argv.
/// `importlib.metadata` reads package metadata without importing the
/// package, so no third-party code runs during the check.
const PY_SCRIPT: &str = "\
import json, sys
from importlib.metadata import version, PackageNotFoundError
out = {}
for n in sys.argv[1:]:
    try:
        out[n] = version(n)
    except PackageNotFoundError:
        out[n] = None
print(json.dumps(out))
";

/// Checks each `[extras.external]` entry. Prints a note to stderr for
/// anything off, never fails and never blocks the caller. An external
/// tool is a development nicety, not something the module needs in
/// order to run.
///
/// - `cli` entries: is the tool on PATH, and does the version printed
///   by `<tool> --version` satisfy the declared semver requirement.
///   The version is read from the first dotted number in the combined
///   stdout and stderr, padded to three components (`3.1` is
///   `3.1.0`). A tool that prints its version some other way is
///   reported as unreadable, not as a mismatch.
/// - `python` entries: one interpreter is resolved per run (see
///   `resolve_interpreter`), asked once for every package's installed
///   version, and the results are compared against PEP 440
///   specifiers. Messages always name the interpreter that was
///   checked, because "installed" means nothing without one.
///
/// Only pass the root project's own `extras`. This runs external
/// programs, so a fetched module's `[extras.external]` must never
/// reach it.
pub fn check_external_tools(extras: Option<&Extras>) {
    let Some(tools) = extras.and_then(|e| e.external.as_ref()) else {
        return;
    };

    let mut python_entries: Vec<PyEntry> = Vec::new();

    for (name, dep) in tools {
        match dep {
            ExternalToolDep::Cli { version, hint } => {
                report_cli(name, version, hint.as_deref());
            }
            ExternalToolDep::Python { version, python, index, git } => {
                python_entries.push(PyEntry {
                    name,
                    version,
                    python: python.as_deref(),
                    index: index.as_deref(),
                    git: git.as_deref(),
                });
            }
        }
    }

    if !python_entries.is_empty() {
        check_python(python_entries);
    }
}

// ---- cli ----

enum CliStatus {
    Ok,
    Missing,
    BadConstraint(String),
    Mismatch { found: semver::Version, path: PathBuf },
    Unreadable(PathBuf),
}

fn report_cli(name: &str, declared: &str, hint: Option<&str>) {
    match check_cli(name, declared) {
        CliStatus::Ok => {}
        CliStatus::Missing => {
            let see = hint.map(|h| format!(" See {h}.")).unwrap_or_default();
            eprintln!(
                "  [extras.external] '{name}' ({declared}) not found on PATH. \
                 Not required to run this module, but declared as a \
                 recommended tool for working on it.{see}"
            );
        }
        CliStatus::BadConstraint(reason) => eprintln!(
            "  [extras.external] '{name}' has an invalid version \
             constraint '{declared}': {reason}"
        ),
        CliStatus::Mismatch { found, path } => eprintln!(
            "  [extras.external] '{name}' is {found} at {}, but baler.toml \
             declares {declared}.",
            path.display()
        ),
        CliStatus::Unreadable(path) => eprintln!(
            "  [extras.external] '{name}' is at {}, but its version \
             could not be read from `{name} --version`, so {declared} \
             was not checked.",
            path.display()
        ),
    }
}

fn check_cli(name: &str, declared: &str) -> CliStatus {
    let Some(path) = find_on_path(name) else {
        return CliStatus::Missing;
    };
    let req = match semver::VersionReq::parse(declared) {
        Ok(req) => req,
        Err(e) => return CliStatus::BadConstraint(e.to_string()),
    };
    let Some(output) = run_capture(&path, &["--version"], PROBE_TIMEOUT, true) else {
        return CliStatus::Unreadable(path);
    };
    let Some(found) = parse_version(&output) else {
        return CliStatus::Unreadable(path);
    };
    if req.matches(&found) {
        CliStatus::Ok
    } else {
        CliStatus::Mismatch { found, path }
    }
}

// ---- python ----

struct PyEntry<'a> {
    name: &'a str,
    version: &'a str,
    python: Option<&'a str>,
    index: Option<&'a str>,
    git: Option<&'a str>,
}

fn check_python(entries: Vec<PyEntry>) {
    // Almost always one group (no per-entry override), so one
    // interpreter start. An override just splits off its own group.
    let mut groups: BTreeMap<Option<&str>, Vec<PyEntry>> = BTreeMap::new();
    for e in entries {
        groups.entry(e.python).or_default().push(e);
    }

    for (explicit, group) in groups {
        let interpreter = match resolve_interpreter(explicit) {
            Ok(p) => p,
            Err(msg) => {
                eprintln!(
                    "  [extras.external] {msg} Skipped {} python entr{}.",
                    group.len(),
                    if group.len() == 1 { "y" } else { "ies" }
                );
                continue;
            }
        };

        let names: Vec<&str> = group.iter().map(|e| e.name).collect();
        let Some(installed) = query_python(&interpreter, &names) else {
            eprintln!(
                "  [extras.external] could not query package versions from {}. \
                 Skipped {} python entr{}.",
                interpreter.display(),
                group.len(),
                if group.len() == 1 { "y" } else { "ies" }
            );
            continue;
        };

        for e in &group {
            let found = installed.get(e.name).and_then(|v| v.as_deref());
            report_python(e, &interpreter, found);
        }
    }
}

fn report_python(e: &PyEntry, interpreter: &Path, installed: Option<&str>) {
    let (name, declared) = (e.name, e.version);
    let py = interpreter.display();

    let specs = match VersionSpecifiers::from_str(declared) {
        Ok(s) => s,
        Err(err) => {
            eprintln!(
                "  [extras.external] '{name}' has an invalid PEP 440 \
                 specifier '{declared}': {err}"
            );
            return;
        }
    };
    let hint = install_hint(interpreter, e);

    let Some(installed) = installed else {
        eprintln!(
            "  [extras.external] '{name}' ({declared}) is not installed in {py}. \
             Not required to run this module. Install with: {hint}"
        );
        return;
    };

    match PepVersion::from_str(installed) {
        Ok(found) if specs.contains(&found) => {}
        Ok(_) => eprintln!(
            "  [extras.external] '{name}' is {installed} in {py}, but baler.toml \
             declares {declared}. Update with: {hint}"
        ),
        Err(_) => eprintln!(
            "  [extras.external] '{name}' in {py} reports version '{installed}', \
             which is not valid PEP 440, so {declared} was not checked."
        ),
    }
}

/// The hint targets the exact interpreter that was checked
/// (`<python> -m pip`), so following it can't install into some other
/// environment.
fn install_hint(interpreter: &Path, e: &PyEntry) -> String {
    let py = interpreter.display();
    if let Some(git) = e.git {
        let url = if git.starts_with("git+") {
            git.to_owned()
        } else {
            format!("git+{git}")
        };
        return format!("{py} -m pip install \"{url}\"");
    }
    let index = e
        .index
        .map(|i| format!(" --index-url \"{i}\""))
        .unwrap_or_default();
    format!("{py} -m pip install{index} \"{}{}\"", e.name, e.version)
}

/// Picks the one interpreter to check against, first match wins:
/// 1. the entry's own `python = "..."`
/// 2. `RETICULATE_PYTHON`, since that is what R will use
/// 3. an active `VIRTUAL_ENV` or `CONDA_PREFIX`
/// 4. `python3`, then `python` on PATH
///
/// An explicit setting (1 or 2) that doesn't resolve is an error, not
/// a reason to quietly fall through to a different interpreter.
fn resolve_interpreter(explicit: Option<&str>) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        return resolve_candidate(p)
            .ok_or_else(|| format!("python interpreter '{p}' (from baler.toml) was not found."));
    }
    if let Some(p) = env_nonempty("RETICULATE_PYTHON") {
        return resolve_candidate(&p)
            .ok_or_else(|| format!("RETICULATE_PYTHON points at '{p}', which was not found."));
    }
    for var in ["VIRTUAL_ENV", "CONDA_PREFIX"] {
        if let Some(prefix) = env_nonempty(var) {
            let prefix = PathBuf::from(prefix);
            if let Some(found) = env_python_candidates(&prefix)
                .into_iter()
                .find(|c| is_runnable(c))
            {
                return Ok(found);
            }
        }
    }
    ["python3", "python"]
        .iter()
        .find_map(|n| find_on_path(n))
        .ok_or_else(|| {
            "no Python interpreter found (checked RETICULATE_PYTHON, VIRTUAL_ENV, \
             CONDA_PREFIX, then python3 and python on PATH)."
                .to_owned()
        })
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string_lossy().into_owned())
}

fn resolve_candidate(s: &str) -> Option<PathBuf> {
    if s.contains(['/', '\\']) {
        let p = PathBuf::from(s);
        is_runnable(&p).then_some(p)
    } else {
        find_on_path(s)
    }
}

fn env_python_candidates(prefix: &Path) -> Vec<PathBuf> {
    if cfg!(windows) {
        vec![
            prefix.join("Scripts").join("python.exe"),
            prefix.join("python.exe"),
        ]
    } else {
        vec![prefix.join("bin").join("python3"), prefix.join("bin").join("python")]
    }
}

/// Asks the interpreter for every package's installed version in one
/// start. Only stdout is read, so a warning on stderr can't corrupt
/// the JSON.
fn query_python(interpreter: &Path, names: &[&str]) -> Option<BTreeMap<String, Option<String>>> {
    let mut args: Vec<&str> = vec!["-c", PY_SCRIPT];
    args.extend_from_slice(names);
    let text = run_capture(interpreter, &args, PYTHON_TIMEOUT, false)?;
    let line = text.lines().rev().find(|l| l.trim_start().starts_with('{'))?;
    serde_json::from_str(line).ok()
}

// ---- shared ----

fn run_capture(
    path: &Path,
    args: &[&str],
    timeout: Duration,
    include_stderr: bool,
) -> Option<String> {
    let mut child = Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let start = Instant::now();
    loop {
        match child.try_wait().ok()? {
            Some(_) => break,
            None if start.elapsed() > timeout => {
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
    if include_stderr {
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut text);
        }
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
