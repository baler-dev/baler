//! Integration tests for `baler box install` and `baler box status`.
//!
//! Each test runs the real `baler` binary against a tiny HTTP server that
//! plays the part of GitHub's release API, and against a fake `Rscript` that
//! only answers the one question baler asks of it (the R version and CPU).
//! Nothing leaves the machine and no R is needed. The fake `Rscript` is a
//! shell script, so these tests run on Unix only.

#![cfg(unix)]

use assert_cmd::Command;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

const UPSTREAM_DESCRIPTION: &str = "Package: box\nVersion: 1.2.3\n";
const FORK_DESCRIPTION: &str =
    "Package: box\nVersion: 1.2.3.261119\nConfig/baler/support: true\n";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER_SHA: &str = "fedcba9876543210fedcba9876543210fedcba98";

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("baler-box-{label}-{n}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A directory holding a fake `Rscript` that says "R 4.5 on x86_64". Made once
/// and shared, so no test writes an executable while another one spawns.
fn fake_r_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("baler-box-fake-r-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("Rscript");
        std::fs::write(&script, "#!/bin/sh\necho \"4.5 x86_64\"\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        dir
    })
}

fn current_os() -> &'static str {
    if cfg!(target_os = "macos") { "macos" } else { "linux" }
}

fn asset_name(r_minor: &str) -> String {
    let ext = if cfg!(target_os = "macos") { "tgz" } else { "tar.gz" };
    format!("box_1.2.3.261119_R{r_minor}_{}-x86_64.{ext}", current_os())
}

fn add_file<W: Write>(builder: &mut tar::Builder<W>, path: &str, data: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    builder.append_data(&mut header, path, data).unwrap();
}

/// A tarball shaped like a built R package: a top-level `box/` directory with
/// a DESCRIPTION and the `Meta/package.rds` that marks a built package.
fn build_archive(description: &str) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    add_file(&mut builder, "box/DESCRIPTION", description.as_bytes());
    add_file(&mut builder, "box/Meta/package.rds", b"fake");
    builder.into_inner().unwrap().finish().unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// What the fake release serves.
struct Fixture {
    assets: Vec<(String, Vec<u8>)>,
    omit_source: bool,
}

impl Fixture {
    /// A release with a valid R 4.5 build for this machine.
    fn good() -> Self {
        Self::with_build(&asset_name("4.5"), &build_archive(FORK_DESCRIPTION), None, SHA)
    }

    fn with_build(name: &str, archive: &[u8], checksum: Option<String>, sha: &str) -> Self {
        let checksum = checksum.unwrap_or_else(|| sha256_hex(archive));
        Self {
            assets: vec![
                (name.to_owned(), archive.to_vec()),
                (
                    format!("{name}.sha256"),
                    format!("{checksum}  {name}\n").into_bytes(),
                ),
                (
                    "box-source.txt".to_owned(),
                    format!("repo=joshuamarie/box\nref=feature/baler-support\nsha={sha}\n").into_bytes(),
                ),
            ],
            omit_source: false,
        }
    }
}

/// Serves `fixture` as the latest release of baler-dev/baler and returns the
/// base URL. The thread ends with the test process.
fn serve(fixture: Fixture) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());

    let mut routes: HashMap<String, Vec<u8>> = HashMap::new();
    let mut listed = Vec::new();
    for (name, body) in &fixture.assets {
        if fixture.omit_source && name == "box-source.txt" {
            continue;
        }
        listed.push(serde_json::json!({
            "name": name,
            "browser_download_url": format!("{base}/download/{name}"),
        }));
        routes.insert(format!("/download/{name}"), body.clone());
    }
    let release = serde_json::json!({ "tag_name": "v9.9.9", "assets": listed });
    routes.insert(
        "/repos/baler-dev/baler/releases/latest".to_owned(),
        release.to_string().into_bytes(),
    );

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) <= 2 {
                    break;
                }
            }
            let path = request_line.split_whitespace().nth(1).unwrap_or("/").to_owned();
            match routes.get(&path) {
                Some(body) => {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(body);
                }
                None => {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
        }
    });
    base
}

