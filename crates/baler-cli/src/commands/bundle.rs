use anyhow::Result;

pub struct BundleArgs {
    pub path: String,
}

pub fn run(args: BundleArgs) -> Result<()> {
    baler_core::ops::bundle::run(&args.path)
}
