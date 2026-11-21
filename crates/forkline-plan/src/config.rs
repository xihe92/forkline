//! The plans file format.
//!
//! ```toml
//! [plans.qp1]
//! root = "request"
//! terminal = "error"
//! time_budget_ms = 80
//! on_conflict = "error"
//! links = [
//!   "request -> time", "request -> branch",
//!   "branch -> horoscope : horoscope", "branch -> stock : stock",
//!   "time -> join", "horoscope -> join", "stock -> join",
//!   "join -> response",
//! ]
//!
//! [plans.qp1.nodes]
//! request   = { module = "RequestDecoder" }
//! time      = { module = "TimeLookup", rename = { svc_response = "time_response" } }
//! horoscope = { module = "HoroscopeLookup", config = { timeout_ms = 40 } }
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

/// A whole plans file.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlansFile {
    /// Plans by name.
    #[serde(default)]
    pub plans: BTreeMap<String, PlanConfig>,
}

/// A plans file could not be read or parsed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The file could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// File path.
        path: String,
        /// Underlying error.
        source: std::io::Error,
    },
    /// The TOML is malformed or does not match the format.
    #[error("{origin}: {message}")]
    Parse {
        /// File path, or `<string>`.
        origin: String,
        /// Parser message, with line and column.
        message: String,
    },
}

impl PlansFile {
    /// Parse a plans file from a string.
    pub fn from_toml_str(text: &str) -> Result<Self, ConfigError> {
        Self::parse(text, "<string>")
    }

    /// Read and parse a plans file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|source| ConfigError::Io { path: path.display().to_string(), source })?;
        Self::parse(&text, &path.display().to_string())
    }

    fn parse(text: &str, origin: &str) -> Result<Self, ConfigError> {
        toml::from_str(text).map_err(|e| ConfigError::Parse { origin: origin.to_owned(), message: e.to_string() })
    }
}

/// What to do when several paths change the same key and the join leaves it unresolved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnConflict {
    /// Drop the key from the merged map. Logged and counted.
    Drop,
    /// Fail the request with a merge-conflict error. Logged and counted.
    #[default]
    Error,
}

fn default_time_budget_ms() -> u64 {
    1000
}

/// One plan.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanConfig {
    /// The decode node every request starts at.
    pub root: String,
    /// The node that answers when the plan fails.
    pub terminal: String,
    /// Time budget per request, in milliseconds.
    #[serde(default = "default_time_budget_ms")]
    pub time_budget_ms: u64,
    /// Merge-conflict policy.
    #[serde(default)]
    pub on_conflict: OnConflict,
    /// Edges, as `"from -> to"` or `"from -> to : label"`.
    #[serde(default)]
    pub links: Vec<String>,
    /// Nodes by name.
    pub nodes: BTreeMap<String, NodeConfig>,
}

/// One node of a plan.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeConfig {
    /// Registered module name.
    pub module: String,
    /// Contract key renames, local name to plan key name.
    #[serde(default)]
    pub rename: BTreeMap<String, String>,
    /// The module's own config table.
    #[serde(default)]
    pub config: Option<toml::Value>,
    /// Run this fork's paths on separate tasks (for CPU-heavy branches).
    #[serde(default)]
    pub spawn: bool,
}

/// A parsed link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Source node.
    pub from: String,
    /// Target node.
    pub to: String,
    /// Branch label, for links out of a branch node.
    pub label: Option<String>,
}

fn is_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Parse `"from -> to"` or `"from -> to : label"`.
pub fn parse_link(text: &str) -> Result<Link, String> {
    let (from, rest) = text
        .split_once("->")
        .ok_or_else(|| format!("link `{text}` is not of the form `from -> to` or `from -> to : label`"))?;
    let (to, label) = match rest.split_once(':') {
        Some((to, label)) => (to, Some(label.trim())),
        None => (rest, None),
    };
    let (from, to) = (from.trim(), to.trim());
    for part in [from, to].into_iter().chain(label) {
        if !is_name(part) {
            return Err(format!("link `{text}`: `{part}` is not a valid name (letters, digits, `_` and `-`)"));
        }
    }
    Ok(Link { from: from.to_owned(), to: to.to_owned(), label: label.map(str::to_owned) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_parse() {
        assert_eq!(parse_link("a -> b").unwrap(), Link { from: "a".into(), to: "b".into(), label: None });
        assert_eq!(
            parse_link(" branch->stock : stock ").unwrap(),
            Link { from: "branch".into(), to: "stock".into(), label: Some("stock".into()) }
        );
        assert!(parse_link("a b").is_err());
        assert!(parse_link("a -> ").is_err());
        assert!(parse_link("a -> b : ").is_err());
        assert!(parse_link("a -> b c").is_err());
    }

    #[test]
    fn plans_file_parses_the_documented_example() {
        let text = r#"
            [plans.qp1]
            root = "request"
            terminal = "error"
            time_budget_ms = 80
            on_conflict = "drop"
            links = ["request -> response"]

            [plans.qp1.nodes]
            request  = { module = "RequestDecoder" }
            time     = { module = "TimeLookup", rename = { svc_response = "time_response" } }
            horo     = { module = "HoroscopeLookup", config = { timeout_ms = 40 } }
            response = { module = "ResponseEncoder" }
            error    = { module = "ErrorEncoder" }
        "#;
        let file = PlansFile::from_toml_str(text).unwrap();
        let plan = &file.plans["qp1"];
        assert_eq!(plan.time_budget_ms, 80);
        assert_eq!(plan.on_conflict, OnConflict::Drop);
        assert_eq!(plan.nodes["time"].rename["svc_response"], "time_response");
        assert!(plan.nodes["horo"].config.is_some());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let text = r#"
            [plans.p]
            root = "a"
            terminal = "e"
            colour = "blue"
            nodes = {}
        "#;
        let err = PlansFile::from_toml_str(text).unwrap_err().to_string();
        assert!(err.contains("colour"), "{err}");
    }
}
