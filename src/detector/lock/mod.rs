//! DeadlockDetector: detects doublelock and conflictlock.
extern crate rustc_data_structures;
extern crate rustc_hash;

mod report;
pub use report::Report;
use report::{DeadlockDiagnosis, ReportContent};

use crate::analysis::callgraph::{CallGraph, CallGraphNode, InstanceId};
use crate::analysis::pointsto::{AliasAnalysis, AliasId, ApproximateAliasKind};
use crate::interest::concurrency::condvar::{CondvarApi, ParkingLotCondvarApi, StdCondvarApi};
use crate::interest::concurrency::lock::{
    DeadlockPossibility, LockGuardCollector, LockGuardId, LockGuardMap, LockGuardTy,
};

use petgraph::algo;
use petgraph::dot::{Config, Dot};
use petgraph::graph::NodeIndex;

use petgraph::visit::{depth_first_search, Control, DfsEvent, EdgeRef, IntoNodeReferences};
use petgraph::{Directed, Direction, Graph};

use rustc_hash::{FxHashMap, FxHashSet};
use rustc_middle::mir::{Body, Location, Operand, TerminatorKind};
use rustc_middle::ty::{ParamEnv, TyCtxt};

use std::collections::VecDeque;

use self::report::{CondvarDeadlockDiagnosis, WaitNotifyLocks};

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

