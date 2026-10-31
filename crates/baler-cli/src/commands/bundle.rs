use anyhow::Result;

pub struct BundleArgs {
    pub path: String,
    /// Fail instead of warning when baler.lock is missing or doesn't
    /// match baler.toml.
    pub locked: bool,
    /// Bundles of the same module made on other platforms. When there are
    /// any, their binaries are merged into the bundle.
    pub merge: Vec<String>,
}

pub fn run(args: BundleArgs) -> Result<()> {
    if args.merge.is_empty() {
        baler_core::ops::bundle::run_with(&args.path, args.locked)
    } else {
        baler_core::ops::bundle::merge_with(&args.path, args.locked, &args.merge)
    }
}
