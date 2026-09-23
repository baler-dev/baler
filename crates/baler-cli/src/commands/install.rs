use anyhow::Result;

pub struct InstallArgs {
    pub source: Option<String>,
    pub install_deps: bool,
    pub repo: Option<String>,
    pub version: Option<String>,
    pub path: Option<String>,
    pub git: Option<String>,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub rev: Option<String>,
    pub url: Option<String>,
    pub module_dir: Option<String>,
}

pub fn run(args: InstallArgs) -> Result<()> {
    let req = baler_core::ops::install::InstallRequest {
        source: args.source,
        repo: args.repo,
        version: args.version,
        path: args.path,
        git: args.git,
        branch: args.branch,
        tag: args.tag,
        rev: args.rev,
        url: args.url,
        module_dir: args.module_dir,
    };
    baler_core::ops::install::run(req, args.install_deps)
}
