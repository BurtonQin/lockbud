//! Generate a CallGraph for instances in each crate.
//! You can roughly think of instances as a monomorphic function.
//! If an instance calls another instance, then we have an edge
//! from caller to callee with callsite locations as edge weight.
//! This is a fundamental analysis for other analysis,
//! e.g., points-to analysis, lockguard collector, etc.
//! We also track where a closure is defined rather than called
//! to record the defined function and the parameter of the closure,
//! which is pointed to by upvars.
extern crate rustc_data_structures;

use petgraph::algo;
use petgraph::dot::{Config, Dot};
use petgraph::graph::NodeIndex;
use petgraph::visit::IntoNodeReferences;
use petgraph::Direction::Incoming;
use petgraph::{Directed, Graph};

use rustc_data_structures::fx::FxHashSet;
use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{Body, Local, LocalDecl, LocalKind, Location, Terminator, TerminatorKind};
use rustc_middle::ty::{self, EarlyBinder, Instance, TyCtxt, TyKind, TypingEnv};

/// Whether the callee spawns its closure arguments on another thread, in
/// which case guards live in the caller are not held by the closure.
fn is_spawn_like_callee(path: &str) -> bool {
    let thread_spawn = path.contains("thread::")
        && (path.ends_with("::spawn")
            || path.contains("::spawn_unchecked")
            || path.contains("::spawn_scoped"));
    let rayon = path.contains("rayon::")
        && (path.ends_with("::spawn")
            || path.ends_with("::spawn_broadcast")
            || path.ends_with("::join"));
    let async_spawn = path.starts_with("tokio::spawn") || path.contains("task::spawn");
    let crossbeam = path.contains("crossbeam::") && path.ends_with("::spawn");
    thread_spawn || rayon || async_spawn || crossbeam
}

/// The NodeIndex in CallGraph, denoting a unique instance in CallGraph.
pub type InstanceId = NodeIndex;

/// The location where caller calls callee.
/// Support direct call for now, where callee resolves to FnDef.
/// Also support tracking the parameter of a closure (pointed to by upvars)
/// TODO(boqin): Add support for FnPtr.
#[derive(Copy, Clone, Debug)]
pub enum CallSiteLocation {
    Direct(Location),
    ClosureDef(Local, Option<Location>),
    // Indirect(Location),
}

impl CallSiteLocation {
    pub fn location(&self) -> Option<Location> {
        match self {
            Self::Direct(loc) | Self::ClosureDef(_, Some(loc)) => Some(*loc),
            _ => None,
        }
    }
}

/// The CallGraph node wrapping an Instance.
/// WithBody means the Instance owns body.
#[derive(Debug, PartialEq, Eq)]
pub enum CallGraphNode<'tcx> {
    WithBody(Instance<'tcx>),
    WithoutBody(Instance<'tcx>),
}

impl<'tcx> CallGraphNode<'tcx> {
    pub fn instance(&self) -> &Instance<'tcx> {
        match self {
            CallGraphNode::WithBody(inst) | CallGraphNode::WithoutBody(inst) => inst,
        }
    }

    pub fn match_instance(&self, other: &Instance<'tcx>) -> bool {
        matches!(self, CallGraphNode::WithBody(inst) | CallGraphNode::WithoutBody(inst) if inst == other)
    }
}

/// CallGraph
/// The nodes of CallGraph are instances.
/// The directed edges are CallSite Locations.
/// e.g., `Instance1--|[CallSite1, CallSite2]|-->Instance2`
/// denotes `Instance1` calls `Instance2` at locations `Callsite1` and `CallSite2`.
pub struct CallGraph<'tcx> {
    pub graph: Graph<CallGraphNode<'tcx>, Vec<CallSiteLocation>, Directed>,
    /// Closures passed to spawn-like APIs: they run on another thread, so
    /// guards live in the defining function are not held inside them.
    pub spawned_closures: FxHashSet<InstanceId>,
    /// Instances of spawn-like APIs: caller state must not flow into them,
    /// otherwise it leaks through the std spawn machinery into the spawned
    /// closure's context.
    pub spawn_like_callees: FxHashSet<InstanceId>,
}

