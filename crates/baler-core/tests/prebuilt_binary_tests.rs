use baler_core::baler_toml::{
    first_target, sniff_binary, target_of_platform, BinaryArch, BinaryOs, BinaryTarget,
};
use baler_core::formats::tar;
use std::path::{Path, PathBuf};

fn elf(machine: u16) -> Vec<u8> {
    let mut head = vec![0u8; 64];
    head[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    head[4] = 2;
    head[5] = 1;
    head[18..20].copy_from_slice(&machine.to_le_bytes());
    head
}

fn macho(cpu: u32) -> Vec<u8> {
    let mut head = vec![0u8; 64];
    head[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
    head[4..8].copy_from_slice(&cpu.to_le_bytes());
    head
}

fn pe(machine: u16) -> Vec<u8> {
    let mut head = vec![0u8; 0x100];
    head[..2].copy_from_slice(b"MZ");
    head[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    head[0x80..0x84].copy_from_slice(b"PE\0\0");
    head[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
    head
}

#[test]
fn platforms_are_read_from_r_platform_strings() {
    let mac = target_of_platform("aarch64-apple-darwin20").unwrap();
    assert_eq!(mac.describe(), "macos aarch64");
    assert_eq!(target_of_platform("arm64-apple-darwin23.4.0").unwrap(), mac);
    assert_eq!(target_of_platform("x86_64-w64-mingw32").unwrap().describe(), "windows x86_64");
    assert_eq!(
        target_of_platform("X86_64-pc-linux-gnu").unwrap().dir_name().unwrap(),
        "linux-x86_64"
    );
    assert!(target_of_platform("riscv64-unknown-linux-gnu").is_none());
    assert!(!mac.fits(&target_of_platform("x86_64-apple-darwin20").unwrap()));
}

#[test]
fn headers_are_read() {
    let target = sniff_binary(&elf(0x3E)).unwrap();
    assert_eq!((target.os, target.archs.clone()), (BinaryOs::Linux, vec![BinaryArch::X86_64]));
    assert_eq!(sniff_binary(&elf(0xB7)).unwrap().archs, vec![BinaryArch::Aarch64]);
    assert!(sniff_binary(&elf(0xF3)).unwrap().archs.is_empty());
    assert_eq!(sniff_binary(&macho(0x0100_000C)).unwrap().os, BinaryOs::MacOs);
    assert_eq!(sniff_binary(&macho(0x0100_0007)).unwrap().archs, vec![BinaryArch::X86_64]);
    assert_eq!(sniff_binary(&pe(0x8664)).unwrap().os, BinaryOs::Windows);
    assert_eq!(sniff_binary(&pe(0xAA64)).unwrap().archs, vec![BinaryArch::Aarch64]);
    assert!(sniff_binary(b"just some text, not a binary").is_none());
    assert!(sniff_binary(&[]).is_none());

    let mut fat = vec![0u8; 64];
    fat[..4].copy_from_slice(&[0xca, 0xfe, 0xba, 0xbe]);
    fat[4..8].copy_from_slice(&2u32.to_be_bytes());
    fat[8..12].copy_from_slice(&0x0100_0007u32.to_be_bytes());
    fat[28..32].copy_from_slice(&0x0100_000Cu32.to_be_bytes());
    let universal = sniff_binary(&fat).unwrap();
    assert_eq!(universal.archs, vec![BinaryArch::X86_64, BinaryArch::Aarch64]);
    assert_eq!(universal.dir_name().unwrap(), "macos-x86_64-aarch64");
    assert!(universal.fits(&target_of_platform("aarch64-apple-darwin20").unwrap()));
    assert!(universal.fits(&target_of_platform("x86_64-apple-darwin20").unwrap()));

    let mut java = vec![0u8; 64];
    java[..4].copy_from_slice(&[0xca, 0xfe, 0xba, 0xbe]);
    java[4..8].copy_from_slice(&0x0000_0034u32.to_be_bytes());
    assert!(sniff_binary(&java).is_none());
}

#[test]
fn first_known_file_decides() {
    let heads = vec![b"data".to_vec(), elf(0x3E)];
    assert_eq!(first_target(&heads).unwrap().describe(), "linux x86_64");
    assert!(first_target(&[b"data".to_vec()]).is_none());
    assert!(first_target(&[]).is_none());
}

/// Bundles a module whose `.lib/cpp.so` holds exactly `binary`, the way
/// `baler bundle` does with `binary = true`, but without compiling anything.
fn bundle_with(work: &Path, label: &str, source: &str, binary: &[u8]) -> PathBuf {
    let root = work.join(label);
    std::fs::create_dir_all(root.join("mod/.lib")).unwrap();
    std::fs::write(root.join("mod/__init__.R"), source).unwrap();
    std::fs::write(root.join("mod/.lib/cpp.so"), binary).unwrap();
    std::fs::write(root.join("baler.toml"), "[project]\nname = \"mod\"\nversion = \"0.1.0\"\n").unwrap();
    let archive = work.join(format!("{label}.tar.gz"));
    tar::bundle(
        &root.join("mod"),
        &root,
        &archive,
        "mod",
        "0.1.0",
        &[],
        &[root.join("mod/.lib")],
    )
    .unwrap();
    archive
}

fn built_for(archive: &Path) -> Vec<String> {
    tar::read_shipped_libs(archive).unwrap()[0]
        .targets
        .iter()
        .map(BinaryTarget::describe)
        .collect()
}

fn names(archive: &Path) -> Vec<String> {
    let file = std::fs::File::open(archive).unwrap();
    let mut archive = ::tar::Archive::new(flate2::read::GzDecoder::new(file));
    archive
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn bundles_from_other_platforms_are_carried_over() {
    let work = tempfile::tempdir().unwrap();
    let linux = bundle_with(work.path(), "linux", "x = 1\n", &elf(0x3E));
    let mac = bundle_with(work.path(), "mac", "x = 1\n", &macho(0x0100_000C));
    assert_eq!(built_for(&linux), vec!["linux x86_64"]);

    // the fresh bundle keeps `.lib/`, the earlier platform moves under `.platforms/`
    let both = work.path().join("both.tar.gz");
    assert!(tar::add_platforms(&mac, &linux, &both).unwrap());
    assert_eq!(built_for(&both), vec!["macos aarch64", "linux x86_64"]);
    let listed = names(&both);
    assert!(listed.contains(&"mod_0.1.0/mod/.lib/cpp.so".to_string()), "{listed:?}");
    assert!(listed.contains(&"mod_0.1.0/mod/.lib/.platforms/linux-x86_64/cpp.so".to_string()), "{listed:?}");

    // bundling again on a platform already inside replaces its binary instead of duplicating it
    let mut newer_linux = elf(0x3E);
    newer_linux.extend_from_slice(b"rebuilt");
    let linux_again = bundle_with(work.path(), "linux_again", "x = 1\n", &newer_linux);
    let again = work.path().join("again.tar.gz");
    assert!(tar::add_platforms(&linux_again, &both, &again).unwrap());
    assert_eq!(built_for(&again), vec!["linux x86_64", "macos aarch64"]);
    let listed = names(&again);
    assert_eq!(listed.iter().filter(|name| name.ends_with("cpp.so")).count(), 2, "{listed:?}");

    // a third platform joins the two
    let windows = bundle_with(work.path(), "windows", "x = 1\n", &pe(0x8664));
    let all = work.path().join("all.tar.gz");
    assert!(tar::add_platforms(&windows, &again, &all).unwrap());
    assert_eq!(built_for(&all), vec!["windows x86_64", "linux x86_64", "macos aarch64"]);
}

#[test]
fn an_earlier_bundle_of_other_source_is_not_used() {
    let work = tempfile::tempdir().unwrap();
    let linux = bundle_with(work.path(), "linux", "x = 1\n", &elf(0x3E));
    let changed = bundle_with(work.path(), "changed", "x = 2\n", &macho(0x0100_000C));
    let output = work.path().join("out.tar.gz");
    assert!(!tar::add_platforms(&changed, &linux, &output).unwrap());
    assert!(!output.exists(), "nothing is written when the source differs");
}

#[test]
fn an_unrecognisable_earlier_binary_is_refused() {
    let work = tempfile::tempdir().unwrap();
    let junk = bundle_with(work.path(), "junk", "x = 1\n", b"not a binary");
    let mac = bundle_with(work.path(), "mac", "x = 1\n", &macho(0x0100_000C));
    let message = tar::add_platforms(&mac, &junk, &work.path().join("out.tar.gz"))
        .unwrap_err()
        .to_string();
    assert!(message.contains("recognisable"), "{message}");
}

#[test]
fn the_output_can_not_be_an_input() {
    let work = tempfile::tempdir().unwrap();
    let linux = bundle_with(work.path(), "linux", "x = 1\n", &elf(0x3E));
    let mac = bundle_with(work.path(), "mac", "x = 1\n", &macho(0x0100_000C));
    assert!(tar::add_platforms(&mac, &linux, &linux).is_err());
}

#[test]
fn line_endings_do_not_make_the_source_differ() {
    let work = tempfile::tempdir().unwrap();
    let linux = bundle_with(work.path(), "linux", "x = 1\ny = 2\n", &elf(0x3E));
    let windows = bundle_with(work.path(), "windows", "x = 1\r\ny = 2\r\n", &pe(0x8664));
    let both = work.path().join("both.tar.gz");
    assert!(tar::add_platforms(&windows, &linux, &both).unwrap());
    assert_eq!(built_for(&both), vec!["windows x86_64", "linux x86_64"]);

    let changed = bundle_with(work.path(), "changed", "x = 1\r\ny = 3\r\n", &macho(0x0100_000C));
    assert!(!tar::add_platforms(&changed, &linux, &work.path().join("out.tar.gz")).unwrap());
}
