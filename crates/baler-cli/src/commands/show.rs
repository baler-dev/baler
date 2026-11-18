use anyhow::Result;

pub struct ShowArgs {
    pub name: String,
}

pub fn run(args: ShowArgs) -> Result<()> {
    baler_core::ops::show::run(&args.name)
}
