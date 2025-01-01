//! Generate a CallGraph for instances in each crate.
//! You can roughly think of instances as a monomorphic function.
//! If an instance calls another instance, then we have an edge
//! from caller to callee with callsite locations as edge weight.
//! This is a fundamental analysis for other analysis,
//! e.g., points-to analysis, lockguard collector, etc.
//! We also track where a closure is defined rather than called
//! to record the defined function and the parameter of the closure,
//! which is pointed to by upvars.
use petgraph::algo;
use petgraph::dot::{Config, Dot};
use petgraph::graph::NodeIndex;
use petgraph::visit::IntoNodeReferences;
use petgraph::Direction::Incoming;
use petgraph::{Directed, Graph};

use stable_mir::mir::visit::{MirVisitor, Location};
use stable_mir::mir::{Body, Terminator, TerminatorKind};
use stable_mir::mir::mono::Instance;
use stable_mir::ty::{RigidTy, TyKind};

/// The NodeIndex in CallGraph, denoting a unique instance in CallGraph.
pub type InstanceId = NodeIndex;

/// The location where caller calls callee.
/// Support direct call for now, where callee resolves to FnDef.
/// Also support tracking the parameter of a closure (pointed to by upvars)
/// TODO(boqin): Add support for FnPtr.
#[derive(Copy, Clone, Debug)]
pub enum CallSiteLocation {
    Direct(Location),
    // ClosureDef(Local),
    Indirect(Location),
}

impl CallSiteLocation {
    pub fn location(&self) -> Option<Location> {
        match self {
            Self::Direct(loc) => Some(*loc),
            _ => None,
        }
    }
}

/// The CallGraph node wrapping an Instance.
/// WithBody means the Instance owns body.
#[derive(Debug, PartialEq, Eq)]
pub enum CallGraphNode {
    WithBody(Instance),
    WithoutBody(Instance),
}

impl CallGraphNode {
    pub fn instance(&self) -> &Instance {
        match self {
            CallGraphNode::WithBody(inst) | CallGraphNode::WithoutBody(inst) => inst,
        }
    }

    pub fn match_instance(&self, other: &Instance) -> bool {
        matches!(self, CallGraphNode::WithBody(inst) | CallGraphNode::WithoutBody(inst) if inst == other)
    }
}

/// CallGraph
/// The nodes of CallGraph are instances.
/// The directed edges are CallSite Locations.
/// e.g., `Instance1--|[CallSite1, CallSite2]|-->Instance2`
/// denotes `Instance1` calls `Instance2` at locations `Callsite1` and `CallSite2`.
pub struct CallGraph {
    pub graph: Graph<CallGraphNode, Vec<CallSiteLocation>, Directed>,
}

impl CallGraph {
    /// Create an empty CallGraph.
    pub fn new() -> Self {
        Self {
            graph: Graph::new(),
        }
    }

    /// Search for the InstanceId of a given instance in CallGraph.
    pub fn instance_to_index(&self, instance: &Instance) -> Option<InstanceId> {
        self.graph
            .node_references()
            .find(|(_idx, inst)| inst.match_instance(instance))
            .map(|(idx, _)| idx)
    }

    /// Get the instance by InstanceId.
    pub fn index_to_instance(&self, idx: InstanceId) -> Option<&CallGraphNode> {
        self.graph.node_weight(idx)
    }

    /// Perform callgraph analysis on the given instances.
    /// The instances should be **all** the instances with MIR available in the current crate.
    pub fn analyze(
        &mut self,
        instances: Vec<Instance>,
    ) {
        let idx_insts = instances
            .into_iter()
            .map(|inst| {
                let idx = self.graph.add_node(CallGraphNode::WithBody(inst));
                (idx, inst)
            })
            .collect::<Vec<_>>();
        for (caller_idx, caller) in idx_insts {
            let body = &caller.body().unwrap();
            let mut collector = CallSiteCollector::new(caller, body);
            collector.visit_body(body);
            for (callee, location) in collector.finish() {
                let callee_idx = if let Some(callee_idx) = self.instance_to_index(&callee) {
                    callee_idx
                } else {
                    self.graph.add_node(CallGraphNode::WithoutBody(callee))
                };
                if let Some(edge_idx) = self.graph.find_edge(caller_idx, callee_idx) {
                    // Update edge weight.
                    self.graph.edge_weight_mut(edge_idx).unwrap().push(location);
                } else {
                    // Add edge if not exists.
                    self.graph.add_edge(caller_idx, callee_idx, vec![location]);
                }
            }
        }
    }

    /// Find the callsites (weight) on the edge from source to target.
    pub fn callsites(
        &self,
        source: InstanceId,
        target: InstanceId,
    ) -> Option<Vec<CallSiteLocation>> {
        let edge = self.graph.find_edge(source, target)?;
        self.graph.edge_weight(edge).cloned()
    }

    /// Find all the callers that call target
    pub fn callers(&self, target: InstanceId) -> Vec<InstanceId> {
        self.graph.neighbors_directed(target, Incoming).collect()
    }

    /// Find all simple paths from source to target.
    /// e.g., for one of the paths, `source --> instance1 --> instance2 --> target`,
    /// the return is [source, instance1, instance2, target].
    pub fn all_simple_paths(&self, source: InstanceId, target: InstanceId) -> Vec<Vec<InstanceId>> {
        algo::all_simple_paths::<Vec<_>, _>(&self.graph, source, target, 0, None)
            .collect::<Vec<_>>()
    }

    /// Print the callgraph in dot format.
    #[allow(dead_code)]
    pub fn dot(&self) {
        println!(
            "{:?}",
            Dot::with_config(&self.graph, &[Config::GraphContentOnly])
        );
    }
}

/// Visit Terminator and record callsites (callee + location).
struct CallSiteCollector<'a> {
    caller: Instance,
    body: &'a Body,
    callsites: Vec<(Instance, CallSiteLocation)>,
}

impl<'a> CallSiteCollector<'a> {
    fn new(
        caller: Instance,
        body: &'a Body,
    ) -> Self {
        Self {
            caller,
            body,
            callsites: Vec::new(),
        }
    }

    /// Consumes `CallSiteCollector` and returns its callsites when finished visiting.
    fn finish(self) -> impl IntoIterator<Item = (Instance, CallSiteLocation)> {
        self.callsites.into_iter()
    }
}

impl MirVisitor for CallSiteCollector<'_,> {
    /// Resolve direct call.
    /// Inspired by https://github.com/model-checking/kani/blob/33b74e03a1fed6b045088e428a5a0cc5fcfd70f6/kani-compiler/src/kani_middle/reachability.rs#L439.
    fn visit_terminator(&mut self, terminator: &Terminator, location: Location) {
        match terminator.kind {
            TerminatorKind::Call { ref func, .. } => {
                let func_ty = func.ty(self.body.locals()).unwrap();
                if let TyKind::RigidTy(RigidTy::FnDef(fn_def, args)) = func_ty.kind() {
                    let callee = Instance::resolve(fn_def, &args).unwrap();
                    self.callsites.push((callee, CallSiteLocation::Direct(location)));
                }
            }
            TerminatorKind::Drop { ref place, .. } => {
                let place_ty = place.ty(self.body.locals()).unwrap();
                let callee = Instance::resolve_drop_in_place(place_ty);
                self.callsites.push((callee, CallSiteLocation::Direct(location)));
            }
            _ => {}
        }
        self.super_terminator(terminator, location);
    }
}