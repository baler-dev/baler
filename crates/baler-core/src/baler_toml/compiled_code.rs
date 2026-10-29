use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::dependencies::PackageDep;

// ---- NativePath ----

/// `[compiled-code].path` accepts either a single string or an array,
/// so a module with one compiled-code dir doesn't have to write
/// `path = ["cpp/"]` just to satisfy a Vec-only field, and a module
/// with several doesn't have to pick one arbitrarily.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum NativePath {
    Single(String),
    Multiple(Vec<String>),
}

impl NativePath {
    pub fn as_paths(&self) -> Vec<&str> {
        match self {
            NativePath::Single(p) => vec![p.as_str()],
            NativePath::Multiple(ps) => ps.iter().map(String::as_str).collect(),
        }
    }
}

// ---- CompiledCode ----
/// Declares where a module's compiled code lives, its build-time-
/// only R package deps (e.g. `Rcpp`), whose headers a `Makevars` needs
/// to find via `system.file()` before `R CMD SHLIB` can run, and how
/// `baler bundle` packages it.
///
/// `path` is relative to the module's own source directory (whatever
/// `resolve_src_dir()` resolves to), the same base `src` in
/// `[project]` already uses, not the project root `baler.toml` lives
/// in. `path = ["cpp", "extra/src"]` in a module's own `baler.toml`
/// means exactly what it looks like: two dirs nested under that
/// module's source tree.
///
/// `path` is optional and exists purely as an override. When omitted,
/// `resolve_native_dirs()` scans the module's whole source tree for
/// compiled-code dirs instead of assuming one is where it must live.
///
/// `binary` and `keep_source` replaced the `--binary` and
/// `--keep-source` flags of `baler bundle`. They describe what a
/// module ships, which belongs to the module and not to whoever
/// happens to run the command.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CompiledCode {
    pub path: Option<NativePath>,
    pub build_deps: Option<BTreeMap<String, PackageDep>>,
    /// `baler bundle` compiles the native code in place and ships the
    /// tagged binary. Native source is stripped from the archive
    /// unless `keep_source` is also set.
    #[serde(default)]
    pub binary: bool,
    /// Only valid together with `binary = true`. Also ships native
    /// source next to the binary, so install can fall back to
    /// compiling when the binary's tag doesn't match the machine.
    #[serde(default)]
    pub keep_source: bool,
}

// Which platform a shipped binary is for? 
//
// There is no record of this in the archive, and nothing here touches
// `baler.toml` or `baler.lock`. A compiled file already says what it was
// built for in its first few bytes (the same check a CRAN binary gets on
// install), so that is what gets read.

/// Where a finished build is copied to, next to the module's `__init__.r`.
pub const LIB_DIR: &str = ".lib";

/// Inside `.lib/`, one directory per extra platform in a merged bundle.
pub const PLATFORMS_DIR: &str = ".platforms";

/// How many leading bytes of a binary are read to tell its platform.
pub const HEAD_LEN: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOs {
    Linux,
    MacOs,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryArch {
    X86_64,
    Aarch64,
}

fn os_name(os: BinaryOs) -> &'static str {
    match os {
        BinaryOs::Linux => "linux",
        BinaryOs::MacOs => "macos",
        BinaryOs::Windows => "windows",
    }
}

fn arch_name(arch: BinaryArch) -> &'static str {
    match arch {
        BinaryArch::X86_64 => "x86_64",
        BinaryArch::Aarch64 => "aarch64",
    }
}

/// What a binary was built for, or what a machine runs. `archs` is empty
/// for a CPU baler doesn't know, which fits no host, and has more than
/// one entry for a macOS universal binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryTarget {
    pub os: BinaryOs,
    pub archs: Vec<BinaryArch>,
}

impl BinaryTarget {
    /// For messages, for example `macos aarch64`.
    pub fn describe(&self) -> String {
        let os = os_name(self.os);
        if self.archs.is_empty() {
            return format!("{os} (another cpu)");
        }
        let archs = self.archs.iter().map(|arch| arch_name(*arch)).collect::<Vec<_>>();
        format!("{os} {}", archs.join("/"))
    }

