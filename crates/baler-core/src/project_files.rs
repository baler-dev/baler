//! File names that count as a license or a README, so `baler show` can
//! find them in a module's `.dist-info` folder.

use std::path::Path;

/// Names that count as a license file, in lower case. Compared without
/// regard to case, so `License.md` is found on a case-sensitive
/// filesystem as well.
const LICENSE_NAMES: [&str; 9] = [
    "license",
    "license.md",
    "license.txt",
    "licence",
    "licence.md",
    "licence.txt",
    "copying",
    "copying.md",
    "copying.txt",
];

/// True for a file name that holds a license: `LICENSE`, `LICENSE.md`,
/// `LICENSE.txt`, their `LICENCE` spelling, and `COPYING` with the same
/// endings.
pub fn is_license_file_name(name: &str) -> bool {
    LICENSE_NAMES.contains(&name.to_ascii_lowercase().as_str())
}

/// True for a file name that holds a README: `README` with any ending,
/// in any case.
pub fn is_readme_file_name(name: &str) -> bool {
    Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case("readme"))
}
