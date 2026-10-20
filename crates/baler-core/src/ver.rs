//! R-native package versions and version requirements.
//!
//! R versions are two or more non-negative integers separated by `.` or
//! `-`, compared component by component. `4.0-3` and `4.0.3` are the same
//! version, `1.0` sorts below `1.0.0`, and `1.2.3.9000` sorts above
//! `1.2.3`. `semver` cannot represent any of that, which is why this type
//! exists: it replaces the dash-as-pre-release encoding in
//! `cran::packages::normalize_r_version`.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

use anyhow::{bail, Context, Result};
use semver::{Prerelease, Version};

/// A package version. Equality, ordering and hashing use the numeric
/// components only. `as_str` keeps the text exactly as published, which
/// is what a CRAN URL or tarball file name needs.
#[derive(Debug, Clone)]
pub struct Ver {
    raw: String,
    parts: Vec<u64>,
}

impl Ver {
    pub fn parse(text: &str) -> Result<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            bail!("Invalid version: the string is empty");
        }
        let parts = trimmed
            .split(['.', '-'])
            .map(|piece| {
                if piece.is_empty() || !piece.chars().all(|c| c.is_ascii_digit()) {
                    bail!(
                        "Invalid version {trimmed:?}: {piece:?} is not a non-negative integer"
                    );
                }
                piece
                    .parse::<u64>()
                    .with_context(|| format!("Invalid version {trimmed:?}: {piece:?} is too large"))
            })
            .collect::<Result<Vec<u64>>>()?;
        Ok(Self { raw: trimmed.to_owned(), parts })
    }

    pub(crate) fn from_parts(parts: Vec<u64>) -> Self {
        let raw = parts.iter().map(u64::to_string).collect::<Vec<_>>().join(".");
        Self { raw, parts }
    }

    /// Reads a `semver` version back as an R version. The lock file keeps
    /// pins as `semver` strings with a fourth component folded into the
    /// pre-release (`1.2.3-9000`), and `-` is a separator here, so that
    /// spelling reads back as `1.2.3.9000`. A pre-release that is not made
    /// of numbers is not an R version and is an error.
    pub fn from_semver(version: &Version) -> Result<Self> {
        Self::parse(&version.to_string())
    }

    /// The `semver` shape of this version, for code that still holds
    /// `semver` values. The first three components map directly and any
    /// further ones become the pre-release (`1.2.3.9000` is `1.2.3-9000`).
    /// Ordering across that encoding is not R's, so compare `Ver`s.
    pub fn to_semver(&self) -> Version {
        let part = |index: usize| self.parts.get(index).copied().unwrap_or(0);
        let mut version = Version::new(part(0), part(1), part(2));
        if self.parts.len() > 3 {
            let extra = self.parts[3..]
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(".");
            version.pre =
                Prerelease::new(&extra).expect("numeric identifiers are valid pre-release text");
        }
        version
    }

    /// The version text exactly as it was written or published.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn parts(&self) -> &[u64] {
        &self.parts
    }
}

impl PartialEq for Ver {
    fn eq(&self, other: &Self) -> bool {
        self.parts == other.parts
    }
}

impl Eq for Ver {}

impl PartialOrd for Ver {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Ver {
    fn cmp(&self, other: &Self) -> Ordering {
        self.parts.cmp(&other.parts)
    }
}

impl Hash for Ver {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.parts.hash(state);
    }
}

impl fmt::Display for Ver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.raw)
    }
}

/// A comparison in a version requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Lt,
    Le,
    Eq,
    Ge,
    Gt,
}

impl Op {
    fn as_str(self) -> &'static str {
        match self {
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Eq => "==",
            Op::Ge => ">=",
            Op::Gt => ">",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Bound {
    op: Op,
    ver: Ver,
}

impl Bound {
    fn matches(&self, candidate: &Ver) -> bool {
        match self.op {
            Op::Lt => candidate < &self.ver,
            Op::Le => candidate <= &self.ver,
            Op::Eq => candidate == &self.ver,
            Op::Ge => candidate >= &self.ver,
            Op::Gt => candidate > &self.ver,
        }
    }
}

/// A version requirement: every bound must hold. The empty requirement
/// accepts any version.
///
/// Accepted syntax, comma separated:
/// - `*` or an empty string: any version
/// - `>=`, `>`, `<=`, `<`, `==` or `=` followed by a version, exactly the
///   operators R writes in `DESCRIPTION` (`>= 0.3-1`, `>= 1.2.3.4`)
/// - `^1.2.3` and `~1.2.3` with Cargo's rules, so existing manifests keep
///   their meaning
/// - a bare version such as `1.2.3`, read as `^1.2.3` (as `semver` does)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VerReq {
    bounds: Vec<Bound>,
}

impl VerReq {
    pub fn any() -> Self {
        Self::default()
    }

