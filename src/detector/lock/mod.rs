//! DeadlockDetector: detects doublelock and conflictlock.
extern crate rustc_data_structures;
extern crate rustc_hash;

mod report;
pub use report::Report;
use report::{DeadlockDiagnosis, ReportContent};

use crate::analysis::callgraph::{CallGraph, CallSiteLocation, InstanceId};
use crate::analysis::pointsto::{AliasAnalysis, ApproximateAliasKind};
use crate::interest::concurrency::lock::{
    DeadlockPossibility, LockGuardCollector, LockGuardId, LockGuardMap,
};

use petgraph::algo;
use petgraph::dot::{Config, Dot};
use petgraph::graph::NodeIndex;

use petgraph::visit::{depth_first_search, Control, DfsEvent, EdgeRef, IntoNodeReferences};
use petgraph::{Directed, Direction, Graph};

use rustc_data_structures::graph::WithSuccessors;
use rustc_hash::{FxHashMap, FxHashSet};
use rustc_middle::mir::{Body, Location};
use rustc_middle::ty::{ParamEnv, TyCtxt};

use std::collections::VecDeque;

#[derive(Clone, Debug, Default)]
struct LiveLockGuards(FxHashSet<LockGuardId>);

impl LiveLockGuards {
    fn insert(&mut self, lockguard_id: LockGuardId) -> bool {
        self.0.insert(lockguard_id)
    }
    fn raw_lockguard_ids(&self) -> &FxHashSet<LockGuardId> {
        &self.0
    }
    // self = self \ other, if changed return true
    fn difference_in_place(&mut self, other: &Self) -> bool {
        let old_len = self.0.len();
        for id in &other.0 {
            self.0.remove(id);
        }
        old_len != self.0.len()
    }
    // self = self U other, if changed return true
    fn union_in_place(&mut self, other: Self) -> bool {
        let old_len = self.0.len();
        self.0.extend(other.0.into_iter());
        old_len != self.0.len()
    }
}

/// Detect doublelock and conflictlock.
pub struct DeadlockDetector<'tcx> {
    tcx: TyCtxt<'tcx>,
    param_env: ParamEnv<'tcx>,
    pub lockguard_relations: FxHashSet<(LockGuardId, LockGuardId)>,
}

impl<'tcx> DeadlockDetector<'tcx> {
    pub fn new(tcx: TyCtxt<'tcx>, param_env: ParamEnv<'tcx>) -> Self {
        Self {
            tcx,
            param_env,
            lockguard_relations: Default::default(),
        }
    }