fn baler(lib: &Path, api: &str) -> Command {
    let mut cmd = Command::cargo_bin("baler").expect("baler binary should be built by `cargo test`");
    let path = format!(
        "{}:{}",
        fake_r_dir().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    cmd.env_remove("RUST_BACKTRACE")
        .env_remove("BALER_R_LIB")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .env("PATH", path)
        .env("R_LIBS_USER", lib)
        .env("BALER_RELEASE_API", api)
        .env("NO_PROXY", "127.0.0.1")
        .env("no_proxy", "127.0.0.1");
    cmd
}

fn stdout_of(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

fn stderr_of(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stderr.clone()).unwrap()
}

fn put_upstream_box(lib: &Path) {
    let dir = lib.join("box");
    std::fs::create_dir_all(dir.join("Meta")).unwrap();
    std::fs::write(dir.join("DESCRIPTION"), UPSTREAM_DESCRIPTION).unwrap();
    std::fs::write(dir.join("Meta").join("package.rds"), "upstream").unwrap();
}

#[test]
fn install_puts_the_baler_build_into_the_library() {
    let lib = Scratch::new("install");
    let api = serve(Fixture::good());

    let run = baler(lib.path(), &api).args(["box", "install"]).assert().success();
    assert!(stdout_of(&run).contains("R 4.5 on"));

    let installed = lib.path().join("box");
    let description = std::fs::read_to_string(installed.join("DESCRIPTION")).unwrap();
    assert!(description.contains("Config/baler/support: true"));
    let source = std::fs::read_to_string(installed.join("baler-source.txt")).unwrap();
    assert!(source.contains(&format!("sha={SHA}")));

    let status = baler(lib.path(), &api).args(["box", "status", "--check"]).assert().success();
    let text = stdout_of(&status);
    assert!(text.contains("1.2.3.261119"));
    assert!(text.contains("baler support level 1"));
    assert!(text.contains("joshuamarie/box@feature/baler-support"));
}

#[test]
fn a_second_install_changes_nothing_unless_forced() {
    let lib = Scratch::new("idempotent");
    let api = serve(Fixture::good());
    baler(lib.path(), &api).args(["box", "install"]).assert().success();

    let sentinel = lib.path().join("box").join("sentinel");
    std::fs::write(&sentinel, "still here").unwrap();

    let again = baler(lib.path(), &api).args(["box", "install"]).assert().success();
    assert!(stdout_of(&again).contains("already installed"));
    assert!(sentinel.exists(), "an up-to-date install must not be redone");

    baler(lib.path(), &api).args(["box", "install", "--force"]).assert().success();
    assert!(!sentinel.exists(), "--force installs again");
}

#[test]
fn a_new_commit_on_the_release_updates_the_install() {
    let lib = Scratch::new("update");
    let first = serve(Fixture::good());
    baler(lib.path(), &first).args(["box", "install"]).assert().success();

    let newer = serve(Fixture::with_build(
        &asset_name("4.5"),
        &build_archive(FORK_DESCRIPTION),
        None,
        OTHER_SHA,
    ));
    let sentinel = lib.path().join("box").join("sentinel");
    std::fs::write(&sentinel, "old build").unwrap();

    baler(lib.path(), &newer).args(["box", "install"]).assert().success();
    assert!(!sentinel.exists(), "a different commit means a new install");
    let source = std::fs::read_to_string(lib.path().join("box").join("baler-source.txt")).unwrap();
    assert!(source.contains(&format!("sha={OTHER_SHA}")));
}

#[test]
fn an_upstream_box_is_kept_unless_forced() {
    let lib = Scratch::new("upstream");
    put_upstream_box(lib.path());
    let api = serve(Fixture::good());

    let refused = baler(lib.path(), &api).args(["box", "install"]).assert().failure();
    assert!(stderr_of(&refused).contains("--force"));
    let kept = std::fs::read_to_string(lib.path().join("box").join("DESCRIPTION")).unwrap();
    assert_eq!(kept, UPSTREAM_DESCRIPTION);

    baler(lib.path(), &api).args(["box", "install", "--force"]).assert().success();
    let replaced = std::fs::read_to_string(lib.path().join("box").join("DESCRIPTION")).unwrap();
    assert!(replaced.contains("Config/baler/support"));
}

#[test]
fn a_bad_checksum_installs_nothing() {
    let lib = Scratch::new("checksum");
    let api = serve(Fixture::with_build(
        &asset_name("4.5"),
        &build_archive(FORK_DESCRIPTION),
        Some("0".repeat(64)),
        SHA,
    ));

    let run = baler(lib.path(), &api).args(["box", "install"]).assert().failure();
    assert!(stderr_of(&run).contains("Checksum mismatch"));
    assert!(!lib.path().join("box").exists());
}

#[test]
fn a_missing_checksum_file_installs_nothing() {
    let lib = Scratch::new("no-checksum");
    let mut fixture = Fixture::good();
    fixture.assets.retain(|(name, _)| !name.ends_with(".sha256"));
    let api = serve(fixture);

    let run = baler(lib.path(), &api).args(["box", "install"]).assert().failure();
    assert!(stderr_of(&run).contains("cannot be verified"));
    assert!(!lib.path().join("box").exists());
}

#[test]
fn a_release_without_a_source_file_is_refused() {
    let lib = Scratch::new("no-source");
    let mut fixture = Fixture::good();
    fixture.omit_source = true;
    let api = serve(fixture);

    let run = baler(lib.path(), &api).args(["box", "install"]).assert().failure();
    assert!(stderr_of(&run).contains("box-source.txt"));
    assert!(!lib.path().join("box").exists());
}

#[test]
fn a_missing_build_for_this_r_names_the_ones_that_exist() {
    let lib = Scratch::new("r-version");
    let api = serve(Fixture::with_build(
        &asset_name("4.6"),
        &build_archive(FORK_DESCRIPTION),
        None,
        SHA,
    ));

    let run = baler(lib.path(), &api).args(["box", "install"]).assert().failure();
    let text = stderr_of(&run);
    assert!(text.contains("no {box} build for R 4.5"));
    assert!(text.contains("R 4.6"));
    assert!(text.contains("pak::pak"));
    assert!(!lib.path().join("box").exists());
}

#[test]
fn status_says_when_box_is_not_installed() {
    let lib = Scratch::new("status-missing");
    let api = "http://127.0.0.1:9";

    let run = baler(lib.path(), api).args(["box", "status"]).assert().success();
    assert!(stdout_of(&run).contains("not installed"));
    baler(lib.path(), api).args(["box", "status", "--check"]).assert().failure();
}

#[test]
fn status_tells_an_upstream_box_from_the_baler_build() {
    let lib = Scratch::new("status-upstream");
    put_upstream_box(lib.path());
    let api = "http://127.0.0.1:9";

    let run = baler(lib.path(), api).args(["box", "status"]).assert().success();
    assert!(stdout_of(&run).contains("without baler support"));
    baler(lib.path(), api).args(["box", "status", "--check"]).assert().failure();
}

#[test]
fn a_tag_that_could_change_the_url_is_refused_before_any_request() {
    let lib = Scratch::new("bad-tag");
    let api = "http://127.0.0.1:9";

    let run = baler(lib.path(), api)
        .args(["box", "install", "--tag", "v1/../x"])
        .assert()
        .failure();
    assert!(stderr_of(&run).contains("is not a release tag"));
}
