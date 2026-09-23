pub mod commands; 

use anyhow::Result;
use clap::{Parser, Subcommand};

use commands::{
    bundle::BundleArgs,
    compile::CompileArgs,
    init::InitArgs,
    install::InstallArgs,
    lock::LockArgs,
    remove::RemoveArgs,
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
    /// Scaffold a new box module
    Init {
        /// Name of the module to create
        name: String,

        /// Override the project directory name.
        /// Defaults to <name>-proj if not specified.
        #[arg(long)]
        dir_name: Option<String>,

        /// Scaffold compiled-code support: c or cpp
        #[arg(long)]
        native: Option<String>,

        /// Binding library for --native (cpp: rcpp (default) or cpp11)
        #[arg(long)]
        backend: Option<String>,
    },

    /// Bundle a module into <name>_<version>.tar.gz
    Bundle {
        /// Path to the project root (e.g. `.` or `./my-project`)
        #[arg(default_value = ".")]
        path: String,

        /// Also compile native code in place and include the tagged
        /// binary in the archive. Native source is stripped from the
        /// archive unless --keep-source is also passed — a mismatched
        /// or missing tag on install then has nothing to fall back to.
        #[arg(long)]
        binary: bool,

        /// Only valid with --binary. Also ships native source
        /// alongside the compiled binary, so install can fall back to
        /// compiling if the tag doesn't match this machine.
        #[arg(long)]
        keep_source: bool,
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

        #[arg(long, help = "Automatically install R package dependencies from CRAN")]
        install_deps: bool,

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
    /// installing anything
    Lock {
        /// Path to the project root (e.g. `.` or `./my-project`)
        path: String,

        #[arg(long, conflicts_with = "remove", help = "Ignore any existing baler.lock and re-resolve everything fresh instead of reusing its pins")]
        update: bool,

        #[arg(long, conflicts_with = "remove", help = "Record the R version currently on PATH in baler.lock, as provenance only, not enforced on install")]
        with_rver: bool,

        #[arg(long, help = "Delete baler.lock instead of writing one. This is the safe version of removal, baler just resolves fresh without it")]
        remove: bool,
    },

    /// Remove an installed module
    Remove {
        /// Name of the module to remove
        name: String,

        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },
}

pub fn run() {
    let cli = Cli::parse();

    let result: Result<()> = match cli.command {
        Commands::Init { name, dir_name, native, backend } => {
            commands::init::run(InitArgs { name, dir_name, native, backend })
        }
        Commands::Compile { path, clean, rebuild } => {
            commands::compile::run(CompileArgs { path, clean, rebuild })
        }
        Commands::Bundle { path, binary, keep_source } => {
            commands::bundle::run(BundleArgs { path, binary, keep_source })
        }
        Commands::Install { source, install_deps, repo, version, path, git, branch, tag, rev, url, module_dir } => {
            commands::install::run(InstallArgs {
                source, install_deps, repo, version, path, git, branch, tag, rev, url, module_dir,
            })
        }
        Commands::Lock { path, update, with_rver, remove } => {
            commands::lock::run(LockArgs { path, update, with_rver, remove })
        }
        Commands::Remove { name, force } => {
            commands::remove::exec(RemoveArgs { name, force })
        }
    };

    if let Err(e) = result {
        eprintln!("error: {e:?}");
        std::process::exit(1);
    }
}