    /// Exactly `version` and nothing else.
    pub fn exactly(version: &Ver) -> Self {
        Self { bounds: vec![Bound { op: Op::Eq, ver: version.clone() }] }
    }

    pub fn parse(spec: &str) -> Result<Self> {
        let trimmed = spec.trim();
        let mut bounds = Vec::new();
        for piece in trimmed.split(',').map(str::trim) {
            if piece.is_empty() || piece == "*" {
                continue;
            }
            bounds.extend(
                parse_piece(piece)
                    .with_context(|| format!("Invalid version requirement {trimmed:?}"))?,
            );
        }
        Ok(Self { bounds })
    }

    pub fn matches(&self, candidate: &Ver) -> bool {
        self.bounds.iter().all(|bound| bound.matches(candidate))
    }

    /// The comparisons that must all hold. Empty for a requirement that
    /// accepts any version.
    pub fn constraints(&self) -> impl Iterator<Item = (Op, &Ver)> {
        self.bounds.iter().map(|bound| (bound.op, &bound.ver))
    }

    /// The highest candidate that satisfies the requirement. The order of
    /// `candidates` does not matter.
    pub fn highest<'a>(&self, candidates: &'a [Ver]) -> Option<&'a Ver> {
        candidates.iter().filter(|candidate| self.matches(candidate)).max()
    }

    /// Both requirements at once.
    pub fn and(&self, other: &VerReq) -> VerReq {
        let mut bounds = self.bounds.clone();
        for bound in &other.bounds {
            if !bounds.contains(bound) {
                bounds.push(bound.clone());
            }
        }
        VerReq { bounds }
    }
}

impl fmt::Display for VerReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.bounds.is_empty() {
            return write!(f, "*");
        }
        let text = self
            .bounds
            .iter()
            .map(|bound| format!("{} {}", bound.op.as_str(), bound.ver))
            .collect::<Vec<_>>()
            .join(", ");
        write!(f, "{text}")
    }
}

fn parse_piece(piece: &str) -> Result<Vec<Bound>> {
    let operators = [">=", "<=", "==", ">", "<", "=", "^", "~"];
    let (symbol, rest) = match operators.iter().find(|op| piece.starts_with(**op)) {
        Some(op) => (*op, piece[op.len()..].trim()),
        None => ("^", piece),
    };
    if rest.is_empty() {
        bail!("{piece:?} has an operator but no version");
    }
    let ver = Ver::parse(rest)?;
    let bound = |op: Op, ver: Ver| Bound { op, ver };
    Ok(match symbol {
        ">=" => vec![bound(Op::Ge, ver)],
        ">" => vec![bound(Op::Gt, ver)],
        "<=" => vec![bound(Op::Le, ver)],
        "<" => vec![bound(Op::Lt, ver)],
        "==" | "=" => vec![bound(Op::Eq, ver)],
        "~" => {
            let upper = tilde_upper(ver.parts());
            vec![bound(Op::Ge, ver), bound(Op::Lt, Ver::from_parts(upper))]
        }
        _ => {
            let upper = caret_upper(ver.parts());
            vec![bound(Op::Ge, ver), bound(Op::Lt, Ver::from_parts(upper))]
        }
    })
}

/// Exclusive upper bound of `^parts`: bump the first non-zero component,
/// or the last one when all are zero. `^1.2.3` is below `2`, `^0.2.3`
/// below `0.3`, `^0.0.3` below `0.0.4`.
fn caret_upper(parts: &[u64]) -> Vec<u64> {
    let index = parts
        .iter()
        .position(|part| *part != 0)
        .unwrap_or(parts.len() - 1);
    let mut upper = parts[..index].to_vec();
    upper.push(parts[index] + 1);
    upper
}

/// Exclusive upper bound of `~parts`: `~1.2.3` is below `1.3`, `~1` is
/// below `2`.
fn tilde_upper(parts: &[u64]) -> Vec<u64> {
    if parts.len() == 1 {
        return vec![parts[0] + 1];
    }
    vec![parts[0], parts[1] + 1]
}