impl<'tcx> CallGraph<'tcx> {
    /// Create an empty CallGraph.
    pub fn new() -> Self {
        Self {
            graph: Graph::new(),
            spawned_closures: FxHashSet::default(),
            spawn_like_callees: FxHashSet::default(),
        }
    }

    /// Whether the closure instance runs on a thread of its own.
    pub fn is_spawned_closure(&self, id: InstanceId) -> bool {
        self.spawned_closures.contains(&id)
    }

    /// Whether the instance is a spawn-like API whose callers' guards are
    /// not held by it.
    pub fn is_spawn_like(&self, id: InstanceId) -> bool {
        self.spawn_like_callees.contains(&id)
    }

    /// Search for the InstanceId of a given instance in CallGraph.
    pub fn instance_to_index(&self, instance: &Instance<'tcx>) -> Option<InstanceId> {
        self.graph
            .node_references()
            .find(|(_idx, inst)| inst.match_instance(instance))
            .map(|(idx, _)| idx)
    }

    /// Get the instance by InstanceId.
    pub fn index_to_instance(&self, idx: InstanceId) -> Option<&CallGraphNode<'tcx>> {
        self.graph.node_weight(idx)
    }

    /// Perform callgraph analysis on the given instances.
    /// The instances should be **all** the instances with MIR available in the current crate.
    pub fn analyze(
        &mut self,
        instances: Vec<Instance<'tcx>>,
        tcx: TyCtxt<'tcx>,
        typing_env: TypingEnv<'tcx>,
    ) {
        let idx_insts = instances
            .into_iter()
            .map(|inst| {
                let idx = self.graph.add_node(CallGraphNode::WithBody(inst));
                (idx, inst)
            })
            .collect::<Vec<_>>();
        for (caller_idx, caller) in idx_insts {
            let body = tcx.instance_mir(caller.def);
            // Skip promoted src
            if body.source.promoted.is_some() {
                continue;
            }
            let mut collector = CallSiteCollector::new(caller, body, tcx, typing_env);
            collector.visit_body(body);
            let (callsites, spawned_closure_instances) = collector.finish();
            if !spawned_closure_instances.is_empty() {
                eprintln!(
                    "[probe-sp] caller {} spawned closures: {:?}",
                    tcx.def_path_str(caller.def_id()),
                    spawned_closure_instances
                        .iter()
                        .map(|i| tcx.def_path_str(i.def_id()))
                        .collect::<Vec<_>>()
                );
            }
            for spawned in spawned_closure_instances {
                // Closure instances are WithBody nodes added up front, so the
                // lookup always succeeds here.
                if let Some(idx) = self.instance_to_index(&spawned) {
                    self.spawned_closures.insert(idx);
                }
            }
            for (callee, location) in callsites {
                let callee_idx = if let Some(callee_idx) = self.instance_to_index(&callee) {
                    callee_idx
                } else {
                    self.graph.add_node(CallGraphNode::WithoutBody(callee))
                };
                if is_spawn_like_callee(&tcx.def_path_str(callee.def_id())) {
                    self.spawn_like_callees.insert(callee_idx);
                }
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
struct CallSiteCollector<'a, 'tcx> {
    caller: Instance<'tcx>,
    body: &'a Body<'tcx>,
    tcx: TyCtxt<'tcx>,
    typing_env: TypingEnv<'tcx>,
    callsites: Vec<(Instance<'tcx>, CallSiteLocation)>,
    /// Closure locals passed as arguments to spawn-like calls.
    spawned_closure_locals: FxHashSet<Local>,
    /// Instances of the closures above.
    spawned_closure_instances: Vec<Instance<'tcx>>,
}

impl<'a, 'tcx> CallSiteCollector<'a, 'tcx> {
    fn new(
        caller: Instance<'tcx>,
        body: &'a Body<'tcx>,
        tcx: TyCtxt<'tcx>,
        typing_env: TypingEnv<'tcx>,
    ) -> Self {
        Self {
            caller,
            body,
            tcx,
            typing_env,
            callsites: Vec::new(),
            spawned_closure_locals: FxHashSet::default(),
            spawned_closure_instances: Vec::new(),
        }
    }

    /// Consumes `CallSiteCollector` and returns its callsites and the
    /// instances of closures spawned on other threads when finished.
    fn finish(
        self,
    ) -> (
        impl IntoIterator<Item = (Instance<'tcx>, CallSiteLocation)>,
        Vec<Instance<'tcx>>,
    ) {
        (self.callsites, self.spawned_closure_instances)
    }
}

impl<'tcx> Visitor<'tcx> for CallSiteCollector<'_, 'tcx> {
    /// Resolve direct call.
    /// Inspired by rustc_mir/src/transform/inline.rs#get_valid_function_call.
    fn visit_terminator(&mut self, terminator: &Terminator<'tcx>, location: Location) {
        if let TerminatorKind::Call {
            ref func, ref args, ..
        } = terminator.kind
        {
            let func_ty = func.ty(self.body, self.tcx);
            // Only after monomorphizing can Instance::try_resolve work
            let func_ty = self.caller.instantiate_mir_and_normalize_erasing_regions(
                self.tcx,
                self.typing_env,
                EarlyBinder::bind(func_ty),
            );
            if let ty::FnDef(def_id, substs) = *func_ty.kind() {
                if let Some(callee) =
                    Instance::try_resolve(self.tcx, self.typing_env, def_id, substs)
                        .ok()
                        .flatten()
                {
                    self.callsites
                        .push((callee, CallSiteLocation::Direct(location)));
                    // A closure passed to a spawn-like call runs on a thread
                    // of its own; remember it by its defining local.
                    if is_spawn_like_callee(&self.tcx.def_path_str(callee.def_id())) {
                        for arg in args {
                            if let Some(place) = arg.node.place() {
                                if place.projection.is_empty() {
                                    let arg_ty =
                                        self.caller.instantiate_mir_and_normalize_erasing_regions(
                                            self.tcx,
                                            self.typing_env,
                                            EarlyBinder::bind(
                                                self.body.local_decls[place.local].ty,
                                            ),
                                        );
                                    if matches!(arg_ty.kind(), TyKind::Closure(..)) {
                                        self.spawned_closure_locals.insert(place.local);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        self.super_terminator(terminator, location);
    }

    /// Find where the closure is defined rather than called,
    /// including the closure instance and the arg.
    ///
    /// e.g., let mut _20: [closure@src/main.rs:13:28: 16:6];
    ///
    /// _20 is of type Closure, but it is actually the arg that captures
    /// the variables in the defining function.
    fn visit_local_decl(&mut self, local: Local, local_decl: &LocalDecl<'tcx>) {
        let func_ty = self.caller.instantiate_mir_and_normalize_erasing_regions(
            self.tcx,
            self.typing_env,
            EarlyBinder::bind(local_decl.ty),
        );
        if let TyKind::Closure(def_id, substs) = func_ty.kind() {
            match self.body.local_kind(local) {
                LocalKind::Arg | LocalKind::ReturnPointer => {}
                _ => {
                    if let Some(callee_instance) =
                        Instance::try_resolve(self.tcx, self.typing_env, *def_id, substs)
                            .ok()
                            .flatten()
                    {
                        if self.spawned_closure_locals.contains(&local) {
                            self.spawned_closure_instances.push(callee_instance);
                        }
                        self.callsites
                            .push((callee_instance, CallSiteLocation::ClosureDef(local, None)));
                    }
                }
            }
        }
        self.super_local_decl(local, local_decl);
    }
}
