use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::installed::{self, InstalledModule, Listing};
use crate::paths::resolve_install_dir;

/// What `baler list` was asked to show. The default asks for every
/// installed module, as a table.
#[derive(Debug, Default, Clone)]
pub struct ListOptions {
    /// A pattern the whole module name has to match. `*` stands for any
    /// run of characters, including none, and `?` for exactly one
    /// character. Case is ignored.
    pub pattern: Option<String>,
    /// Keywords from `[project].keywords` in the module's `baler.toml`. A
    /// module has to carry all of them. Case is ignored.
    pub keywords: Vec<String>,
    /// Print the names only, one for each line, with no table.
    pub names_only: bool,
}

impl ListOptions {
    /// Refuses a pattern or a keyword that is empty. Neither can match
    /// anything useful, so it is a typo, such as a shell variable that
    /// expanded to nothing, and silently listing nothing would hide it.
    pub fn check(&self) -> Result<()> {
        if self.pattern.as_deref().is_some_and(|pattern| pattern.trim().is_empty()) {
            bail!("The pattern is empty. Leave it out to list every module.");
        }
        if self.keywords.iter().any(|keyword| keyword.trim().is_empty()) {
            bail!("A --keyword is empty. Give the keyword to look for.");
        }
        Ok(())
    }

    /// Whether anything narrows the listing down.
    pub fn is_filtered(&self) -> bool {
        self.pattern.is_some() || !self.keywords.is_empty()
    }

    fn name_is_wanted(&self, name: &str) -> bool {
        match &self.pattern {
            Some(pattern) => name_matches(pattern, name),
            None => true,
        }
    }

    fn keywords_are_carried(&self, module: &InstalledModule) -> bool {
        let carried: Vec<String> = module
            .toml
            .project
            .keywords
            .iter()
            .map(|keyword| keyword.trim().to_lowercase())
            .collect();
        self.keywords
            .iter()
            .all(|wanted| carried.contains(&wanted.trim().to_lowercase()))
    }

    /// The filters in words, for the line printed when nothing matches.
    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(pattern) = &self.pattern {
            parts.push(format!("the pattern '{pattern}'"));
        }
        for keyword in &self.keywords {
            parts.push(format!("the keyword '{keyword}'"));
        }
        parts.join(" and ")
    }
}

/// `baler list`: the modules installed in the install directory.
pub fn run(options: &ListOptions) -> Result<()> {
    options.check()?;
    let install_dir = resolve_install_dir().context("Failed to resolve install directory")?;
    let listing = select(&install_dir, options)?;

    for problem in &listing.problems {
        eprintln!("warning: could not read the record of {problem}");
    }
    print!("{}", render_with(&listing, &install_dir.display().to_string(), options));
    Ok(())
}

/// The modules in `install_dir` that `options` asks for. The pattern is
/// applied before a record is read, and the keywords after, since they
/// live in the record.
pub fn select(install_dir: &Path, options: &ListOptions) -> Result<Listing> {
    let mut listing = installed::list_where(install_dir, |name| options.name_is_wanted(name))?;
    listing.modules.retain(|module| options.keywords_are_carried(module));
    Ok(listing)
}

/// Whether `name` fits `pattern`: the whole name, with `*` for any run of
/// characters and `?` for any one, ignoring case.
///
/// It walks both strings once. At a `*` it remembers where it was, and
/// when a later character does not fit it lets that `*` swallow one more
/// character and tries again. That keeps the cost proportional to the two
/// lengths multiplied, with no recursion to blow up on a pattern such as
/// `*a*a*a*a*b`.
pub fn name_matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let name: Vec<char> = name.to_lowercase().chars().collect();

    let mut at_pattern = 0;
    let mut at_name = 0;
    let mut last_star: Option<(usize, usize)> = None;
    while at_name < name.len() {
        if at_pattern < pattern.len() && pattern[at_pattern] == '*' {
            last_star = Some((at_pattern, at_name));
            at_pattern += 1;
        } else if at_pattern < pattern.len()
            && (pattern[at_pattern] == '?' || pattern[at_pattern] == name[at_name])
        {
            at_pattern += 1;
            at_name += 1;
        } else if let Some((star, swallowed)) = last_star {
            at_pattern = star + 1;
            at_name = swallowed + 1;
            last_star = Some((star, swallowed + 1));
        } else {
            return false;
        }
    }
    pattern[at_pattern..].iter().all(|c| *c == '*')
}

/// The table `baler list` prints, one line for each module. `location`
/// is the install directory, named only when nothing is installed.
pub fn render(listing: &Listing, location: &str) -> String {
    render_with(listing, location, &ListOptions::default())
}

/// What `baler list` prints for `options`: the table, or the names alone.
/// With names only and nothing to show, the text is empty, so a script
/// reading it sees no lines instead of a sentence.
pub fn render_with(listing: &Listing, location: &str, options: &ListOptions) -> String {
    if listing.modules.is_empty() {
        if options.names_only {
            return String::new();
        }
        if options.is_filtered() {
            return format!("No installed modules match {}.\n", options.describe());
        }
        return format!("No modules installed in {location}.\n");
    }

    if options.names_only {
        return listing
            .modules
            .iter()
            .map(|module| format!("{}\n", module.name))
            .collect();
    }

    let header = ["NAME", "VERSION", "LICENSE", "R"];
    let mut rows: Vec<[String; 4]> = vec![header.map(str::to_owned)];
    for module in &listing.modules {
        rows.push([
            module.name.clone(),
            module.version.clone(),
            module.toml.project.license.clone(),
            module.toml.project.r_version.clone(),
        ]);
    }

    let widths: Vec<usize> = (0..header.len())
        .map(|column| rows.iter().map(|row| row[column].chars().count()).max().unwrap_or(0))
        .collect();

    let mut out = String::new();
    for row in &rows {
        let cells: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        out.push_str(cells.join("  ").trim_end());
        out.push('\n');
    }
    out
}
