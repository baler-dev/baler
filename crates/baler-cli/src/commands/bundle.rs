use anyhow::Result;

pub struct BundleArgs {
    pub path: String,
    /// Fail instead of warning when baler.lock is missing or doesn't
    /// match baler.toml.
    pub locked: bool,
}

pub fn run(args: BundleArgs) -> Result<()> {
    baler_core::ops::bundle::run_with(&args.path, args.locked)
}
