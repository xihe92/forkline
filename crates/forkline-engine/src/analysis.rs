//! Graph facts the executor needs, computed once per plan load.

use forkline_core::{FixedBitSet, NodeKind};
use forkline_plan::{CompiledPlan, NodeId};
use petgraph::algo::{dominators, toposort};
use petgraph::graph::{DiGraph, NodeIndex};

/// A compiled plan plus the facts the executor looks up per request.
#[derive(Debug)]
pub struct ExecPlan {
    /// The validated plan.
    pub compiled: CompiledPlan,
    /// `reach[n]`: the nodes reachable from `n` (not including `n`).
    pub(crate) reach: Vec<FixedBitSet>,
    /// For each join, its owning divergence node (immediate dominator).
    pub(crate) owner: Vec<Option<NodeId>>,
    /// The encode node.
    pub(crate) encode: Option<NodeId>,
    /// Metric handles.
    pub(crate) metrics: std::sync::Arc<crate::metrics_handles::PlanMetrics>,
}

impl ExecPlan {
    /// Analyse a compiled plan. Validation guarantees the graph is a DAG
    /// rooted at `compiled.root`.
    pub fn new(compiled: CompiledPlan) -> Self {
        let n = compiled.nodes.len();
        let mut g = DiGraph::<(), ()>::with_capacity(n, n);
        let ids: Vec<NodeIndex> = (0..n).map(|_| g.add_node(())).collect();
        for (from, node) in compiled.nodes.iter().enumerate() {
            for edge in &node.children {
                if let (Some(&a), Some(&b)) = (ids.get(from), ids.get(edge.to)) {
                    g.add_edge(a, b, ());
                }
            }
        }

        let mut reach = vec![FixedBitSet::with_capacity(n); n];
        if let Ok(order) = toposort(&g, None) {
            for idx in order.into_iter().rev() {
                let i = idx.index();
                let mut bits = FixedBitSet::with_capacity(n);
                for edge in &compiled.nodes[i].children {
                    bits.insert(edge.to);
                    bits.union_with(&reach[edge.to]);
                }
                reach[i] = bits;
            }
        }

        let mut owner = vec![None; n];
        if let Some(&root) = ids.get(compiled.root) {
            let doms = dominators::simple_fast(&g, root);
            for (i, node) in compiled.nodes.iter().enumerate() {
                if node.kind() == NodeKind::Join {
                    owner[i] = doms.immediate_dominator(ids[i]).map(NodeIndex::index);
                }
            }
        }

        let encode = compiled.nodes.iter().position(|node| node.kind() == NodeKind::Encode);
        let names: Vec<&str> = compiled.nodes.iter().map(|n| &*n.name).collect();
        let metrics = std::sync::Arc::new(crate::metrics_handles::PlanMetrics::new(&compiled.name, &names));
        Self { compiled, reach, owner, encode, metrics }
    }

    /// True when `to` is reachable from `from`.
    pub(crate) fn reaches(&self, from: NodeId, to: NodeId) -> bool {
        self.reach.get(from).is_some_and(|bits| bits.contains(to))
    }

    /// True when a path at `node` can still produce the reply.
    pub(crate) fn leads_to_encode(&self, node: NodeId) -> bool {
        self.encode.is_some_and(|e| e == node || self.reaches(node, e))
    }
}
