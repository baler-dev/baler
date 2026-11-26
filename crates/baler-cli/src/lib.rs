pub mod commands; 

use anyhow::Result;
use baler_core::settings::{self, SettingsLayer};
use clap::{Parser, Subcommand};

use commands::{
    box_pkg::{BoxInstallArgs, BoxStatusArgs},
    bundle::BundleArgs,
    compile::CompileArgs,
    init::InitArgs,
    install::InstallArgs,
    list::ListArgs,
    lock::LockArgs,
    remove::RemoveArgs,
    show::ShowArgs,
};

#[derive(Parser)]
#[command(name = "baler")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "A bundler and package manager for box modules")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scaffold a box module. With a NAME, creates <NAME>-proj/. Without
    /// one, initializes the current directory in place, like `uv init`:
    /// writes baler.toml, finds an existing module directory, and never
    /// overwrites a file.
    Init {
        /// Name of the module to create in a new directory. Leave out to
        /// initialize the current directory instead.
        name: Option<String>,

        /// Override the project directory name.
        /// Defaults to <name>-proj if not specified.
        #[arg(long, requires = "name")]
        dir_name: Option<String>,

        /// In-place only: name of the module. Defaults to the name of
        /// the module directory found, or of the current directory.
        #[arg(long = "name", conflicts_with = "name")]
        module_name: Option<String>,

        /// In-place only: the directory holding the module's source,
        /// relative to here. Defaults to searching for one with an
        /// __init__.r, or else one holding R files.
        #[arg(long, conflicts_with = "name")]
        src: Option<String>,

        /// Scaffold compiled-code support: c or cpp. Only for a new
        /// module, not one that already has files.
        #[arg(long)]
        native: Option<String>,

        /// Binding library for --native (cpp: rcpp (default) or cpp11)
        #[arg(long)]
        backend: Option<String>,

        /// SPDX license expression for baler.toml, such as MIT,
        /// Apache-2.0 or GPL-3.0-or-later. Defaults to Unknown.
        #[arg(long)]
        license: Option<String>,
    },

    /// Bundle a module into <name>_<version>.tar.gz. Native source always
    /// ships, and `binary = true` under `[compiled-code]` in baler.toml
    /// adds the binary compiled on this platform. Running it on another
    /// platform against the same archive adds that platform's binary, and
    /// --merge adds the binaries of archives made on other platforms.
    Bundle {
        /// Path to the project root (e.g. `.` or `./my-project`)
        #[arg(default_value = ".")]
        path: String,

        /// Fail instead of warning when baler.lock is missing, or doesn't
        /// match baler.toml. Like `cargo publish --locked`. The lock ships
        /// inside the bundle, so a stale one breaks installs of it.
        #[arg(long)]
        locked: bool,

        /// Also merge into the bundle the binaries of these bundles of the
        /// same module, made on other platforms. The bundle is compiled
        /// for this platform as usual. Each archive must come from the
        /// same files as the project. A path, if given, goes before this
        /// flag: `baler bundle --merge a.tar.gz b.tar.gz`.
        #[arg(long, num_args = 1.., value_name = "ARCHIVE")]
        merge: Vec<String>,
    },

    /// Compile a module's native code in place for local dev/testing.
    /// Does not delete the native source, unlike the build step that
    /// runs during `install`.
    Compile {
        /// Path to the project root (e.g. `.` or `./my-project`)
        #[arg(default_value = ".")]
        path: String,

        /// Remove this module's cached and compiled native artifacts
        /// and stop, without compiling. Equivalent to
        /// pkgbuild::clean_dll(), not a rebuild.
        #[arg(long, conflicts_with = "rebuild")]
        clean: bool,

        /// Force a real recompile even if the source hash hasn't
        /// changed, by evicting this module's cache first. Unlike
        /// --clean, this still compiles.
        #[arg(long, conflicts_with = "clean")]
        rebuild: bool,
    },

    /// Install a module: a registry name (positional, --repo/
    /// --version), a local path or .tar.gz (--path), a GitHub repo
    /// (--git, still GitHub-tarball-only underneath despite the
    /// name), or a direct tarball URL (--url). --git takes
    /// --module-dir for a repo holding more than one module.
    Install {
        /// Bare module name for a registry lookup (registries aren't
        /// implemented yet)
        #[arg(conflicts_with_all = ["path", "git", "url"])]
        source: Option<String>,

        #[arg(
            long,
            overrides_with = "no_install_deps",
            help = "Automatically install R package dependencies from CRAN. Off by default, see `baler config install-deps`"
        )]
        install_deps: bool,

        #[arg(
            long,
            overrides_with = "install_deps",
            help = "Do not install dependencies, even when `baler config install-deps` is on"
        )]
        no_install_deps: bool,

        #[arg(
            long,
            conflicts_with = "no_install_deps",
            help = "Also install the module's [extras] R packages (test frameworks, dev tooling). Needs --install-deps, or `baler config install-deps on`"
        )]
        extras: bool,

        #[arg(
            long,
            help = "Registry URL to look SOURCE up in (registries aren't implemented yet)",
            requires = "source",
            conflicts_with_all = ["path", "git", "url"]
        )]
        repo: Option<String>,

        #[arg(
            long,
            help = "Version constraint for a registry lookup",
            requires = "source",
            conflicts_with_all = ["path", "git", "url"]
        )]
        version: Option<String>,

        #[arg(
            long,
            help = "Install from a local directory or .tar.gz",
            conflicts_with_all = ["source", "git", "url"]
        )]
        path: Option<String>,

        #[arg(
            long,
            help = "Install from a GitHub repo, e.g. https://github.com/user/repo (GitHub-tarball fetch only, not generic git, despite the flag name)",
            conflicts_with_all = ["source", "path", "url"]
        )]
        git: Option<String>,

        #[arg(long, help = "Branch to install from --git", requires = "git", conflicts_with_all = ["tag", "rev"])]
        branch: Option<String>,

        #[arg(long, help = "Tag to install from --git", requires = "git", conflicts_with_all = ["branch", "rev"])]
        tag: Option<String>,

        #[arg(long, help = "Commit to install from --git", requires = "git", conflicts_with_all = ["branch", "tag"])]
        rev: Option<String>,

        #[arg(
            long,
            help = "Install directly from an already-bundled archive URL, e.g. a GitHub Release asset produced by `baler bundle`",
            conflicts_with_all = ["source", "path", "git"]
        )]
        url: Option<String>,

        #[arg(
            long,
            help = "Directory within --git's repo that holds the module, for a repo holding more than one",
            requires = "git",
            conflicts_with_all = ["source", "path", "url"]
        )]
        module_dir: Option<String>,
    },

    /// Resolve R package dependencies and write baler.lock, without
    /// installing anything. An existing lock is kept as it is unless
    /// baler.toml changed: then every pin that still fits stays, and only
    /// what changed is added, dropped or re-resolved.
    Lock {
        /// Path to the project root (e.g. `.` or `./my-project`)
        #[arg(default_value = ".")]
        path: String,

        #[arg(
            long,
            num_args = 0..,
            value_name = "PKG",
            conflicts_with_all = ["remove", "check"],
            help = "Re-resolve instead of reusing pins. Alone, re-resolves everything. With names (--update purrr cli), re-resolves only those and keeps every other pin"
        )]
        update: Option<Vec<String>>,

        #[arg(long, conflicts_with_all = ["remove", "check"], help = "Record the R version currently on PATH in baler.lock, as provenance only, not enforced on install. Only when creating the lock or with --update")]
        with_rver: bool,

        #[arg(long, conflicts_with = "remove", help = "Don't write anything. Exit with an error if baler.lock is missing or doesn't match baler.toml, for CI")]
        check: bool,

        #[arg(long, help = "Delete baler.lock instead of writing one. This is the safe version of removal, baler just resolves fresh without it")]
        remove: bool,
    },

    /// List the `box-bale` modules installed by baler: name, version, license and
    /// the R version each asks for. Give a PATTERN, --keyword or both to
    /// list only the modules that match.
    List {
        /// Only the modules whose whole name matches this pattern. `*`
        /// stands for any run of characters and `?` for any one, so
        /// 'conv*' matches `convert` and '*calc*' matches `rcalc`. Case
        /// is ignored. Quote it, or the shell may expand the `*` first.
        pattern: Option<String>,

        /// Only `box-bale` modules that list this keyword in their baler.toml.
        /// Give several, separated by commas or by repeating the flag, and
        /// a module must have all of them: `-k stats,plots` is the same as
        /// `-k stats -k plots`. Case is ignored.
        #[arg(short, long, visible_alias = "tag", value_name = "KEYWORD", value_delimiter = ',')]
        keyword: Vec<String>,

        /// Print only the module names, one for each line, with no table.
        /// Meant for scripts.
        #[arg(long)]
        names_only: bool,
    },

    /// Show what one installed module declared: description, license,
    /// authors, dependencies, and where its files and metadata are.
    Show {
        /// Name of the installed `box-bale` module, as `baler list` shows it
        name: String,
    },

    /// Remove an installed module
    Remove {
        /// Name of the module to remove
        name: String,

        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },

    /// Install and inspect the patched {box} that baler modules are loaded
    /// with. It comes pre-built from the baler release, so no compiler is
    /// needed.
    #[command(name = "box")]
    BoxPkg {
        #[command(subcommand)]
        command: BoxCommands,
    },
}

