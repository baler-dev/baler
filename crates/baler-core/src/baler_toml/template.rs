use baler_native::{Backend, NativeLang};

use super::BalerToml;

impl BalerToml {
    /// `native` is `Some((lang, backend))` when `baler init` was run
    /// with `--native`. When it's `None`, the generated `baler.toml`
    /// has no `[compiled-code]` section at all, not even a commented
    /// placeholder, someone not working with native code shouldn't
    /// see a block about it in a file they just scaffolded. `path`
    /// inside that section is written relative to the module's own
    /// source directory, matching how `resolve_native_dirs()` resolves
    /// it, not the project root.
    pub fn default_template(name: &str, native: Option<(NativeLang, Option<Backend>)>) -> String {
        let mut out = format!(
            r#"[project]
name = "{name}"
version = "0.1.0"
description = ""
authors = [
    {{ name = "Your Name", email = "you@example.com" }},
]
license = "Unknown"
r_version = ">=4.0.0"
repository = ""
keywords = []
# readme = "README.md"
# src = "{name}"    # path to the source directory containing __init__.R
                    # defaults to a directory named after the module

[dependencies]
# dplyr = "*"
# ggplot2 = ">=3.4.0"
# fable = {{ version = "*", repo = "https://tidyverts.r-universe.dev/" }}

[dependencies.baler]
# other_module = {{ version = "*", source = "https://github.com/user/repo" }}

# [extras]/[extras.baler] hold deps only needed outside normal
# runtime use (tests, mocks, dev tooling), same shape as
# [dependencies]/[dependencies.baler] above. [extras.external] adds
# one more kind found only here: external CLI tools, never loaded by
# box, so never part of [dependencies] at all.
[extras]
# testthat = "*"

[extras.baler]
# mock_module = {{ version = "*", source = "https://github.com/user/repo" }}

[extras.external]
# baler doesn't install these, only checks for them. A bare string is
# a version spec; an inline table can carry whatever else that
# particular tool needs, since baler itself only reads `version`.
# quarto = ">=1.4"
"#
        );

        if let Some((lang, backend)) = native {
            let build_deps_line = match lang {
                NativeLang::Cpp => match backend.unwrap_or_default() {
                    Backend::Rcpp => "build_deps = { Rcpp = \"*\" }",
                    Backend::Cpp11 => "build_deps = { cpp11 = \"*\" }",
                },
                _ => "# build_deps = { Rcpp = \"*\" }",
            };
            out.push_str(&format!(
                "\n[compiled-code]\n\
                 # Native code is auto-detected under this module's source dir, no\n\
                 # path needed for the default src/ layout. Only set path if compiled\n\
                 # code lives somewhere else, or in more than one place.\n\
                 # path = \"native/\"\n\
                 # path can also be an array: path = [\"native/\", \"extra/src\"]\n\
                 {build_deps_line}\n\
                 # build_deps is resolved and installed before compiling.\n\
                 # Does not imply a runtime dependency; list it in\n\
                 # [dependencies] too if the compiled code also needs it\n\
                 # loaded at runtime\n"
            ));
        }

        out.push_str(
            "\n# [tool.test]\n\
             # framework = \"testthat\"\n\
             # dir = \"tests\"\n"
        );

        out
    }
}
