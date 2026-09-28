use crate::baler_toml::Extras;

/// Checks each `[extras.external]` entry for a matching binary on
/// PATH, printing a note for anything missing. Never fails and never
/// blocks the caller: an external tool (a linter, Quarto) is a
/// development nicety, not something the module needs in order to
/// run, so a missing one is worth flagging, not worth failing a
/// compile or install over.
///
/// Presence-only. Nothing here verifies the declared version
/// constraint against what's actually installed: there is no generic,
/// cross-tool way to ask an arbitrary CLI "what version are you" the
/// way there is for an R package's DESCRIPTION file. The version is
/// still printed alongside the name, as information for whoever reads
/// the output, not as something baler actually checked.
pub fn check_external_tools(extras: Option<&Extras>) {
    let Some(tools) = extras.and_then(|e| e.external.as_ref()) else {
        return;
    };

    for (name, dep) in tools {
        if !on_path(name) {
            println!(
                "  [extras.external] '{}' ({}) not found on PATH. Not \
                 required to run this module, but declared as a \
                 recommended tool for working on it.",
                name,
                dep.version()
            );
        }
    }
}

fn on_path(name: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path_var).any(|dir| {
        if cfg!(windows) {
            ["exe", "cmd", "bat"]
                .iter()
                .any(|ext| dir.join(format!("{name}.{ext}")).is_file())
        } else {
            dir.join(name).is_file()
        }
    })
}