#[derive(Subcommand)]
enum BoxCommands {
    /// Install the pre-built {box} that matches this machine's R and platform
    /// into the R library. It never touches an upstream {box} unless --force
    /// is given, and does nothing when the installed build is already current.
    Install {
        /// Release tag to take {box} from. Defaults to the latest release,
        /// the one whose {box} files are kept up to date.
        #[arg(long)]
        tag: Option<String>,

        /// Replace a {box} that is already installed, even an upstream one,
        /// and install again even if the installed build is current.
        #[arg(long)]
        force: bool,
    },

    /// Show which {box} is installed, whether it has baler support, and
    /// which commit it was built from. Reads files only, no network.
    Status {
        /// Exit with an error unless a {box} with baler support is
        /// installed, for CI and scripts.
        #[arg(long)]
        check: bool,
    },
}

/// What the install flags say, if they were given. When both are given,
/// clap keeps only the last one, so at most one of them is true here.
fn install_deps_flag(command: &Commands) -> Option<bool> {
    match command {
        Commands::Install { install_deps, no_install_deps, .. } => {
            if *no_install_deps {
                Some(false)
            } else if *install_deps {
                Some(true)
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn run() {
    let cli = Cli::parse();

    let loaded = match settings::init(SettingsLayer {
        install_deps: install_deps_flag(&cli.command),
        lib: None,
    }) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("error: {e:?}");
            std::process::exit(1);
        }
    };

    let result: Result<()> = match cli.command {
        Commands::Init { name, dir_name, module_name, src, native, backend, license } => {
            commands::init::run(InitArgs { name, dir_name, module_name, src, native, backend, license })
        }
        Commands::Compile { path, clean, rebuild } => {
            commands::compile::run(CompileArgs { path, clean, rebuild })
        }
        Commands::Bundle { path, locked, merge } => {
            commands::bundle::run(BundleArgs { path, locked, merge })
        }
        Commands::Install { source, extras, repo, version, path, git, branch, tag, rev, url, module_dir, .. } => {
            commands::install::run(InstallArgs {
                source,
                install_deps: loaded.install_deps.value,
                extras,
                repo,
                version,
                path,
                git,
                branch,
                tag,
                rev,
                url,
                module_dir,
            })
        }
        Commands::Lock { path, update, with_rver, check, remove } => {
            commands::lock::run(LockArgs { path, update, with_rver, remove, check })
        }
        Commands::List { pattern, keyword, names_only } => {
            commands::list::run(ListArgs { pattern, keyword, names_only })
        }
        Commands::Show { name } => commands::show::run(ShowArgs { name }),
        Commands::Remove { name, force } => {
            commands::remove::exec(RemoveArgs { name, force })
        }
        Commands::BoxPkg { command } => match command {
            BoxCommands::Install { tag, force } => {
                commands::box_pkg::install(BoxInstallArgs { tag, force })
            }
            BoxCommands::Status { check } => {
                commands::box_pkg::status(BoxStatusArgs { check })
            }
        },
    };

    if let Err(e) = result {
        eprintln!("error: {e:?}");
        std::process::exit(1);
    }
}
