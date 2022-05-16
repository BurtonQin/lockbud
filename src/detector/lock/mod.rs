//! LockBugDetector: detects doublelock and conflictlock
//! bfs each WCC of callgraph
//! if instance contains lockguards then make it vertex
//! else push it to path
//! so as to build a new LockGuardCallGraph
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

use petgraph::unionfind::UnionFind;
use petgraph::visit::{
    depth_first_search, Control, DfsEvent, EdgeRef, IntoNodeReferences,
    NodeIndexable,
};
use petgraph::{Directed, Direction, Graph};

use rustc_data_structures::graph::WithSuccessors;
use rustc_hash::{FxHashMap, FxHashSet};
use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{Body, Location, Terminator, TerminatorKind};
use rustc_middle::ty::{Instance, ParamEnv, TyCtxt};

use std::collections::VecDeque;

use smallvec::SmallVec;

pub struct LockGuardInstanceGraph<'tcx> {
    pub graph: Graph<InstanceId, Vec<InstanceId>, Directed>,
    lockguard_instances: FxHashMap<InstanceId, LockGuardMap<'tcx>>,
}

// The immutable context when dfs recursively visit instances with lockguards

struct LockGuardInstanceContext<'a, 'b, 'c, 'tcx> {
    start: InstanceId,
    callgraph: &'a CallGraph<'tcx>,
    lockguard_instances: &'b FxHashMap<InstanceId, LockGuardMap<'tcx>>,
    lockguard_instance_ids: &'c FxHashMap<InstanceId, NodeIndex>,
}

impl<'a, 'b, 'c, 'tcx> LockGuardInstanceContext<'a, 'b, 'c, 'tcx> {
    fn new(
        start: InstanceId,
        callgraph: &'a CallGraph<'tcx>,
        lockguard_instances: &'b FxHashMap<InstanceId, LockGuardMap<'tcx>>,
        lockguard_instance_ids: &'c FxHashMap<InstanceId, NodeIndex>,
    ) -> Self {
        Self {
            start,
            callgraph,
            lockguard_instances,
            lockguard_instance_ids,
        }
    }
}

impl<'tcx> LockGuardInstanceGraph<'tcx> {
    pub fn new() -> Self {
        Self {
            graph: Graph::new(),
            lockguard_instances: FxHashMap::default(),
        }
    }

    pub fn analyze(
        &mut self,
        callgraph: &CallGraph<'tcx>,
        tcx: TyCtxt<'tcx>,
        param_env: ParamEnv<'tcx>,
    ) {
        // filter local instances with lockguards in it.
        let mut lockguard_instances = FxHashMap::default();
        let mut lockguard_instance_ids = FxHashMap::default();
        for (instance_id, instance) in callgraph.graph.node_references() {
            if !instance.def_id().is_local() {
                continue;
            }
            let body = tcx.instance_mir(instance.def);
            let mut lockguard_collector =
                LockGuardCollector::new(instance_id, instance, body, tcx, param_env);
            lockguard_collector.analyze();
            if !lockguard_collector.lockguards.is_empty() {
                let lockguard_instance_id = self.graph.add_node(instance_id);
                lockguard_instance_ids.insert(instance_id, lockguard_instance_id);
                // println!("{:?}: {:?}", instance_id, instance);
                lockguard_instances.insert(instance_id, lockguard_collector.lockguards);
            }
        }
        // find all paths between the instances with lockguards
        for (instance_id, _) in lockguard_instances.iter() {
            self.dfs_visit_lockguard_instance(
                *instance_id,
                callgraph,
                &lockguard_instances,
                &lockguard_instance_ids,
            );
        }
        self.lockguard_instances = lockguard_instances;
    }

    // find all paths between the starting lockguard instance and the first met lockguard instance
    fn dfs_visit_lockguard_instance(
        &mut self,
        start: InstanceId,
        callgraph: &CallGraph<'tcx>,
        lockguard_instances: &FxHashMap<InstanceId, LockGuardMap<'tcx>>,
        lockguard_instance_ids: &FxHashMap<InstanceId, NodeIndex>,
    ) {
        let mut visited = FxHashSet::default();
        let mut path = Vec::new();
        let context = LockGuardInstanceContext::new(
            start,
            callgraph,
            lockguard_instances,
            lockguard_instance_ids,
        );
        self.dfs_visit_lockguard_instance_recur(start, &mut visited, &mut path, &context);
    }

