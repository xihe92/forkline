//! Validation problems.

use std::fmt;

/// One problem found in a plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Plan name.
    pub plan: String,
    /// Node name, when the problem belongs to one node.
    pub node: Option<String>,
    /// What is wrong.
    pub message: String,
}

impl Diagnostic {
    pub(crate) fn plan(plan: &str, message: impl Into<String>) -> Self {
        Self { plan: plan.to_owned(), node: None, message: message.into() }
    }

    pub(crate) fn node(plan: &str, node: &str, message: impl Into<String>) -> Self {
        Self { plan: plan.to_owned(), node: Some(node.to_owned()), message: message.into() }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.node {
            Some(node) => write!(f, "plan `{}`, node `{}`: {}", self.plan, node, self.message),
            None => write!(f, "plan `{}`: {}", self.plan, self.message),
        }
    }
}

/// Every problem found in one load. Never empty when returned as an error.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diagnostics(pub Vec<Diagnostic>);

impl Diagnostics {
    /// True when no problems were found.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Number of problems.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Iterate over the problems.
    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter()
    }
}

impl fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, d) in self.0.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{d}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Diagnostics {}

impl From<Vec<Diagnostic>> for Diagnostics {
    fn from(v: Vec<Diagnostic>) -> Self {
        Diagnostics(v)
    }
}