    fn collect_lockguards(
        &self,
        callgraph: &CallGraph<'tcx>,
    ) -> FxHashMap<InstanceId, LockGuardMap<'tcx>> {
        let mut lockguards = FxHashMap::default();
        for (instance_id, instance) in callgraph.graph.node_references() {
            // Only analyze local fn
            if !instance.def_id().is_local() {
                continue;
            }
            let body = self.tcx.instance_mir(instance.def);
            let mut lockguard_collector =
                LockGuardCollector::new(instance_id, instance, body, self.tcx, self.param_env);
            lockguard_collector.analyze();
            if !lockguard_collector.lockguards.is_empty() {
                lockguards.insert(instance_id, lockguard_collector.lockguards);
            }
        }
        lockguards
    }

    /// Detect deadlock inter-procedurally and returns bug report.
    pub fn detect(&mut self, callgraph: &CallGraph<'tcx>) -> Vec<Report> {
        let lockguards = self.collect_lockguards(callgraph);
        let mut worklist = VecDeque::new();
        for (id, _) in callgraph.graph.node_references() {
            worklist.push_back(id);
        }
        // `contexts` records live lockguards before calling each instance, init empty
        let mut contexts = worklist
            .iter()
            .copied()
            .map(|id| (id, LiveLockGuards::default()))
            .collect::<FxHashMap<_, _>>();
        // The fixed-point algorithm
        while let Some(id) = worklist.pop_front() {
            if let Some(lockguard_info) = lockguards.get(&id) {
                let instance = callgraph.index_to_instance(id).unwrap();
                let body = self.tcx.instance_mir(instance.def);
                let context = contexts[&id].clone();
                let states = self.intraproc_gen_kill(body, &context, lockguard_info);
                for edge in callgraph.graph.edges_directed(id, Direction::Outgoing) {
                    let callee = edge.target();
                    let callsite = edge.weight();
                    for callsite_loc in callsite {
                        let CallSiteLocation::FnDef(loc) = callsite_loc;
                        let callsite_state = states[loc].clone();
                        let changed = contexts
                            .get_mut(&callee)
                            .unwrap()
                            .union_in_place(callsite_state);
                        if changed {
                            worklist.push_back(callee);
                        }
                    }
                }
            } else {
                for edge in callgraph.graph.edges_directed(id, Direction::Outgoing) {
                    let callee = edge.target();
                    let context = contexts[&id].clone();
                    let changed = contexts.get_mut(&callee).unwrap().union_in_place(context);
                    if changed {
                        worklist.push_back(callee);
                    }
                }
            }
        }

        // Get lockguard info
        let mut info = FxHashMap::default();
        for (_, map) in lockguards.into_iter() {
            info.extend(map.into_iter());
        }
        self.detect_deadlock(&info, callgraph)
    }

    /// Collect gen/kill info for related locations.
    fn location_to_live_lockguards(
        lockguard_map: &LockGuardMap<'tcx>,
    ) -> (
        FxHashMap<Location, LiveLockGuards>,
        FxHashMap<Location, LiveLockGuards>,
    ) {
        let mut gen_map: FxHashMap<Location, LiveLockGuards> = Default::default();
        let mut kill_map: FxHashMap<Location, LiveLockGuards> = Default::default();
        for (id, info) in lockguard_map {
            for loc in &info.gen_locs {
                gen_map.entry(*loc).or_default().insert(*id);
            }
            for loc in &info.kill_locs {
                kill_map.entry(*loc).or_default().insert(*id);
            }
        }
        (gen_map, kill_map)
    }

    /// state' = state \ kill U gen
    /// return lockguard relation(a, b) where a is still live when b becomes live.
    fn apply_gen_kill(
        state: &mut LiveLockGuards,
        gen: Option<&LiveLockGuards>,
        kill: Option<&LiveLockGuards>,
    ) -> FxHashSet<(LockGuardId, LockGuardId)> {
        // First kill, then gen
        if let Some(kill) = kill {
            state.difference_in_place(kill);
        }
        let mut relations = FxHashSet::default();
        if let Some(gen) = gen {
            for s in state.raw_lockguard_ids() {
                for g in gen.raw_lockguard_ids() {
                    relations.insert((*s, *g));
                }
            }
            state.union_in_place(gen.clone());
        }
        relations
    }

    /// Apply Gen/Kill to get live lockguards for each location in the same fn.
    fn intraproc_gen_kill(
        &mut self,
        body: &'tcx Body<'tcx>,
        context: &LiveLockGuards,
        lockguard_info: &LockGuardMap<'tcx>,
    ) -> FxHashMap<Location, LiveLockGuards> {
        let (gen_map, kill_map) = Self::location_to_live_lockguards(lockguard_info);
        let mut worklist: VecDeque<Location> = Default::default();
        for (bb, bb_data) in body.basic_blocks().iter_enumerated() {
            for stmt_idx in 0..bb_data.statements.len() + 1 {
                worklist.push_back(Location {
                    block: bb,
                    statement_index: stmt_idx,
                });
            }
        }
        let mut states: FxHashMap<Location, LiveLockGuards> = worklist
            .iter()
            .copied()
            .map(|loc| (loc, LiveLockGuards::default()))
            .collect();
        *states.get_mut(&Location::START).unwrap() = context.clone();
        while let Some(loc) = worklist.pop_front() {
            let mut after = states[&loc].clone();
            let relation = Self::apply_gen_kill(&mut after, gen_map.get(&loc), kill_map.get(&loc));
            self.lockguard_relations.extend(relation.into_iter());
            let term_loc = body.terminator_loc(loc.block);
            if loc != term_loc {
                // if not terminator
                let succ = loc.successor_within_block();
                // check lockguard relations
                // union and reprocess if changed
                let changed = states.get_mut(&succ).unwrap().union_in_place(after);
                if changed {
                    worklist.push_back(succ);
                }
            } else {
                // if is terminator
                for succ_bb in body.successors(loc.block) {
                    let succ = Location {
                        block: succ_bb,
                        statement_index: 0,
                    };
                    // union and reprocess if changed
                    let changed = states.get_mut(&succ).unwrap().union_in_place(after.clone());
                    if changed {
                        worklist.push_back(succ);
                    }
                }
            }
        }
        states
    }

    /// First detect doublelock on each relation(a, b),
    /// use non-doublelock relations to build `ConflictLockGraph`.
    /// Then find the cycles in `ConflictLockGraph` as conflictlock.
    fn detect_deadlock(
        &self,
        lockguards: &LockGuardMap<'tcx>,
        callgraph: &CallGraph<'tcx>,
    ) -> Vec<Report> {
        let mut reports = Vec::new();
        let mut conflictlock_graph = ConflictLockGraph::new();
        let mut relation_to_nodes = FxHashMap::default();
        let mut alias_analysis = AliasAnalysis::new(self.tcx, callgraph);
        // Detect doublelock:
        // forall relation(a, b): deadlock(a, b) => doublelock(a, b)
        for (a, b) in &self.lockguard_relations {
            let possibility = deadlock_possibility(a, b, lockguards, &mut alias_analysis);
            match possibility {
                DeadlockPossibility::Probably | DeadlockPossibility::Possibly => {
                    let diagnosis = diagnose_doublelock(a, b, lockguards, callgraph, self.tcx);
                    let report = Report::DoubleLock(ReportContent::new(
                        "DoubleLock".to_owned(),
                        format!("{:?}", possibility),
                        diagnosis,
                        "The first lock is not released when acquiring the second lock".to_owned(),
                    ));
                    reports.push(report);
                }
                _ => {
                    // if unlikely doublelock, add the pair into graph to check conflictlock
                    let node = conflictlock_graph.add_node((*a, *b));
                    relation_to_nodes.insert((*a, *b), node);
                }
            }
        }
        // Detect conflictlock:
        // forall relation(a, b), relation(c, d): deadlock(b, c) and deadlock(d, a) => conflictlock((a, b), (c, d))
        // forall relation(a, b), relation(c, d), relation(e, f): deadlock(b, c) and deadlock(d, e) and deadlock(f, a) => conflictlock((a, b), (c, d), (e, f))
        // ...
        // In general, forall relation(a, b), relation(c, d): deadlock(b, c) => edge(relation(a, b), relation(c, d))
        // if exists a cycle, i.e., edge(r1, r2), edge(r2, r3), ..., edge(rn, r1) then conflictlock((r1, r2, r3, ..., rn))
        for ((_, a), node1) in relation_to_nodes.iter() {
            for ((b, _), node2) in relation_to_nodes.iter() {
                let possibility = deadlock_possibility(a, b, lockguards, &mut alias_analysis);
                match possibility {
                    DeadlockPossibility::Probably | DeadlockPossibility::Possibly => {
                        conflictlock_graph.add_edge(*node1, *node2, possibility);
                    }
                    _ => {}
                };
            }
        }
        let cycle_paths = conflictlock_graph.cycle_paths();
        for path in cycle_paths {
            let diagnosis = path
                .into_iter()
                .map(|relation_id| {
                    let (a, b) = conflictlock_graph.node_weight(relation_id).unwrap();
                    diagnose_one_relation(a, b, lockguards, callgraph, self.tcx)
                })
                .collect::<Vec<_>>();
            let report = report::Report::ConflictLock(ReportContent::new(
                "ConflictLock".to_owned(),
                "Possibly".to_owned(),
                diagnosis,
                "Locks mutually wait for each other to form a cycle".to_owned(),
            ));
            reports.push(report);
        }
        reports
    }
}