    fn dfs_visit_lockguard_instance_recur(
        &mut self,
        curr: InstanceId,
        visited: &mut FxHashSet<InstanceId>,
        path: &mut Vec<InstanceId>,
        context: &LockGuardInstanceContext,
    ) {
        // println!("curr: {:?}", curr);
        visited.insert(curr);
        path.push(curr);
        // println!("path: {:?}", path);
        // the first met lockguard instance
        if curr != context.start && context.lockguard_instances.contains_key(&curr) {
            let start_node = context.lockguard_instance_ids[&context.start];
            let curr_node = context.lockguard_instance_ids[&curr];
            self.graph.add_edge(start_node, curr_node, path.clone());
        } else {
            for n in context.callgraph.graph.neighbors(curr) {
                if !visited.contains(&n) {
                    self.dfs_visit_lockguard_instance_recur(n, visited, path, context);
                }
            }
        }
        path.pop();
        visited.remove(&curr);
    }

    pub fn weak_connected_component_roots(&self) -> Vec<NodeIndex> {
        let mut vertex_sets = UnionFind::new(self.graph.node_bound());
        for edge in self.graph.edge_references() {
            let (a, b) = (edge.source(), edge.target());
            // union the two vertices of the edge
            vertex_sets.union(self.graph.to_index(a), self.graph.to_index(b));
        }

        let mut labels = vertex_sets.into_labeling();
        labels.sort();
        labels.dedup();
        labels.into_iter().map(|u| NodeIndex::new(u)).collect()
    }

    pub fn index_to_instance_id(&self, idx: NodeIndex) -> Option<&InstanceId> {
        self.graph.node_weight(idx)
    }

    // Print the callgraph in dot format
    pub fn dot(&self) {
        println!(
            "{:?}",
            Dot::with_config(&self.graph, &[Config::GraphContentOnly])
        );
    }
}

// CallGraph<ID> -> LGCallGraph<ID> -> PrunedLGCallGraph(ID x Location)
// PrunedLockGuardGraph
// Graph: <(InstanceId, Location), CallChain>
// interest: Location is START, return, resume, callsites, gen, kill

pub type LocationId = (InstanceId, Location);

// Locations of Interest
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum LocationKind {
    Start(LocationId),
    Return(LocationId),
    Resume(LocationId),
    CallSite(LocationId),
    ReturnSite(LocationId),
    ResumeSite(LocationId),
    Gen(LocationId),
    Kill(LocationId),
}
pub struct PrunedLockGuardGraph {
    graph: Graph<LocationId, Vec<(InstanceId, Location)>>,
    location_kinds: FxHashSet<LocationKind>,
}

