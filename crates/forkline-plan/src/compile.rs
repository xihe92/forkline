//! Checks that need the registry, and compilation into an executable plan.

use std::any::TypeId;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use forkline_core::{NodeImpl, NodeKind, Registry, Slot};

use crate::config::{OnConflict, PlanConfig, PlansFile};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::graph::{PlanGraph, check_structure};

/// Index of a node within its [`CompiledPlan`].
pub type NodeId = usize;

/// One interned key.
#[derive(Clone, Debug)]
pub struct KeyInfo {
    /// Plan-level key name (after renames).
    pub name: Arc<str>,
    /// Value type.
    pub type_id: TypeId,
    /// Value type name, for messages.
    pub type_name: &'static str,
}

/// The plan's keys, interned to slots.
#[derive(Clone, Debug, Default)]
pub struct KeyTable {
    keys: Vec<KeyInfo>,
    index: HashMap<Arc<str>, Slot>,
}

impl KeyTable {
    /// Number of slots a request map for this plan needs.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// True when the plan uses no keys.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Slot of a key, by plan-level name.
    pub fn slot(&self, name: &str) -> Option<Slot> {
        self.index.get(name).copied()
    }

    /// Key information for a slot.
    pub fn info(&self, slot: Slot) -> Option<&KeyInfo> {
        self.keys.get(slot as usize)
    }

    /// All keys, in slot order.
    pub fn iter(&self) -> impl Iterator<Item = &KeyInfo> {
        self.keys.iter()
    }

    fn intern(&mut self, name: &str, type_id: TypeId, type_name: &'static str) -> Result<Slot, &KeyInfo> {
        if let Some(&slot) = self.index.get(name) {
            return match self.keys.get(slot as usize) {
                Some(info) if info.type_id == type_id => Ok(slot),
                Some(info) => Err(info),
                None => Ok(slot),
            };
        }
        let slot = Slot::try_from(self.keys.len()).unwrap_or(Slot::MAX);
        let name: Arc<str> = Arc::from(name);
        self.keys.push(KeyInfo { name: name.clone(), type_id, type_name });
        self.index.insert(name, slot);
        Ok(slot)
    }
}

/// An outgoing edge of a compiled node.
#[derive(Clone, Debug)]
pub struct Edge {
    /// Target node.
    pub to: NodeId,
    /// Branch label, for edges out of a branch.
    pub label: Option<Arc<str>>,
}

/// One node, built and bound to its slots.
#[derive(Clone)]
pub struct CompiledNode {
    /// Node name.
    pub name: Arc<str>,
    /// Registered module name.
    pub module: Arc<str>,
    /// The built module.
    pub imp: NodeImpl,
    /// Slot for each entry of the module's contract, in contract order.
    pub slots: Box<[Slot]>,
    /// Outgoing edges, in link order.
    pub children: Vec<Edge>,
    /// Incoming edges, in link order.
    pub parents: Vec<NodeId>,
    /// Run this fork's paths on separate tasks.
    pub spawn: bool,
}

impl CompiledNode {
    /// The node kind.
    pub fn kind(&self) -> NodeKind {
        self.imp.kind()
    }
}

impl fmt::Debug for CompiledNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledNode")
            .field("name", &self.name)
            .field("module", &self.module)
            .field("kind", &self.kind())
            .field("slots", &self.slots)
            .field("children", &self.children)
            .field("spawn", &self.spawn)
            .finish()
    }
}

/// A validated plan, ready to execute.
#[derive(Clone, Debug)]
pub struct CompiledPlan {
    /// Plan name.
    pub name: Arc<str>,
    /// Root (decode) node.
    pub root: NodeId,
    /// Terminal node.
    pub terminal: NodeId,
    /// Nodes, indexed by [`NodeId`].
    pub nodes: Vec<CompiledNode>,
    /// Interned keys.
    pub keys: KeyTable,
    /// Per-request time budget.
    pub time_budget: Duration,
    /// Merge-conflict policy.
    pub on_conflict: OnConflict,
}

