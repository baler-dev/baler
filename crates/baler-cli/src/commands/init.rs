use anyhow::{Context, Result};
use baler_core::ops::init::InPlaceOptions;
use baler_native::{Backend, NativeLang};

pub struct InitArgs {
    /// `Some` creates a new directory for the module. `None` initializes
    /// the current directory.
    pub name: Option<String>,
    pub dir_name: Option<String>,
    /// In place only: the module name, instead of working it out.
    pub module_name: Option<String>,
    /// In place only: the existing module directory.
    pub src: Option<String>,
    pub native: Option<String>,
    pub backend: Option<String>,
}

pub fn run(args: InitArgs) -> Result<()> {
    let native = args.native.as_deref().map(NativeLang::parse).transpose()?;
    let backend = args.backend.as_deref().map(Backend::parse).transpose()?;

    match args.name {
        Some(name) => {
            baler_core::ops::init::run(&name, args.dir_name.as_deref(), native, backend)
        }
        None => {
            let project_root =
                std::env::current_dir().context("Failed to get the current directory")?;
            baler_core::ops::init::run_in_place(
                &project_root,
                &InPlaceOptions {
                    name: args.module_name.as_deref(),
                    src: args.src.as_deref(),
                    native,
                    backend,
                },
            )
        }
    }
}
