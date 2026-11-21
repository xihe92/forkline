//! Graph rules that need no module information.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use petgraph::algo::is_cyclic_directed;
use petgraph::graph::DiGraph;

use crate::config::{PlanConfig, parse_link};
use crate::diagnostic::Diagnostic;

/// A plan's nodes and edges, indexed. Node indexes follow sorted node names.
#[derive(Clone, Debug)]
pub struct PlanGraph {
    /// Node names, sorted; a node's index is its position here.
    pub names: Vec<String>,
    /// Outgoing edges per node: target index and optional label, in link order.
    pub children: Vec<Vec<(usize, Option<String>)>>,
    /// Incoming edges per node: source indexes, in link order.
    pub parents: Vec<Vec<usize>>,
    /// Index of the root node.
    pub root: usize,
    /// Index of the terminal node.
    pub terminal: usize,
}

impl PlanGraph {
    /// Index of a node by name.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.names.binary_search_by(|n| n.as_str().cmp(name)).ok()
    }

    /// True when the node has two or more children and is not a branch.
    /// (Whether it is a branch depends on its module, so callers pass that in.)
    pub fn is_fork(&self, node: usize, is_branch: bool) -> bool {
        !is_branch && self.children.get(node).is_some_and(|c| c.len() >= 2)
    }
}

