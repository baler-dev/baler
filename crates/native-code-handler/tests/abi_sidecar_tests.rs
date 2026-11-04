use std::path::{Path, PathBuf};

use baler_native::toolchain::{read_abi_sidecars, sidecar_path, AbiInfo};

fn temp_lib(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("baler-abi-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Same layout `build` writes next to an artifact.
fn write_sidecar(artifact: &Path, target_triple: &str, r_version: &str, source_hash: &str) {
    std::fs::write(
        sidecar_path(artifact),
        format!(
            "{{\n  \"target_triple\": {:?},\n  \"r_version\": {:?},\n  \"source_hash\": {:?}\n}}\n",
            target_triple, r_version, source_hash
        ),
    )
    .unwrap();
}

fn info(target_triple: &str, r_version: &str) -> AbiInfo {
    AbiInfo {
        target_triple: target_triple.to_owned(),
        r_version: r_version.to_owned(),
        source_hash: "abc123".to_owned(),
    }
}

#[test]
fn sidecar_is_read_back() {
    let lib = temp_lib("read-back");
    let artifact = lib.join("foo.so");
    std::fs::write(&artifact, b"binary").unwrap();
    write_sidecar(&artifact, "x86_64-pc-linux-gnu", "4.4", "abc123");

    let found = read_abi_sidecars(&lib).unwrap();
    assert_eq!(found, vec![(artifact, info("x86_64-pc-linux-gnu", "4.4"))]);
    std::fs::remove_dir_all(&lib).unwrap();
}

#[test]
fn sidecar_without_its_artifact_is_ignored() {
    let lib = temp_lib("orphan");
    write_sidecar(&lib.join("gone.so"), "a", "4.4", "h");
    assert!(read_abi_sidecars(&lib).unwrap().is_empty());
    std::fs::remove_dir_all(&lib).unwrap();
}

#[test]
fn artifact_without_a_sidecar_is_ignored() {
    let lib = temp_lib("no-sidecar");
    std::fs::write(lib.join("foo.so"), b"binary").unwrap();
    assert!(read_abi_sidecars(&lib).unwrap().is_empty());
    std::fs::remove_dir_all(&lib).unwrap();
}

#[test]
fn missing_lib_dir_gives_an_empty_list() {
    let lib = std::env::temp_dir().join(format!("baler-abi-{}-absent", std::process::id()));
    assert!(read_abi_sidecars(&lib).unwrap().is_empty());
}

#[test]
fn incomplete_sidecar_is_an_error_naming_the_file() {
    let err = AbiInfo::parse("{\n  \"target_triple\": \"a\"\n}\n", "x.abi.json").unwrap_err();
    assert!(err.to_string().contains("x.abi.json"));
}

#[test]
fn compatibility_needs_same_triple_and_r_version() {
    let built = info("aarch64-apple-darwin20", "4.4");
    assert!(built.is_compatible("aarch64-apple-darwin20", "4.4"));
    assert!(!built.is_compatible("aarch64-apple-darwin20", "4.5"));
    assert!(!built.is_compatible("x86_64-apple-darwin20", "4.4"));
}
