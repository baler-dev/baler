use baler_native::{Backend, NativeLang};

use super::BalerToml;

/// What `baler init` found in an existing directory and wants written
/// into the manifest in place of the placeholder defaults.
#[derive(Debug, Default, Clone, Copy)]
pub struct TemplateDefaults<'a> {
    /// A source directory that isn't named after the module. Written as
    /// a real `src = "..."` line instead of the commented-out default.
    pub src: Option<&'a str>,
    /// An existing README to point `readme` at.
    pub readme: Option<&'a str>,
}

/// Escapes a value for a TOML basic string.
fn toml_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

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
        Self::default_template_with(name, native, TemplateDefaults::default())
    }

    /// Same as `default_template`, with the values `baler init` worked
    /// out from an existing directory filled in.
    pub fn default_template_with(
        name: &str,
        native: Option<(NativeLang, Option<Backend>)>,
        defaults: TemplateDefaults<'_>,
    ) -> String {
        let readme_line = match defaults.readme {
            Some(path) => format!("readme = \"{}\"", toml_str(path)),
            None => "# readme = \"README.md\"".to_owned(),
        };
        let src_line = match defaults.src {
            Some(dir) => format!(
                "src = \"{}\"    # path to the source directory containing __init__.R",
                toml_str(dir)
            ),
            None => format!(
                "# src = \"{name}\"    # path to the source directory containing __init__.R"
            ),
        };

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
{readme_line}
{src_line}
                    # defaults to a directory named after the module

[dependencies]
# dplyr = "*"
# ggplot2 = ">=3.4.0"
# fable = {{ version = "*", repo = "https://tidyverts.r-universe.dev/" }}

[dependencies.baler]
# A module needs a source: `git` (a github.com repo) or `url` (a
# bundled .tar.gz). `git` can take one of branch/tag/rev, and
# module_dir for a repo holding more than one module. `version` is
# optional.
# other_module = {{ git = "https://github.com/user/repo", tag = "v1.0.0" }}

# [extras]/[extras.baler] hold deps only needed outside normal
# runtime use (tests, mocks, dev tooling), same shape as
# [dependencies]/[dependencies.baler] above. [extras.external] adds
# one more kind found only here: external tools (command-line
# programs, Python packages), never loaded by box, so never part of
# [dependencies] at all.
[extras]
# testthat = "*"

[extras.baler]
# mock_module = {{ git = "https://github.com/user/repo" }}

[extras.external]
# baler doesn't install these, only checks for them. A bare string is
# a command-line tool with that version requirement. A table picks a
# type: "cli" (the default, with an optional `hint`) or "python" (a
# PyPI package, checked in one Python interpreter).
# quarto = ">=1.4"
# numpy = {{ type = "python", version = ">=1.24" }}
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
             # dir = \"tests\"\n",
        );

        out
    }
}