impl CompiledPlan {
    /// A node by id.
    pub fn node(&self, id: NodeId) -> Option<&CompiledNode> {
        self.nodes.get(id)
    }
}

/// Compile every plan in a file. Fails with every problem from every plan.
pub fn compile_all(file: &PlansFile, registry: &Registry) -> Result<BTreeMap<String, Arc<CompiledPlan>>, Diagnostics> {
    let mut plans = BTreeMap::new();
    let mut diags = Vec::new();
    for (name, plan) in &file.plans {
        match compile(name, plan, registry) {
            Ok(p) => {
                plans.insert(name.clone(), Arc::new(p));
            }
            Err(d) => diags.extend(d.0),
        }
    }
    if diags.is_empty() { Ok(plans) } else { Err(Diagnostics(diags)) }
}

/// Validate one plan against the registry and build its modules.
pub fn compile(name: &str, plan: &PlanConfig, registry: &Registry) -> Result<CompiledPlan, Diagnostics> {
    let (graph, mut diags) = check_structure(name, plan);

    // Every module exists.
    let mut kinds: Vec<Option<NodeKind>> = Vec::with_capacity(plan.nodes.len());
    for (node, cfg) in &plan.nodes {
        match registry.get(&cfg.module) {
            Some(entry) => kinds.push(Some(entry.kind())),
            None => {
                diags.push(Diagnostic::node(name, node, format!("module `{}` is not registered", cfg.module)));
                kinds.push(None);
            }
        }
    }

    if let Some(graph) = &graph {
        check_positions(name, plan, graph, &kinds, &mut diags);
    }

    // Keys: renames, interning and type agreement.
    let mut keys = KeyTable::default();
    let mut bindings: Vec<Box<[Slot]>> = Vec::with_capacity(plan.nodes.len());
    for (node, cfg) in &plan.nodes {
        let Some(entry) = registry.get(&cfg.module) else {
            bindings.push(Box::new([]));
            continue;
        };
        let contract = entry.contract();
        for local in cfg.rename.keys() {
            if !contract.iter().any(|k| k.name == local) {
                diags.push(Diagnostic::node(
                    name,
                    node,
                    format!("rename of `{local}`: module `{}` has no key of that name", cfg.module),
                ));
            }
        }
        let mut slots = Vec::with_capacity(contract.len());
        let mut globals: BTreeMap<&str, &str> = BTreeMap::new();
        for spec in contract {
            let global = cfg.rename.get(spec.name).map_or(spec.name, String::as_str);
            if let Some(other) = globals.insert(global, spec.name) {
                diags.push(Diagnostic::node(
                    name,
                    node,
                    format!("keys `{other}` and `{}` both map to plan key `{global}`", spec.name),
                ));
            }
            match keys.intern(global, (spec.type_id)(), (spec.type_name)()) {
                Ok(slot) => slots.push(slot),
                Err(existing) => {
                    diags.push(Diagnostic::node(
                        name,
                        node,
                        format!(
                            "key `{global}` is `{}` here but `{}` elsewhere in the plan",
                            (spec.type_name)(),
                            existing.type_name
                        ),
                    ));
                    slots.push(Slot::MAX);
                }
            }
        }
        bindings.push(slots.into_boxed_slice());
    }

    // Build modules from their config.
    let mut imps: Vec<Option<NodeImpl>> = Vec::with_capacity(plan.nodes.len());
    for (node, cfg) in &plan.nodes {
        let built = registry.get(&cfg.module).and_then(|entry| match entry.build(cfg.config.as_ref()) {
            Ok(imp) => Some(imp),
            Err(e) => {
                diags.push(Diagnostic::node(name, node, format!("module `{}`: {e}", cfg.module)));
                None
            }
        });
        imps.push(built);
    }

    let graph = match graph {
        Some(g) if diags.is_empty() => g,
        _ => return Err(Diagnostics(diags)),
    };

    let mut nodes = Vec::with_capacity(plan.nodes.len());
    for (i, ((node, cfg), (imp, slots))) in plan.nodes.iter().zip(imps.into_iter().zip(bindings)).enumerate() {
        let Some(imp) = imp else {
            return Err(Diagnostics(vec![Diagnostic::node(name, node, "module was not built")]));
        };
        nodes.push(CompiledNode {
            name: Arc::from(node.as_str()),
            module: Arc::from(cfg.module.as_str()),
            imp,
            slots,
            children: graph.children[i]
                .iter()
                .map(|(to, label)| Edge { to: *to, label: label.as_deref().map(Arc::from) })
                .collect(),
            parents: graph.parents[i].clone(),
            spawn: cfg.spawn,
        });
    }

    Ok(CompiledPlan {
        name: Arc::from(name),
        root: graph.root,
        terminal: graph.terminal,
        nodes,
        keys,
        time_budget: Duration::from_millis(plan.time_budget_ms),
        on_conflict: plan.on_conflict,
    })
}

