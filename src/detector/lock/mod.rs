//! LockBugDetector: detects doublelock and conflictlock
//! bfs each WCC of callgraph
//! if instance contains lockguards then make it vertex
//! else push it to path
//! so as to build a new LockGuardCallGraph
extern crate rustc_hash;
extern crate rustc_data_structures;

use crate::analysis::callgraph::{CallGraph, CallSiteLocation, InstanceId};
use crate::interest::concurrency::lock::{LockGuardId, LockGuardInfo, LockGuardMap, LockGuardCollector};

use petgraph::dot::{Config, Dot};
use petgraph::graph::NodeIndex;
use petgraph::unionfind::UnionFind;
use petgraph::visit::{Bfs, EdgeRef, IntoNodeReferences, NodeIndexable, Walker, IntoEdgesDirected};
use petgraph::{Directed, Direction, Graph};

use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{BasicBlock, Body, Location, Terminator, TerminatorKind};
use rustc_middle::ty::{self, Instance, ParamEnv, TyCtxt};
use rustc_hash::{FxHashMap, FxHashSet};
use rustc_data_structures::graph::WithSuccessors;

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
	fn new(start: InstanceId, callgraph: &'a CallGraph<'tcx>, lockguard_instances: &'b FxHashMap<InstanceId, LockGuardMap<'tcx>>, lockguard_instance_ids: &'c FxHashMap<InstanceId, NodeIndex>) -> Self {
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

	pub fn analyze(&mut self, callgraph: &CallGraph<'tcx>, tcx: TyCtxt<'tcx>, param_env: ParamEnv<'tcx>) {
		// filter local instances with lockguards in it.
		let mut lockguard_instances = FxHashMap::default();
		let mut lockguard_instance_ids = FxHashMap::default();
		for (instance_id, instance) in callgraph.graph.node_references() {
			if !instance.def_id().is_local() {
				continue;
			}
			let body = tcx.instance_mir(instance.def);
			let mut lockguard_collector = LockGuardCollector::new(instance_id, instance, body, tcx, param_env);
			lockguard_collector.analyze();
			if !lockguard_collector.lockguards.is_empty() {
				let lockguard_instance_id = self.graph.add_node(instance_id);
				lockguard_instance_ids.insert(instance_id, lockguard_instance_id);
				println!("{:?}: {:?}", instance_id, instance);
				lockguard_instances.insert(instance_id, lockguard_collector.lockguards);
			}
		}
		// find all paths between the instances with lockguards
		for (instance_id, _) in lockguard_instances.iter() {
			self.dfs_visit_lockguard_instance(*instance_id, callgraph, &lockguard_instances, &lockguard_instance_ids);
		}
		self.lockguard_instances = lockguard_instances;
	}

	// find all paths between the starting lockguard instance and the first met lockguard instance
	fn dfs_visit_lockguard_instance(&mut self, start: InstanceId, callgraph: &CallGraph<'tcx>, lockguard_instances: &FxHashMap<InstanceId, LockGuardMap<'tcx>>, lockguard_instance_ids: &FxHashMap<InstanceId, NodeIndex>) {
		let mut visited = FxHashSet::default();
		let mut path = Vec::new();
		let context = LockGuardInstanceContext::new(start, callgraph, lockguard_instances, lockguard_instance_ids);
		self.dfs_visit_lockguard_instance_recur(start, &mut visited, &mut path, &context);
	}

	fn dfs_visit_lockguard_instance_recur(&mut self, curr: InstanceId, visited: &mut FxHashSet<InstanceId>, path: &mut Vec<InstanceId>, context: &LockGuardInstanceContext) {
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
	fn analyze<'tcx>(&mut self, lockguard_instance_graph: &LockGuardInstanceGraph<'tcx>, callgraph: &CallGraph<'tcx>, tcx: TyCtxt<'tcx>) {
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
			let src_instance = callgraph.index_to_instance(*src_instance_id).unwrap();
			for edge in lockguard_instance_graph.graph.edges_directed(node_idx, petgraph::Direction::Outgoing) {
				let target_instance_id = lockguard_instance_graph.index_to_instance_id(edge.target()).unwrap();
				let target_instance = callgraph.index_to_instance(*target_instance_id).unwrap();
				let callchain = edge.weight();
				if callchain.is_empty() {
					// src_instance directly connects to target_instance
					let edge = callgraph.graph.find_edge(*src_instance_id, *target_instance_id).unwrap();
					let path = callgraph.graph.edge_weight(edge).unwrap();
					if let CallSiteLocation::FnDef(first) = path[0] {
						// path[0].successors filter return 
						let src_idx = self.graph.add_node((*src_instance_id, first));
						let target_idx = self.graph.add_node((*target_instance_id, Location::START));
						self.graph.add_edge(src_idx, target_idx, Vec::new());
					}
				} else {
					let first = lockguard_instance_graph.index_to_instance_id(callchain[0]).unwrap();
					let last = lockguard_instance_graph.index_to_instance_id(callchain[callchain.len()-1]).unwrap();
					// callgraph.graph.find_edge(src_instance_id, first);
				}
			}
		}	
	
	}

	fn collect_return_resume<'tcx>(instance: &Instance<'tcx>, tcx: TyCtxt<'tcx>) -> ReturnResumeLocationCollector {
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
			TerminatorKind::Return => { self.returns.push(location); },
			TerminatorKind::Resume => { self.resumes.push(location); },
			_ => {},
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

	fn collect_lockguards(&self, callgraph: &CallGraph<'tcx>) -> FxHashMap<InstanceId, LockGuardMap<'tcx>> {
		let mut lockguards = FxHashMap::default();
		for (instance_id, instance) in callgraph.graph.node_references() {
			// only analyze local fn
			if !instance.def_id().is_local() {
				continue;
			}
			let body = self.tcx.instance_mir(instance.def);
			let mut lockguard_collector = LockGuardCollector::new(instance_id, instance, body, self.tcx, self.param_env);
			lockguard_collector.analyze();
			if !lockguard_collector.lockguards.is_empty() {
				lockguards.insert(instance_id, lockguard_collector.lockguards);
			}
		}
		lockguards
	}

	pub fn detect(&mut self, callgraph: &CallGraph<'tcx>, /* pointer_analysis */) {	
		let lockguards = self.collect_lockguards(callgraph);
		println!("lockguards: {:?}", lockguards.keys());
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
			let mut contexts = worklist.iter().copied().map(|id| (id, LiveLockGuards::default())).collect::<FxHashMap<_, _>>();
			// the fixed-point algorithm
			while let Some(id) = worklist.pop_front() {
				if let Some(lockguard_info) = lockguards.get(&id) {
					// interproc gen kill on each instance
					let instance = callgraph.index_to_instance(id).unwrap();
					let body = self.tcx.instance_mir(instance.def);
					let context = contexts[&id].clone();
					let states = self.intraproc_gen_kill(body, &context, lockguard_info);
					for edge in callgraph.graph.edges_directed(id, Direction::Outgoing) {
						let callee = edge.target();
						let callsite = edge.weight();
						for callsite_loc in callsite {
							let CallSiteLocation::FnDef(loc) = callsite_loc;
							let callsite_state = states[&loc].clone();
							let changed = contexts.get_mut(&callee).unwrap().union_in_place(callsite_state);
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
			for (pred, curr) in &self.lockguard_relations {
				// println!("{:?}, {:?}", info[&pred], info[&curr]);
				if info[&pred].lockguard_ty.deadlock_with(&info[&curr].lockguard_ty) {
					println!("deadlock: {:?}\n{:?}", info[&pred], info[&curr]);
				} else if info[&pred].lockguard_ty.may_deadlock_with(&info[&curr].lockguard_ty) {
					println!("may deadlock: {:?}\n{:?}", info[&pred], info[&curr]);
				}
			}
		// }
	}

	fn location_to_live_lockguards(lockguard_map: &LockGuardMap<'tcx>) -> (FxHashMap<Location, LiveLockGuards>, FxHashMap<Location, LiveLockGuards>) {
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
	fn check_lockguard_relation(pred: &LiveLockGuards, curr: &LiveLockGuards) -> FxHashSet<(LockGuardId, LockGuardId)> {
		let mut relation = FxHashSet::default();
		for p in pred.raw_lockguard_ids() {
			for c in curr.raw_lockguard_ids() {
				relation.insert((*p, *c));
			}
		}
		relation
	}

	fn intraproc_gen_kill(&mut self, body: &'tcx Body<'tcx>, context: &LiveLockGuards, lockguard_info: &LockGuardMap<'tcx>) -> FxHashMap<Location, LiveLockGuards> {
		let (gen_map, kill_map) = Self::location_to_live_lockguards(lockguard_info);
		let mut worklist: VecDeque<Location> = Default::default();
		for (bb, bb_data) in body.basic_blocks().iter_enumerated() {
			for stmt_idx in 0..bb_data.statements.len() + 1 {
				worklist.push_back(Location { block: bb, statement_index: stmt_idx });
			}
		}
		let mut states: FxHashMap<Location, LiveLockGuards> = worklist.iter().copied().map(|loc| (loc, LiveLockGuards::default())).collect();
		*states.get_mut(&Location::START).unwrap() = context.clone();
		while let Some(loc) = worklist.pop_front() {
			let mut after = states[&loc].clone();
			// first kill, then gen
			if let Some(kill) = kill_map.get(&loc) {
				after.difference_in_place(kill);
			}
			if let Some(gen) = gen_map.get(&loc) {
				after.union_in_place(gen.clone());
			}
			let term_loc = body.terminator_loc(loc.block);
			if loc != term_loc {
				// if not terminator
				let succ = loc.successor_within_block();
				// check lockguard relations
				let relation = Self::check_lockguard_relation(&after, &states[&succ]);
				self.lockguard_relations.extend(relation.into_iter());
				// union and reprocess if changed
				let changed = states.get_mut(&succ).unwrap().union_in_place(after);
				if changed {
					worklist.push_back(succ);	
				}
			} else {
				// if is terminator
				for succ_bb in body.successors(loc.block) {
					let succ = Location { block: succ_bb, statement_index: 0 };
					// check lockguard relations
					let relation = Self::check_lockguard_relation(&after, &states[&succ]);
					self.lockguard_relations.extend(relation.into_iter());
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
}

// WCC
// for each WCC
// search for WCC
// if worklist contains A then
// visited = 0
// worklist
// visited