impl PrunedLockGuardGraph {
    fn analyze<'tcx>(
        &mut self,
        lockguard_instance_graph: &LockGuardInstanceGraph<'tcx>,
        callgraph: &CallGraph<'tcx>,
        tcx: TyCtxt<'tcx>,
    ) {
        // 1. collect return resume locations of instance from Graph
        // 2. collect the outgoing edges of instance on Graph
        // 3. for each edge(from: inst1=instance, to: inst2, weight=path):
        //      add edge(inst1.path[0], inst2.start, weight=path[1:])
        //      add edge(inst2.return, inst1.path[0].succ[return], weight)
        //      add edge(inst2.return, inst1.path[0].succ[resume], weight) if exists loc in path[1:]: loc.succ has resume
        //      add edge(inst2.resume, inst1.path[0].succ[resume], weight)
        // 4. collect gen/kill locations of instance from lockguard_instances
        // 5. for each gen/kill/start/return/resume location loc:
        //      dfs_visit(loc) to add edge between locs
        //
        // 1. instance_id -> return/resume
        let mut returns = FxHashMap::default();
        let mut resumes = FxHashMap::default();
        for (_, instance_id) in lockguard_instance_graph.graph.node_references() {
            let instance = callgraph.index_to_instance(*instance_id).unwrap();
            let collector = Self::collect_return_resume(instance, tcx);
            let (collector_returns, collector_resumes) = (collector.returns, collector.resumes);
            returns.insert(*instance_id, collector_returns);
            resumes.insert(*instance_id, collector_resumes);
        }
        // 2. outgoing edges of instance
        for (node_idx, src_instance_id) in lockguard_instance_graph.graph.node_references() {
            let _src_instance = callgraph.index_to_instance(*src_instance_id).unwrap();
            for edge in lockguard_instance_graph
                .graph
                .edges_directed(node_idx, petgraph::Direction::Outgoing)
            {
                let target_instance_id = lockguard_instance_graph
                    .index_to_instance_id(edge.target())
                    .unwrap();
                let _target_instance = callgraph.index_to_instance(*target_instance_id).unwrap();
                let callchain = edge.weight();
                if callchain.is_empty() {
                    // src_instance directly connects to target_instance
                    let edge = callgraph
                        .graph
                        .find_edge(*src_instance_id, *target_instance_id)
                        .unwrap();
                    let path = callgraph.graph.edge_weight(edge).unwrap();
                    let CallSiteLocation::FnDef(first) = path[0];
                        // path[0].successors filter return
                        let src_idx = self.graph.add_node((*src_instance_id, first));
                        let target_idx =
                            self.graph.add_node((*target_instance_id, Location::START));
                        self.graph.add_edge(src_idx, target_idx, Vec::new());
                } else {
                    let _first = lockguard_instance_graph
                        .index_to_instance_id(callchain[0])
                        .unwrap();
                    let _last = lockguard_instance_graph
                        .index_to_instance_id(callchain[callchain.len() - 1])
                        .unwrap();
                    // callgraph.graph.find_edge(src_instance_id, first);
                }
            }
        }
    }

    fn collect_return_resume<'tcx>(
        instance: &Instance<'tcx>,
        tcx: TyCtxt<'tcx>,
    ) -> ReturnResumeLocationCollector {
        let body = tcx.instance_mir(instance.def);
        let mut collector = ReturnResumeLocationCollector::new();
        collector.visit_body(body);
        collector
    }
}

pub struct ReturnResumeLocationCollector {
    returns: SmallVec<[Location; 4]>,
    resumes: SmallVec<[Location; 4]>,
}

impl ReturnResumeLocationCollector {
    pub fn new() -> Self {
        Self {
            returns: Default::default(),
            resumes: Default::default(),
        }
    }
}

impl<'tcx> Visitor<'tcx> for ReturnResumeLocationCollector {
    fn visit_terminator(&mut self, terminator: &Terminator<'tcx>, location: Location) {
        match terminator.kind {
            TerminatorKind::Return => {
                self.returns.push(location);
            }
            TerminatorKind::Resume => {
                self.resumes.push(location);
            }
            _ => {}
        }
    }
}

// collect lockguards from callgraph: get inst -> lockguards
// worklist SCC of callgraph if exists inst in SCC s.t. inst contains lockguard
// contexts of insts are empty
// for instance in worklist SCC
// analyze instance:
// if instance contains lockguards
//  forall locations: state[locations] = empty
//  state[START] = contexts[instance]
//  for loc in worklist Locations:
//    for succ in successors of location:
//       check(state[loc] \ kill[loc], gen[loc])
//       new = state[loc] \ kill[loc] U gen[loc]
//       if new != state[loc]:
//         worklist.push(new)
//         if succ is Callsite(callee):
//           new_ctxt = contexts[callee] U new
//           if new_ctxt != contexts[callee]:
//             worklist<instance> push(callee)
//             contexts[callee] = new_ctxt
// else:
//    for callsite(callee) in instance
//       new_ctxt = contexts[callee] U contexts[callee]
//       if new_ctxt != contexts[callee]:
//         worklist<instance> push(callee)
//         context_callee = new_ctxt

#[derive(Clone, Debug)]
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

impl Default for LiveLockGuards {
    fn default() -> Self {
        Self(Default::default())
    }
}

