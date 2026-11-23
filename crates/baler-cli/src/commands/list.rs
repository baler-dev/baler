use anyhow::Result;
use baler_core::ops::list::ListOptions;

pub struct ListArgs {
    pub pattern: Option<String>,
    pub keyword: Vec<String>,
    pub names_only: bool,
}

pub fn run(args: ListArgs) -> Result<()> {
    baler_core::ops::list::run(&ListOptions {
        pattern: args.pattern,
        keywords: args.keyword,
        names_only: args.names_only,
    })
}