type LockGuardsBeforeCallSites = FxHashMap<(InstanceId, Location), LiveLockGuards>;

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
        for (instance_id, node) in callgraph.graph.node_references() {
            let instance = match node {
                CallGraphNode::WithBody(instance) => instance,
                _ => continue,
            };
            // Only analyze local fn with body
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

    /// Collect condvar APIs.
    /// Return the condvar API's InstanceId and kind.
    fn collect_condvars(&self, callgraph: &CallGraph<'tcx>) -> FxHashMap<InstanceId, CondvarApi> {
        callgraph
            .graph
            .node_references()
            .filter_map(|(instance_id, node)| {
                CondvarApi::from_instance(node.instance(), self.tcx)
                    .map(|condvar_api| (instance_id, condvar_api))
            })
            .collect()
    }

    /// Detect deadlock inter-procedurally and returns bug report.
    pub fn detect<'a>(
        &mut self,
        callgraph: &'a CallGraph<'tcx>,
        alias_analysis: &mut AliasAnalysis<'a, 'tcx>,
    ) -> Vec<Report> {
        let lockguards = self.collect_lockguards(callgraph);
        let condvar_apis = self.collect_condvars(callgraph);
        let mut lockguards_before_condvar_apis: FxHashMap<InstanceId, LockGuardsBeforeCallSites> =
            condvar_apis
                .keys()
                .map(|instance_id| (*instance_id, FxHashMap::default()))
                .collect();
        // Init `worklist` with all the `InstanceId`s
        let mut worklist = callgraph
            .graph
            .node_references()
            .map(|(instance_id, _)| instance_id)
            .collect::<VecDeque<_>>();
        // `contexts` records live lockguards before calling each instance, init empty
        let mut contexts = worklist
            .iter()
            .copied()
            .map(|id| (id, LiveLockGuards::default()))
            .collect::<FxHashMap<_, _>>();
        // The fixed-point algorithm
        while let Some(id) = worklist.pop_front() {
            if let Some(lockguard_info) = lockguards.get(&id) {
                let instance = match callgraph.index_to_instance(id).unwrap() {
                    CallGraphNode::WithBody(instance) => instance,
                    _ => continue,
                };
                let body = self.tcx.instance_mir(instance.def);
                let context = contexts[&id].clone();
                let states = self.intraproc_gen_kill(body, &context, lockguard_info);
                for edge in callgraph.graph.edges_directed(id, Direction::Outgoing) {
                    let callee = edge.target();
                    for callsite in edge.weight() {
                        let loc = match callsite.location() {
                            Some(loc) => loc,
                            None => continue,
                        };
                        let callsite_state = states[&loc].clone();
                        let changed = contexts
                            .get_mut(&callee)
                            .unwrap()
                            .union_in_place(callsite_state);
                        if changed {
                            worklist.push_back(callee);
                        }
                        if condvar_apis.contains_key(&callee) {
                            lockguards_before_condvar_apis
                                .entry(callee)
                                .or_default()
                                .entry((id, loc))
                                .or_default()
                                .union_in_place(states[&loc].clone());
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
                    if condvar_apis.contains_key(&callee) {
                        for callsite in edge.weight() {
                            if let Some(loc) = callsite.location() {
                                lockguards_before_condvar_apis
                                    .entry(callee)
                                    .or_default()
                                    .entry((id, loc))
                                    .or_default()
                                    .union_in_place(contexts[&id].clone());
                            }
                        }
                    }
                }
            }
        }

        // Get lockguard info
        let mut info = FxHashMap::default();
        for (_, map) in lockguards.into_iter() {
            info.extend(map.into_iter());
        }

        let mut reports = self.detect_deadlock(&info, callgraph, alias_analysis);
        if !lockguards_before_condvar_apis.is_empty() {
            reports.extend(
                self.detect_condvar_misuse(
                    &lockguards_before_condvar_apis,
                    &condvar_apis,
                    &info,
                    callgraph,
                    alias_analysis,
                )
                .into_iter(),
            );
        }
        reports
    }

    /// Detect condvar misuse.
    /// First collect Condvar APIs info: callsites to (Condvar, MutexGuard)
    /// - std::sync::Condvar::wait(&Condvar, MutexGuard) -> MutexGuard
    /// - std::sync::Condvar::notify(&Condvar)
    /// - parking_lot::Condvar::wait(&Condvar, &mut MutexGuard)
    /// - parking_lot::Condvar::notify(&Condvar)
    /// Then match `wait` and `notify` if their Condvars alias with each other.
    /// Finally check LiveLockGuards before `wait` and `notify`:
    /// if they have aliasing LockGuards that are not aliased with MutexGuard in `wait` then possibly deadlock
    /// else if LiveLockGuards before `notify` does not contain a LockGuard aliasing with
    /// the MutexLock in `wait`, then possibly missing lock before notify.
    /// Note that the `missing lock` warning is imprecise because the rule rigidly follows the
    /// Condvar example code. In fact, it is correct as long as there is a LockGuard aliasing with
    /// the MutexGuard in `wait` that dominates `notify`.
    /// TODO(boqin): check if an aliased MutexGuard dominates `notify` instead.
    fn detect_condvar_misuse<'a>(
        &self,
        lockguards_before_condvar_apis: &FxHashMap<InstanceId, LockGuardsBeforeCallSites>,
        condvar_apis: &FxHashMap<InstanceId, CondvarApi>,
        lockguards: &LockGuardMap<'tcx>,
        callgraph: &'a CallGraph<'tcx>,
        alias_analysis: &mut AliasAnalysis<'a, 'tcx>,
    ) -> Vec<Report> {
        let mut reports = Vec::new();
        // Collect Condvar API info
        // callee_instance -> (Caller, Location, Callee) -> (&Condvar, MutexGuard)
        let mut std_notify = FxHashMap::default();
        // callee_instance -> (Caller, Location, Callee) -> &Condvar
        let mut std_wait = FxHashMap::default();
        // callee_instance -> (Caller, Location, Callee) -> (&Condvar, &mut MutexGuard)
        let mut parking_lot_notify = FxHashMap::default();
        // callee_instance -> (Caller, Location, Callee) -> &Condvar
        let mut parking_lot_wait = FxHashMap::default();
        for (callee_id, callsite_lockguards) in lockguards_before_condvar_apis {
            let condvar_api = condvar_apis.get(callee_id).unwrap();
            for (caller_id, loc) in callsite_lockguards.keys() {
                let body = self.tcx.instance_mir(
                    callgraph
                        .index_to_instance(*caller_id)
                        .unwrap()
                        .instance()
                        .def,
                );
                let term = body[loc.block].terminator();
                let args = match &term.kind {
                    TerminatorKind::Call { func: _, args, .. } => args.clone(),
                    _ => continue,
                };
                match condvar_api {
                    CondvarApi::Std(StdCondvarApi::Wait(_)) => {
                        if let (Operand::Move(condvar_ref), Operand::Move(mutex_guard)) =
                            (&args[0], &args[1])
                        {
                            // callsite -> (&Condvar, MutexGuard)
                            std_wait.insert(
                                (*caller_id, *loc, *callee_id),
                                (
                                    AliasId {
                                        instance_id: *caller_id,
                                        local: condvar_ref.local,
                                    },
                                    AliasId {
                                        instance_id: *caller_id,
                                        local: mutex_guard.local,
                                    },
                                ),
                            );
                        }
                    }
                    CondvarApi::ParkingLot(ParkingLotCondvarApi::Wait(_)) => {
                        if let (Operand::Move(condvar_ref), Operand::Move(mutex_guard_ref)) =
                            (&args[0], &args[1])
                        {
                            // callsite -> (&Condvar, &mut MutexGuard)
                            parking_lot_wait.insert(
                                (*caller_id, *loc, *callee_id),
                                (
                                    AliasId {
                                        instance_id: *caller_id,
                                        local: condvar_ref.local,
                                    },
                                    AliasId {
                                        instance_id: *caller_id,
                                        local: mutex_guard_ref.local,
                                    },
                                ),
                            );
                        }
                    }
                    CondvarApi::Std(StdCondvarApi::Notify(_)) => {
                        if let Operand::Move(condvar_ref) = args[0] {
                            // callsite -> &Condvar
                            std_notify.insert(
                                (*caller_id, *loc, *callee_id),
                                AliasId {
                                    instance_id: *caller_id,
                                    local: condvar_ref.local,
                                },
                            );
                        }
                    }
                    CondvarApi::ParkingLot(ParkingLotCondvarApi::Notify(_)) => {
                        if let Operand::Move(condvar_ref) = args[0] {
                            // callsite -> &Condvar
                            parking_lot_notify.insert(
                                (*caller_id, *loc, *callee_id),
                                AliasId {
                                    instance_id: *caller_id,
                                    local: condvar_ref.local,
                                },
                            );
                        }
                    }
                }
            }
        }
        // Check std::sync::Condvar
        for ((caller_id1, loc1, callee_id1), (condvar_ref1, mutex_guard1)) in std_wait.iter() {
            for ((caller_id2, loc2, callee_id2), condvar_ref2) in std_notify.iter() {
                let res = alias_analysis.alias(*condvar_ref1, *condvar_ref2);
                match res {
                    ApproximateAliasKind::Possibly | ApproximateAliasKind::Probably => {
                        // live1: LiveLockGuards before `wait`
                        // live2: LiveLockGuards before `notify`
                        let live1 = lockguards_before_condvar_apis
                            .get(callee_id1)
                            .and_then(|cs| cs.get(&(*caller_id1, *loc1)));
                        let live2 = lockguards_before_condvar_apis
                            .get(callee_id2)
                            .and_then(|cs| cs.get(&(*caller_id2, *loc2)));
                        match (live1, live2) {
                            (Some(live1), Some(live2)) => {
                                // aliased_pairs = {(l1, l2) | (l1, l2) in live1 X live2 and alias(l1, l2)}
                                let live1 = live1.raw_lockguard_ids().iter();
                                let live2 = live2.raw_lockguard_ids().iter();
                                let cartesian_product =
                                    live2.flat_map(|g2| live1.clone().map(move |g1| (*g1, *g2)));
                                let aliased_pairs = cartesian_product
                                    .filter(|(g1, g2)| {
                                        alias_analysis.alias((*g1).into(), (*g2).into())
                                            > ApproximateAliasKind::Unlikely
                                            && deadlock_possibility(
                                                g1,
                                                g2,
                                                lockguards,
                                                alias_analysis,
                                            )
                                            .0 > DeadlockPossibility::Unlikely
                                    })
                                    .collect::<Vec<_>>();
                                // exists (g1, g2) in aliased_pairs: alias(g2, mutex_guard1)
                                // LockGuard pairs that do not alias with MutexGuard in `wait`
                                let mut no_mutex_guards = Vec::new();
                                for (g1, g2) in aliased_pairs.iter() {
                                    if AliasId::from(*g1) != *mutex_guard1 {
                                        no_mutex_guards.push((g1, g2));
                                    }
                                }
                                if !no_mutex_guards.is_empty() {
                                    let diagnosis = diagnose_condvar_deadlock(
                                        (*caller_id1, *loc1),
                                        (*caller_id2, *loc2),
                                        true,
                                        &no_mutex_guards,
                                        lockguards,
                                        callgraph,
                                        self.tcx,
                                    );
                                    let content = ReportContent::new(
                                        "Deadlock before Condvar::wait and notify".to_owned(),
                                        "Possibly".to_owned(),
                                        diagnosis,
                                        "The same lock before Condvar::wait and notify".to_owned(),
                                    );
                                    let report = Report::CondvarDeadlock(content);
                                    reports.push(report);
                                }
                            }
                            (Some(_), None) => {}
                            _ => {
                                // There must be a MutexGuard before `wait`.
                                unreachable!()
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // Check parking_lot::Condvar
        for ((caller_id1, loc1, callee_id1), (condvar_ref1, mutex_guard1)) in
            parking_lot_wait.iter()
        {
            for ((caller_id2, loc2, callee_id2), condvar_ref2) in parking_lot_notify.iter() {
                let res = alias_analysis.alias(*condvar_ref1, *condvar_ref2);
                match res {
                    ApproximateAliasKind::Possibly | ApproximateAliasKind::Probably => {
                        // live1: LiveLockGuards before `wait`
                        // live2: LiveLockGuards before `notify`
                        let live1 = lockguards_before_condvar_apis
                            .get(callee_id1)
                            .and_then(|cs| cs.get(&(*caller_id1, *loc1)));
                        let live2 = lockguards_before_condvar_apis
                            .get(callee_id2)
                            .and_then(|cs| cs.get(&(*caller_id2, *loc2)));
                        match (live1, live2) {
                            (Some(live1), Some(live2)) => {
                                // aliased_pairs = {(l1, l2) | (l1, l2) in live1 X live2 and alias(l1, l2)}
                                let live1 = live1.raw_lockguard_ids().iter();
                                let live2 = live2.raw_lockguard_ids().iter();
                                let cartesian_product =
                                    live2.flat_map(|g2| live1.clone().map(move |g1| (*g1, *g2)));
                                let aliased_pairs = cartesian_product
                                    .filter(|(g1, g2)| {
                                        alias_analysis.alias((*g1).into(), (*g2).into())
                                            > ApproximateAliasKind::Unlikely
                                            && deadlock_possibility(
                                                g1,
                                                g2,
                                                lockguards,
                                                alias_analysis,
                                            )
                                            .0 > DeadlockPossibility::Unlikely
                                    })
                                    .collect::<Vec<_>>();
                                // exists (g1, g2) in aliased_pairs: alias(g2, mutex_guard1)
                                // LockGuard pairs that do not alias with MutexGuard in `wait`
                                let mut no_mutex_guards = Vec::new();
                                for (g1, g2) in aliased_pairs.iter() {
                                    if !matches!(
                                        alias_analysis.points_to(*mutex_guard1, AliasId::from(*g1)),
                                        ApproximateAliasKind::Possibly
                                            | ApproximateAliasKind::Probably
                                    ) {
                                        no_mutex_guards.push((g1, g2));
                                    }
                                }
                                if !no_mutex_guards.is_empty() {
                                    let diagnosis = diagnose_condvar_deadlock(
                                        (*caller_id1, *loc1),
                                        (*caller_id2, *loc2),
                                        false,
                                        &no_mutex_guards,
                                        lockguards,
                                        callgraph,
                                        self.tcx,
                                    );
                                    let content = ReportContent::new(
                                        "Deadlock before Condvar::wait and notify".to_owned(),
                                        "Possibly".to_owned(),
                                        diagnosis,
                                        "The same lock before Condvar::wait and notify".to_owned(),
                                    );
                                    let report = Report::CondvarDeadlock(content);
                                    reports.push(report);
                                }
                            }
                            (Some(_), None) => {}
                            _ => {
                                // There must be a MutexGuard before `wait`.
                                unreachable!()
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        reports
    }

    /// Collect gen/kill info for related locations.
    fn gen_kill_locations(
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
        let (gen_map, kill_map) = Self::gen_kill_locations(lockguard_info);
        let mut worklist: VecDeque<Location> = Default::default();
        for (bb, bb_data) in body.basic_blocks.iter_enumerated() {
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
                for succ_bb in body[loc.block].terminator().successors() {
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
    fn detect_deadlock<'a>(
        &self,
        lockguards: &LockGuardMap<'tcx>,
        callgraph: &'a CallGraph<'tcx>,
        alias_analysis: &mut AliasAnalysis<'a, 'tcx>,
    ) -> Vec<Report> {
        let mut reports = Vec::new();
        let mut conflictlock_graph = ConflictLockGraph::new();
        let mut relation_to_nodes = FxHashMap::default();
        // Detect doublelock:
        // forall relation(a, b): deadlock(a, b) => doublelock(a, b)
        for (a, b) in &self.lockguard_relations {
            let (possibility, reason) = deadlock_possibility(a, b, lockguards, alias_analysis);
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
                _ if NotDeadlockReason::RecursiveRead != reason
                    && NotDeadlockReason::SameSpan != reason =>
                {
                    // if unlikely doublelock, add the pair into graph to check conflictlock
                    // when the lockguards are gen by call rather than move
                    if !lockguards[a].is_gen_only_by_move() && !lockguards[b].is_gen_only_by_move()
                    {
                        let node = conflictlock_graph.add_node((*a, *b));
                        relation_to_nodes.insert((*a, *b), node);
                    }
                }
                _ => {}
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
                let (possibility, _) = deadlock_possibility(a, b, lockguards, alias_analysis);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotDeadlockReason {
    TrueDeadlock,
    RecursiveRead,
    SameSpan,
    // TODO,
}

/// Check deadlock possibility.
/// for two lockguards, first check if their types may deadlock;
/// if so, then check if they may alias.
fn deadlock_possibility(
    a: &LockGuardId,
    b: &LockGuardId,
    lockguards: &LockGuardMap<'_>,
    alias_analysis: &mut AliasAnalysis,
) -> (DeadlockPossibility, NotDeadlockReason) {
    let a_ty = &lockguards[a].lockguard_ty;
    let b_ty = &lockguards[b].lockguard_ty;
    if let (LockGuardTy::ParkingLotRead(_), LockGuardTy::ParkingLotRead(_)) = (a_ty, b_ty) {
        if lockguards[b].is_gen_only_by_recursive() {
            return (
                DeadlockPossibility::Unlikely,
                NotDeadlockReason::RecursiveRead,
            );
        }
    }
    // Assume that a lock in a loop or recursive functions will not deadlock with itself,
    // in which case the lock spans of the two locks are the same.
    // This may miss some bugs but can reduce many FPs.
    if lockguards[a].span == lockguards[b].span {
        return (DeadlockPossibility::Unlikely, NotDeadlockReason::SameSpan);
    }
    let possibility = match a_ty.deadlock_with(b_ty) {
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
    };
    (possibility, NotDeadlockReason::TrueDeadlock)
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
    let paths = callgraph.all_simple_paths(source, target);
    paths
        .into_iter()
        .map(|vec| {
            vec.windows(2)
                .map(|window| {
                    let (caller, callee) = (window[0], window[1]);
                    let caller_instance = match callgraph.index_to_instance(caller).unwrap() {
                        CallGraphNode::WithBody(instance) => instance,
                        n => panic!("CallGraphNode {:?} must own body", n),
                    };
                    let caller_body = tcx.instance_mir(caller_instance.def);
                    let callsites = callgraph.callsites(caller, callee).unwrap();
                    callsites
                        .into_iter()
                        .filter_map(|location| {
                            location
                                .location()
                                .map(|loc| format!("{:?}", caller_body.source_info(loc).span))
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
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

fn diagnose_condvar_deadlock<'tcx>(
    callsite1: (InstanceId, Location),
    callsite2: (InstanceId, Location),
    is_std_condvar: bool,
    aliased_pairs: &[(&LockGuardId, &LockGuardId)],
    lockguards: &LockGuardMap<'tcx>,
    callgraph: &CallGraph<'tcx>,
    tcx: TyCtxt<'tcx>,
) -> CondvarDeadlockDiagnosis {
    let (caller_id1, loc1) = callsite1;
    let (caller_id2, loc2) = callsite2;
    let caller_body1 = tcx.instance_mir(
        callgraph
            .index_to_instance(caller_id1)
            .unwrap()
            .instance()
            .def,
    );
    let caller_body2 = tcx.instance_mir(
        callgraph
            .index_to_instance(caller_id2)
            .unwrap()
            .instance()
            .def,
    );
    let wait_span = format!("{:?}", caller_body1.source_info(loc1).span);
    let notify_span = format!("{:?}", caller_body2.source_info(loc2).span);
    let wait_notify_locks = aliased_pairs
        .iter()
        .map(|(a, b)| {
            let a_info = &lockguards[a];
            let b_info = &lockguards[b];
            WaitNotifyLocks::new(
                format!("{:?}", a_info.lockguard_ty),
                format!("{:?}", a_info.span),
                format!("{:?}", b_info.lockguard_ty),
                format!("{:?}", b_info.span),
            )
        })
        .collect::<Vec<_>>();
    if is_std_condvar {
        CondvarDeadlockDiagnosis::new(
            "std::sync::Condvar::wait".to_owned(),
            wait_span,
            "std::sync::Condvar::notify".to_owned(),
            notify_span,
            wait_notify_locks,
        )
    } else {
        CondvarDeadlockDiagnosis::new(
            "parking_lot::Condvar::wait".to_owned(),
            wait_span,
            "parking_lot::Condvar::notify".to_owned(),
            notify_span,
            wait_notify_locks,
        )
    }
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
