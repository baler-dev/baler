use anyhow::Result;
use std::path::PathBuf;

use baler_core::ops::compile::CompileMode;

pub struct CompileArgs {
    pub path: String,
    pub clean: bool,
    pub rebuild: bool,
}

/// Thin CLI wrapper of `baler_core::ops::compile()`. `clean` and
/// `rebuild` are mutually exclusive at the clap level (`conflicts_with`
/// on both flags in `lib.rs`), so at most one of them is ever `true`
/// here.
pub fn run(args: CompileArgs) -> Result<()> {
    let project_root = PathBuf::from(&args.path);

    let mode = if args.clean {
        CompileMode::Clean
    } else if args.rebuild {
        CompileMode::Rebuild
    } else {
        CompileMode::Normal
    };

    let compiled = baler_core::ops::compile::run(&project_root, mode)?;

    if mode == CompileMode::Clean {
        println!("Cleared cached and compiled native artifacts for this module.");
        return Ok(());
    }

    if compiled.is_empty() {
        println!("No native code to compile.");
        return Ok(());
    }

    for artifact in &compiled {
        println!(
            "Compiled {} -> {} ({})",
            artifact.native_dir.display(),
            artifact.artifact_path.display(),
            if artifact.from_cache { "cached" } else { "compiled" }
        );
    }
    println!();
    println!("Native code compiled in place. box::use() will pick it up on next load.");

    Ok(())
}