pub struct DeadLockDetector<'tcx> {
    tcx: TyCtxt<'tcx>,
    param_env: ParamEnv<'tcx>,
    pub lockguard_relations: FxHashSet<(LockGuardId, LockGuardId)>,
}

impl<'tcx> DeadLockDetector<'tcx> {
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
            // only analyze local fn
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

    pub fn detect(&mut self, callgraph: &CallGraph<'tcx> /* pointer_analysis */) {
        let lockguards = self.collect_lockguards(callgraph);
        // println!("lockguards: {:?}", lockguards.keys());
        // collect wcc (weak connected components)

        // for root in callgraph.weak_connected_component_roots() {
        // collect all the nodes in wcc, starting with root
        let mut worklist = VecDeque::new();
        for (id, _) in callgraph.graph.node_references() {
            worklist.push_back(id);
        }
        // callgraph.bfs_visit(root, |id| worklist.push_back(id));
        // println!("worklist: {:?}", worklist);
        // skip wcc without lockguards
        // if !worklist.iter().any(|id| lockguards.contains_key(&id)) {
        // 	continue;
        // }
        // contexts record live lockguards before calling each instance, init empty
        let mut contexts = worklist
            .iter()
            .copied()
            .map(|id| (id, LiveLockGuards::default()))
            .collect::<FxHashMap<_, _>>();
        // the fixed-point algorithm
        while let Some(id) = worklist.pop_front() {
            if let Some(lockguard_info) = lockguards.get(&id) {
                // interproc gen kill on each instance
                let instance = callgraph.index_to_instance(id).unwrap();
                let body = self.tcx.instance_mir(instance.def);
                let context = contexts[&id].clone();
                // println!("instance: {:?}", instance);
                let states = self.intraproc_gen_kill(body, &context, lockguard_info);
                // println!(
                //     "states: {:#?}",
                //     states.iter().collect::<std::collections::BTreeMap<_, _>>()
                // );
                for edge in callgraph.graph.edges_directed(id, Direction::Outgoing) {
                    let callee = edge.target();
                    let callsite = edge.weight();
                    for callsite_loc in callsite {
                        let CallSiteLocation::FnDef(loc) = callsite_loc;
                        let callsite_state = states[&loc].clone();
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

        // lockguard_relations
        // map to lockguard info
        let mut info = FxHashMap::default();
        for (_, map) in lockguards.into_iter() {
            info.extend(map.into_iter());
        }
        // for (pred, curr) in &self.lockguard_relations {
        // 	// println!("{:?}, {:?}", info[&pred], info[&curr]);
        // 	if info[&pred].lockguard_ty.deadlock_with(&info[&curr].lockguard_ty) {
        // 		println!("deadlock: {:?}\n{:?}", info[&pred], info[&curr]);
        // 	} else if info[&pred].lockguard_ty.may_deadlock_with(&info[&curr].lockguard_ty) {
        // 		println!("may deadlock: {:?}\n{:?}", info[&pred], info[&curr]);
        // 	}
        // }
        println!("{:#?}", info);
        self.deadlock_candidate_graph(&info, callgraph);
        // }
    }

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

    // LockGuard Relation: Cartesian product of LockGuard. `pre` not dropped when `curr` becomes live
    // pred \ curr -> curr
    fn check_lockguard_relation(
        pred: &LiveLockGuards,
        curr: &LiveLockGuards,
    ) -> FxHashSet<(LockGuardId, LockGuardId)> {
        let mut relation = FxHashSet::default();
        for new in pred
            .raw_lockguard_ids()
            .difference(&curr.raw_lockguard_ids())
        {
            for c in curr.raw_lockguard_ids() {
                relation.insert((*new, *c));
            }
        }
        relation
    }

    fn apply_gen_kill(
        state: &mut LiveLockGuards,
        gen: Option<&LiveLockGuards>,
        kill: Option<&LiveLockGuards>,
    ) -> FxHashSet<(LockGuardId, LockGuardId)> {
        // first kill, then gen
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

    // Graph G
    // for each relation(a, b)
    //   for each relation(c, d)
    //     if b.deadlock_with(c)
    //       G.add_edge (a,b) -> (c,d)
    // for each cycle in G
    //   add cycle to deadlock candidate
    // for each deadlock candidate
    //   for each edge(a, b) in candidate
    //     verify a b from the same lock
    //   if all edges pass verification
    //     report deadlock candidate as deadlock
    fn deadlock_candidate_graph(
        &self,
        lockguards: &LockGuardMap<'tcx>,
        callgraph: &CallGraph<'tcx>,
    ) {
        let mut reports = Vec::new();
        // TODO(boqin): extract this graph
        let mut conflictlock_graph = ConflictLockGraph::new();
        let mut relation_to_nodes = FxHashMap::default();
        println!(
            "lockguard_relations: {:#?}",
            self.lockguard_relations
                .iter()
                .map(|(a, b)| (&lockguards[a].span, &lockguards[b].span))
                .collect::<Vec<_>>()
        );
        let mut alias_analysis = AliasAnalysis::new(self.tcx, callgraph);
        // detect doublelock:
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
                    println!("{:?}", report);
                    reports.push(report);
                }
                _ => {
                    // if unlikely doublelock, add the pair into graph to check conflictlock
                    let node = conflictlock_graph.add_node((*a, *b));
                    relation_to_nodes.insert((*a, *b), node);
                }
            }
        }
        // detect conflictlock:
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
        conflictlock_graph.dot();

        // debug print back_edges
        // let lockguard_id_to_span = |id: &LockGuardId| lockguards[id].span;
        // let relation_id_to_lockguard_id_pair = |relation_id: NodeIndex | graph.node_weight(relation_id).unwrap();
        // let to_span = |id: NodeIndex| {
        //     let (a, b) = relation_id_to_lockguard_id_pair(id);
        //     (lockguard_id_to_span(a), lockguard_id_to_span(b))
        // };
        // println!("back_edges: {:#?}", back_edges.iter().map(|(relation_id1, relation_id2)| (to_span(*relation_id1), to_span(*relation_id2))).collect::<Vec<_>>());

        let cycle_paths = conflictlock_graph.cycle_paths();
        println!("cycle_paths: {:#?}", cycle_paths);
        // map Vec<RelationId> to Vec<(LockGuardId, LockGuardId)> then to Vec<DeadlockDiagnosis>
        let diagnosis = cycle_paths
            .into_iter()
            .map(|path| {
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
                println!("{:?}", report);
                reports.push(report);
            })
            .collect::<Vec<_>>();
        // let cycle_paths = cycle_paths.into_iter().map(|vec| {
        //     vec.into_iter()
        //         .map(|idx| graph.node_weight(idx).unwrap())
        //         .collect::<Vec<_>>()
        // });

        // (LockGuardId, LockGuardId) -> ((LockGuardTy, Span), (LockGuardTy, Span), [[CallSite]])
        // let diagnosis = cycle_paths
        //     .map(|path| {
        //         path.into_iter().map(|(a, b)| {
        //             let (ty1, span1, ty2, span2) = (
        //                 format!("{:?}", lockguards[&a].lockguard_ty),
        //                 format!("{:?}", lockguards[&a].span),
        //                 format!("{:?}", lockguards[&b].lockguard_ty),
        //                 format!("{:?}", lockguards[&b].span),
        //             );
        //             let callchains = if a.instance_id == b.instance_id {
        //                 vec![]
        //             } else {
        //                 let paths = callgraph.all_simple_paths(a.instance_id, b.instance_id);
        //                 paths
        //                     .into_iter()
        //                     .map(|vec| {
        //                         vec.iter()
        //                             .zip(vec.iter().skip(1))
        //                             .map(|(instance_id1, instance_id2)| {
        //                                 let instance1 =
        //                                     callgraph.index_to_instance(*instance_id1).unwrap();
        //                                 let body1 = self.tcx.instance_mir(instance1.def);
        //                                 let callsites = callgraph
        //                                     .callsites(*instance_id1, *instance_id2)
        //                                     .unwrap();
        //                                 let callsites = callsites
        //                                     .into_iter()
        //                                     .map(|location| {
        //                                         let CallSiteLocation::FnDef(location) =
        //                                             location;
        //                                         format!(
        //                                             "{:?}",
        //                                             body1.source_info(location).span
        //                                         )
        //                                     })
        //                                     .collect::<Vec<_>>();
        //                                 callsites
        //                             })
        //                             .collect::<Vec<_>>()
        //                     })
        //                     .collect::<Vec<_>>()
        //             };
        //             ((ty1, span1), (ty2, span2), callchains)
        //         }).collect::<Vec<_>>()
        //     })
        //     .collect::<Vec<_>>();
        println!("{:#?}", diagnosis);

        // (iid1: id1.instance_id, iid2: id2.instance_id)
        // callgaph iid1 -> instance, -> def_id
        // callchain
        //
        // let fn_names = path
        //     .iter()
        //     .map(|instance_id| {
        //         let def_id =
        //             callgraph.index_to_instance(*instance_id).unwrap().def_id();
        //         self.tcx.def_path_debug_str(def_id)
        //     })
        //     .collect::<Vec<_>>();
        // println!("{:?}", fn_names);
        // let mut callsite_spans = vec![vec![format!("{:?}", span1)]];
        // let mut callsite_spans = Vec::new();
        // callsite_spans.extend(path.iter().zip(path.iter().skip(1)).map(
        //     |(instance_id1, instance_id2)| {
        //         let body1 = self.tcx.instance_mir(
        //             callgraph.index_to_instance(*instance_id1).unwrap().def,
        //         );
        //         let callsite_locations =
        //             callgraph.callsites(*instance_id1, *instance_id2).unwrap();
        //         callsite_locations
        //             .into_iter()
        //             .map(|location| {
        //                 let CallSiteLocation::FnDef(location) = location;
        //                 format!("{:?}", body1.source_info(location).span)
        //             })
        //             .collect::<Vec<_>>()
        //     },
        // ));
        // let callchain = callsite_spans
        //     .into_iter()
        //     .zip(fn_names.into_iter())
        //     .collect::<Vec<_>>();
        // callchains.push(callchain);
        // }
        // println!("callchains: {:?}", callchains);
        // let mut relation_loops = Vec::new();
        // for path in paths {
        //  	let relation_loop: Vec<_> = path.into_iter().map(|n| graph.node_weight(n).unwrap()).map(|(a, b)| (&lockguards[a].span, &lockguards[b].span)).collect();
        //  	println!("deadlock path: {:#?}", relation_loop);
        //     //  let mut callchains = Vec::new();

        // }
        // }
    }
}

// check deadlock possibility.
// for two lockguards, first check if their types may deadlock;
// if so, then check if they may alias.
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

// generate doublelock diagnosis
fn diagnose_doublelock<'tcx>(
    a: &LockGuardId,
    b: &LockGuardId,
    lockguards: &LockGuardMap<'tcx>,
    callgraph: &CallGraph<'tcx>,
    tcx: TyCtxt<'tcx>,
) -> report::DeadlockDiagnosis {
    diagnose_one_relation(a, b, lockguards, callgraph, tcx)
}

// find all the callchains: source -> target
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

// find the diagnosis info for relation(a, b), including a's ty & span, b's ty & span, and callchains.
fn diagnose_one_relation<'tcx>(
    a: &LockGuardId,
    b: &LockGuardId,
    lockguards: &LockGuardMap<'tcx>,
    callgraph: &CallGraph<'tcx>,
    tcx: TyCtxt<'tcx>,
) -> DeadlockDiagnosis {
    let a_info = &lockguards[&a];
    let b_info = &lockguards[&b];
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

type RelationId = NodeIndex;
// record deadlock possibility of b and c between relation(a, b) to relation(c, d)
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
                    .zip(path.iter().skip(1).chain(path.iter().next()))
                    .map(|(a, b)| (*a, *b))
                    .collect::<FxHashSet<_>>();
                if !edge_sets.contains(&set) {
                    edge_sets.push(set);
                    dedup.push(path);
                    // break;
                    // TODO(boqin): remove break
                }
            }
        }
        dedup
    }

    /// Print the ConflictGraph in dot format.
    pub fn dot(&self) {
        println!(
            "{:?}",
            Dot::with_config(&self.graph, &[Config::GraphContentOnly])
        );
    }
}