/// Check deadlock possibility.
/// for two lockguards, first check if their types may deadlock;
/// if so, then check if they may alias.
fn deadlock_possibility<'tcx>(
    a: &LockGuardId,
    b: &LockGuardId,
    lockguards: &LockGuardMap<'tcx>,
    alias_analysis: &mut AliasAnalysis,
) -> DeadlockPossibility {
    let a_ty = &lockguards[a].lockguard_ty;
    let b_ty = &lockguards[b].lockguard_ty;
    match a_ty.deadlock_with(b_ty) {
        DeadlockPossibility::Probably => match alias_analysis.alias((*a).into(), (*b).into()) {
            ApproximateAliasKind::Probably => DeadlockPossibility::Probably,
            ApproximateAliasKind::Possibly => DeadlockPossibility::Possibly,
            ApproximateAliasKind::Unlikely => DeadlockPossibility::Unlikely,
            ApproximateAliasKind::Unknown => DeadlockPossibility::Unknown,
        },
        DeadlockPossibility::Possibly => match alias_analysis.alias((*a).into(), (*b).into()) {
            ApproximateAliasKind::Probably => DeadlockPossibility::Possibly,
            ApproximateAliasKind::Possibly => DeadlockPossibility::Possibly,
            ApproximateAliasKind::Unlikely => DeadlockPossibility::Unlikely,
            ApproximateAliasKind::Unknown => DeadlockPossibility::Unknown,
        },
        _ => DeadlockPossibility::Unlikely,
    }
}

/// Generate doublelock diagnosis.
fn diagnose_doublelock<'tcx>(
    a: &LockGuardId,
    b: &LockGuardId,
    lockguards: &LockGuardMap<'tcx>,
    callgraph: &CallGraph<'tcx>,
    tcx: TyCtxt<'tcx>,
) -> report::DeadlockDiagnosis {
    diagnose_one_relation(a, b, lockguards, callgraph, tcx)
}

