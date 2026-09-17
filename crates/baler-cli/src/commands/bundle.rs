use anyhow::Result;

pub struct BundleArgs {
    pub path: String,
    pub binary: bool,
    pub keep_source: bool,
}

pub fn run(args: BundleArgs) -> Result<()> {
    baler_core::ops::bundle::run(&args.path, args.binary, args.keep_source)
}