/// Each node's kind must fit where it sits in the graph.
fn check_positions(
    name: &str,
    plan: &PlanConfig,
    graph: &PlanGraph,
    kinds: &[Option<NodeKind>],
    diags: &mut Vec<Diagnostic>,
) {
    let mut encoders = Vec::new();
    for (i, (node, cfg)) in plan.nodes.iter().enumerate() {
        let Some(kind) = kinds.get(i).copied().flatten() else {
            continue;
        };
        let children = &graph.children[i];
        let parents = &graph.parents[i];
        let labelled = children.iter().any(|(_, l)| l.is_some());
        let module = &cfg.module;

        if i == graph.root && kind != NodeKind::Decode {
            diags.push(Diagnostic::node(
                name,
                node,
                format!("the root must be a decode module, `{module}` is a {kind}"),
            ));
        }
        if i != graph.root && kind == NodeKind::Decode {
            diags.push(Diagnostic::node(name, node, "a decode module can only be the root"));
        }
        if i == graph.terminal && kind != NodeKind::Terminal {
            diags.push(Diagnostic::node(
                name,
                node,
                format!("the terminal must be a terminal module, `{module}` is a {kind}"),
            ));
        }
        if i != graph.terminal && kind == NodeKind::Terminal {
            diags.push(Diagnostic::node(name, node, "a terminal module can only be the plan's terminal"));
        }
        if labelled && !kind.is_branching() {
            diags.push(Diagnostic::node(
                name,
                node,
                format!("has labelled links, so it must be a branch or multibranch module, `{module}` is a {kind}"),
            ));
        }
        if kind.is_branching() {
            if children.is_empty() {
                diags.push(Diagnostic::node(name, node, "a branch needs at least one labelled link"));
            } else if !labelled {
                diags.push(Diagnostic::node(name, node, "links out of a branch must carry labels"));
            }
        }
        if parents.len() >= 2 && kind != NodeKind::Join {
            diags.push(Diagnostic::node(
                name,
                node,
                format!("has {} incoming links, so it must be a join module, `{module}` is a {kind}", parents.len()),
            ));
        }
        if kind == NodeKind::Join && parents.len() < 2 {
            diags.push(Diagnostic::node(name, node, "a join needs at least two incoming links"));
        }
        if kind == NodeKind::Encode {
            encoders.push(node.as_str());
            if !children.is_empty() {
                diags.push(Diagnostic::node(
                    name,
                    node,
                    "an encode module ends the plan and cannot have outgoing links",
                ));
            }
        }
        if cfg.spawn && !graph.is_fork(i, kind.is_branching()) {
            diags.push(Diagnostic::node(
                name,
                node,
                "`spawn = true` only applies to a fork (a non-branch node with two or more children)",
            ));
        }
    }
    match encoders.len() {
        0 => diags.push(Diagnostic::plan(name, "the plan has no encode node")),
        1 => {}
        _ => diags
            .push(Diagnostic::plan(name, format!("the plan has more than one encode node: {}", encoders.join(", ")))),
    }
}