/// Find all the callchains: source -> target
// e.g., for one path: source --|callsites1|--> medium --|callsites2|--> target,
// first extract callsite locations on edge, namely, [callsites1, callsites2],
// then map locations to spans [spans1, spans2].
fn track_callchains<'tcx>(
    source: InstanceId,
    target: InstanceId,
    callgraph: &CallGraph<'tcx>,
    tcx: TyCtxt<'tcx>,
) -> Vec<Vec<Vec<String>>> {
    if source == target {
        vec![]
    } else {
        let paths = callgraph.all_simple_paths(source, target);
        paths
            .into_iter()
            .map(|vec| {
                vec.iter()
                    .zip(vec.iter().skip(1))
                    .map(|(caller, callee)| {
                        let caller_instance = callgraph.index_to_instance(*caller).unwrap();
                        let caller_body = tcx.instance_mir(caller_instance.def);
                        let callsites = callgraph.callsites(*caller, *callee).unwrap();
                        callsites
                            .into_iter()
                            .map(|location| {
                                let CallSiteLocation::FnDef(location) = location;
                                format!("{:?}", caller_body.source_info(location).span)
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    }
}

// Find the diagnosis info for relation(a, b), including a's ty & span, b's ty & span, and callchains.
fn diagnose_one_relation<'tcx>(
    a: &LockGuardId,
    b: &LockGuardId,
    lockguards: &LockGuardMap<'tcx>,
    callgraph: &CallGraph<'tcx>,
    tcx: TyCtxt<'tcx>,
) -> DeadlockDiagnosis {
    let a_info = &lockguards[a];
    let b_info = &lockguards[b];
    let first_lock = (
        format!("{:?}", a_info.lockguard_ty),
        format!("{:?}", a_info.span),
    );
    let second_lock = (
        format!("{:?}", b_info.lockguard_ty),
        format!("{:?}", b_info.span),
    );
    let callchains = track_callchains(a.instance_id, b.instance_id, callgraph, tcx);
    DeadlockDiagnosis::new(
        first_lock.0,
        first_lock.1,
        second_lock.0,
        second_lock.1,
        callchains,
    )
}

/// The NodeIndex in ConflictLockGraph, denoting a unique relation(a, b) in ConflictLockGraph,
/// where a and b are LockGuardId.
type RelationId = NodeIndex;

/// forall relation(a, b), relation(c, d): if b and c are probably/possibly deadlock,
/// then add edge between relation(a, b) and relation(c, d).
/// The cycles in the graph may be conflictlock bugs.
struct ConflictLockGraph {
    graph: Graph<(LockGuardId, LockGuardId), DeadlockPossibility, Directed>,
}

impl ConflictLockGraph {
    fn new() -> Self {
        Self {
            graph: Graph::new(),
        }
    }
    fn add_node(&mut self, relation: (LockGuardId, LockGuardId)) -> RelationId {
        self.graph.add_node(relation)
    }

    fn add_edge(&mut self, a: RelationId, b: RelationId, weight: DeadlockPossibility) {
        self.graph.add_edge(a, b, weight);
    }

    fn node_weight(&self, a: RelationId) -> Option<&(LockGuardId, LockGuardId)> {
        self.graph.node_weight(a)
    }

    /// Find all the back-edges in the graph.
    fn back_edges(&self) -> Vec<(RelationId, RelationId)> {
        let mut back_edges = Vec::new();
        let nodes = self.graph.node_indices();
        for start in nodes {
            depth_first_search(&self.graph, Some(start), |event| {
                match event {
                    DfsEvent::BackEdge(u, v) => {
                        if !back_edges.contains(&(u, v)) && !back_edges.contains(&(v, u)) {
                            back_edges.push((u, v));
                        }
                    }
                    DfsEvent::Finish(_, _) => {
                        return Control::Break(());
                    }
                    _ => {}
                };
                Control::Continue
            });
        }
        back_edges
    }

    /// Find all the cycles in the graph.
    fn cycle_paths(&self) -> Vec<Vec<RelationId>> {
        let mut dedup = Vec::new();
        let mut edge_sets = Vec::new();
        for (src, target) in self.back_edges() {
            let cycle_paths =
                algo::all_simple_paths::<Vec<_>, _>(&self.graph, target, src, 0, None)
                    .collect::<Vec<_>>();
            for path in cycle_paths {
                // `path` forms a cycle, where adjacent nodes are directly connected and last_node connects to first_node.
                // Different `path`s beginning with different nodes may denote the same cycle if their edges are the same.
                // Thus we use `edge_sets` to deduplicate the cycle paths.
                let set = path
                    .iter()
                    .zip(path.iter().skip(1).chain(path.get(0)))
                    .map(|(a, b)| (*a, *b))
                    .collect::<FxHashSet<_>>();
                if !edge_sets.contains(&set) {
                    edge_sets.push(set);
                    dedup.push(path);
                }
            }
        }
        dedup
    }

    /// Print the ConflictGraph in dot format.
    #[allow(dead_code)]
    fn dot(&self) {
        println!(
            "{:?}",
            Dot::with_config(&self.graph, &[Config::GraphContentOnly])
        );
    }
}