/// Check a plan's graph rules without any module information.
///
/// Returns the indexed graph when it is well-formed enough to analyse
/// further (every link names existing nodes, root and terminal exist), plus
/// every problem found.
pub fn check_structure(name: &str, plan: &PlanConfig) -> (Option<PlanGraph>, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    if plan.time_budget_ms == 0 {
        diags.push(Diagnostic::plan(name, "time_budget_ms must be greater than 0"));
    }

    let names: Vec<String> = plan.nodes.keys().cloned().collect();
    let index: BTreeMap<&str, usize> = names.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();

    let root = index.get(plan.root.as_str()).copied();
    let terminal = index.get(plan.terminal.as_str()).copied();
    if root.is_none() {
        diags.push(Diagnostic::plan(name, format!("root `{}` is not a node of this plan", plan.root)));
    }
    if terminal.is_none() {
        diags.push(Diagnostic::plan(name, format!("terminal `{}` is not a node of this plan", plan.terminal)));
    }
    if plan.root == plan.terminal {
        diags.push(Diagnostic::plan(name, "root and terminal must be different nodes"));
    }

    let mut children: Vec<Vec<(usize, Option<String>)>> = vec![Vec::new(); names.len()];
    let mut parents: Vec<Vec<usize>> = vec![Vec::new(); names.len()];
    let mut seen_edges = BTreeSet::new();
    let mut links_ok = true;
    for text in &plan.links {
        let link = match parse_link(text) {
            Ok(link) => link,
            Err(message) => {
                diags.push(Diagnostic::plan(name, message));
                links_ok = false;
                continue;
            }
        };
        let (Some(&from), Some(&to)) = (index.get(link.from.as_str()), index.get(link.to.as_str())) else {
            for end in [&link.from, &link.to] {
                if !index.contains_key(end.as_str()) {
                    diags.push(Diagnostic::plan(name, format!("link `{text}` names unknown node `{end}`")));
                }
            }
            links_ok = false;
            continue;
        };
        if from == to {
            diags.push(Diagnostic::node(name, &link.from, "links to itself"));
            links_ok = false;
            continue;
        }
        if !seen_edges.insert((from, to)) {
            diags.push(Diagnostic::plan(name, format!("link `{text}` is listed twice")));
            continue;
        }
        children[from].push((to, link.label));
        parents[to].push(from);
    }

    let (Some(root), Some(terminal)) = (root, terminal) else {
        return (None, diags);
    };
    if !links_ok {
        return (None, diags);
    }

    // The terminal is reached only on failure, never through a link.
    if !children[terminal].is_empty() || !parents[terminal].is_empty() {
        diags.push(Diagnostic::node(name, &names[terminal], "the terminal node cannot have links"));
    }
    if !parents[root].is_empty() {
        diags.push(Diagnostic::node(name, &names[root], "the root node cannot have incoming links"));
    }

    // Labels: all or none on a node's outgoing links, and unique.
    for (i, edges) in children.iter().enumerate() {
        let labelled = edges.iter().filter(|(_, l)| l.is_some()).count();
        if labelled != 0 && labelled != edges.len() {
            diags.push(Diagnostic::node(
                name,
                &names[i],
                "either every outgoing link carries a branch label or none does",
            ));
        }
        let mut labels = BTreeSet::new();
        for label in edges.iter().filter_map(|(_, l)| l.as_deref()) {
            if !labels.insert(label) {
                diags.push(Diagnostic::node(name, &names[i], format!("branch label `{label}` is used twice")));
            }
        }
    }

    // No cycles.
    let mut g = DiGraph::<(), ()>::with_capacity(names.len(), plan.links.len());
    let ids: Vec<_> = (0..names.len()).map(|_| g.add_node(())).collect();
    for (from, edges) in children.iter().enumerate() {
        for (to, _) in edges {
            if let (Some(&a), Some(&b)) = (ids.get(from), ids.get(*to)) {
                g.add_edge(a, b, ());
            }
        }
    }
    if is_cyclic_directed(&g) {
        diags.push(Diagnostic::plan(name, "the links form a cycle"));
    }

    // Every node except the terminal is reachable from the root.
    let mut reached = vec![false; names.len()];
    let mut queue = VecDeque::from([root]);
    reached[root] = true;
    while let Some(n) = queue.pop_front() {
        for (c, _) in &children[n] {
            if !reached[*c] {
                reached[*c] = true;
                queue.push_back(*c);
            }
        }
    }
    for (i, r) in reached.iter().enumerate() {
        if !r && i != terminal {
            diags.push(Diagnostic::node(name, &names[i], "is not reachable from the root"));
        }
    }

    (Some(PlanGraph { names, children, parents, root, terminal }), diags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PlansFile;

    fn check(text: &str) -> Vec<String> {
        let file = PlansFile::from_toml_str(text).unwrap();
        let (name, plan) = file.plans.iter().next().unwrap();
        check_structure(name, plan).1.iter().map(ToString::to_string).collect()
    }

    const NODES: &str = r#"
        [plans.p.nodes]
        a = { module = "A" }
        b = { module = "B" }
        c = { module = "C" }
        e = { module = "E" }
    "#;

    fn plan(links: &str) -> String {
        format!("[plans.p]\nroot = \"a\"\nterminal = \"e\"\nlinks = [{links}]\n{NODES}")
    }

    #[test]
    fn a_chain_is_clean() {
        assert_eq!(check(&plan(r#""a -> b", "b -> c""#)), Vec::<String>::new());
    }

    #[test]
    fn cycles_are_rejected() {
        let d = check(&plan(r#""a -> b", "b -> c", "c -> b""#));
        assert!(d.iter().any(|m| m.contains("cycle")), "{d:?}");
    }

    #[test]
    fn unreachable_nodes_are_rejected() {
        let d = check(&plan(r#""a -> b""#));
        assert_eq!(d, vec!["plan `p`, node `c`: is not reachable from the root"]);
    }

    #[test]
    fn terminal_and_root_links_are_rejected() {
        let d = check(&plan(r#""a -> b", "b -> c", "c -> e", "b -> a""#));
        assert!(d.iter().any(|m| m.contains("terminal node cannot have links")), "{d:?}");
        assert!(d.iter().any(|m| m.contains("root node cannot have incoming")), "{d:?}");
    }

    #[test]
    fn labels_must_be_all_or_none_and_unique() {
        let d = check(&plan(r#""a -> b : x", "a -> c""#));
        assert!(d.iter().any(|m| m.contains("every outgoing link")), "{d:?}");
        let d = check(&plan(r#""a -> b : x", "a -> c : x""#));
        assert!(d.iter().any(|m| m.contains("used twice")), "{d:?}");
    }

    #[test]
    fn unknown_nodes_and_bad_links_are_reported_together() {
        let d = check(&plan(r#""a -> zz", "a b", "a -> a""#));
        assert_eq!(d.len(), 3, "{d:?}");
    }

    #[test]
    fn missing_root_and_terminal() {
        let text = "[plans.p]\nroot = \"x\"\nterminal = \"y\"\n[plans.p.nodes]\na = { module = \"A\" }\n";
        let d = check(text);
        assert_eq!(d.len(), 2, "{d:?}");
    }
}