    /// A name safe for a directory, for example `macos-aarch64`.
    /// `None` for a CPU baler doesn't know.
    pub fn dir_name(&self) -> Option<String> {
        if self.archs.is_empty() {
            return None;
        }
        let archs = self.archs.iter().map(|arch| arch_name(*arch)).collect::<Vec<_>>();
        Some(format!("{}-{}", os_name(self.os), archs.join("-")))
    }

    /// Whether a binary built for `self` loads on `host`.
    pub fn fits(&self, host: &BinaryTarget) -> bool {
        self.os == host.os && host.archs.iter().any(|arch| self.archs.contains(arch))
    }
}

/// Reads the platform out of an ELF, Mach-O or PE header. `None` means
/// the bytes are none of those, so there is nothing to compare.
pub fn sniff_binary(head: &[u8]) -> Option<BinaryTarget> {
    if head.len() >= 20 && head[..4] == [0x7f, b'E', b'L', b'F'] {
        let machine = match head[5] {
            1 => u16::from_le_bytes([head[18], head[19]]),
            2 => u16::from_be_bytes([head[18], head[19]]),
            _ => return None,
        };
        let archs = match machine {
            0x3E => vec![BinaryArch::X86_64],
            0xB7 => vec![BinaryArch::Aarch64],
            _ => Vec::new(),
        };
        return Some(BinaryTarget { os: BinaryOs::Linux, archs });
    }

    if head.len() >= 8 && (head[..4] == [0xcf, 0xfa, 0xed, 0xfe] || head[..4] == [0xce, 0xfa, 0xed, 0xfe]) {
        let cpu = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
        return Some(BinaryTarget { os: BinaryOs::MacOs, archs: macho_arch(cpu).into_iter().collect() });
    }

    if head.len() >= 8 && head[..4] == [0xca, 0xfe, 0xba, 0xbe] {
        let count = u32::from_be_bytes([head[4], head[5], head[6], head[7]]) as usize;
        // Java class files share this magic, and have a large number here.
        if count == 0 || count > 8 || head.len() < 8 + 20 * count {
            return None;
        }
        let archs = (0..count)
            .filter_map(|i| {
                let at = 8 + 20 * i;
                macho_arch(u32::from_be_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]))
            })
            .collect();
        return Some(BinaryTarget { os: BinaryOs::MacOs, archs });
    }

    if head.len() >= 0x40 && head[..2] == *b"MZ" {
        let pe = u32::from_le_bytes([head[0x3C], head[0x3D], head[0x3E], head[0x3F]]) as usize;
        if head.len() >= pe + 6 && head[pe..pe + 4] == *b"PE\0\0" {
            let archs = match u16::from_le_bytes([head[pe + 4], head[pe + 5]]) {
                0x8664 => vec![BinaryArch::X86_64],
                0xAA64 => vec![BinaryArch::Aarch64],
                _ => Vec::new(),
            };
            return Some(BinaryTarget { os: BinaryOs::Windows, archs });
        }
    }

    None
}

fn macho_arch(cpu: u32) -> Option<BinaryArch> {
    match cpu {
        0x0100_0007 => Some(BinaryArch::X86_64),
        0x0100_000C => Some(BinaryArch::Aarch64),
        _ => None,
    }
}

/// The platform of the first file whose header is a known binary format.
pub fn first_target(heads: &[Vec<u8>]) -> Option<BinaryTarget> {
    heads.iter().find_map(|head| sniff_binary(head))
}

/// The OS and CPU an `R.version$platform` string stands for, or `None`
/// for one baler doesn't recognise.
pub fn target_of_platform(platform: &str) -> Option<BinaryTarget> {
    let platform = platform.trim().to_lowercase();
    let arch = if platform.starts_with("x86_64") || platform.starts_with("amd64") {
        BinaryArch::X86_64
    } else if platform.starts_with("aarch64") || platform.starts_with("arm64") {
        BinaryArch::Aarch64
    } else {
        return None;
    };
    let os = if platform.contains("linux") {
        BinaryOs::Linux
    } else if platform.contains("darwin") {
        BinaryOs::MacOs
    } else if platform.contains("mingw") || platform.contains("windows") {
        BinaryOs::Windows
    } else {
        return None;
    };
    Some(BinaryTarget { os, archs: vec![arch] })
}
