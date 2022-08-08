//! Points-to analysis.
//! It checks if two pointers may point to the same memory cell.
//! It depends on `CallGraph` and provides support for detectors.
extern crate rustc_hash;
extern crate rustc_hir;

use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};
use rustc_hir::def_id::DefId;
use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{
    Body, Constant, ConstantKind, Local, Location, Operand, Place, PlaceRef, ProjectionElem,
    Rvalue, Statement, StatementKind, Terminator, TerminatorKind,
};
use rustc_middle::ty::{Instance, TyCtxt};

use petgraph::dot::{Config, Dot};
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use petgraph::{Directed, Direction, Graph};

use crate::analysis::callgraph::{CallGraph, CallGraphNode, InstanceId};
use crate::interest::concurrency::lock::LockGuardId;

/// Field-sensitive intra-procedural Andersen pointer analysis.
/// <https://helloworld.pub/program-analysis-andersen-pointer-analysis-algorithm-based-on-svf.html>
/// 1. collect constraints from MIR to build a `ConstraintGraph`
/// 2. adopt a fixed-point algorithm to update `ConstraintGraph` and points-to info
pub struct Andersen<'a, 'tcx> {
    body: &'a Body<'tcx>,
    pts: PointsToMap<'tcx>,
}

pub type PointsToMap<'tcx> = FxHashMap<ConstraintNode<'tcx>, FxHashSet<ConstraintNode<'tcx>>>;

impl<'a, 'tcx> Andersen<'a, 'tcx> {
    pub fn new(body: &'a Body<'tcx>) -> Self {
        Self {
            body,
            pts: Default::default(),
        }
    }

    pub fn analyze(&mut self) {
        let mut collector = ConstraintGraphCollector::new();
        collector.visit_body(self.body);
        let mut graph = collector.finish();
        let mut worklist = VecDeque::new();
        // alloc: place = alloc
        for node in graph.nodes() {
            match node {
                ConstraintNode::Place(place) => {
                    graph.add_alloc(place);
                }
                ConstraintNode::Constant(constant) => {
                    graph.add_constant(constant);
                    // For constant C, track *C.
                    worklist.push_back(ConstraintNode::ConstantDeref(constant));
                }
                _ => {}
            }
            worklist.push_back(node);
        }

        // address: target = &source
        for (source, target, weight) in graph.edges() {
            if weight == ConstraintEdge::Address {
                self.pts.entry(target).or_default().insert(source);
                worklist.push_back(target);
            }
        }

        while let Some(node) = worklist.pop_front() {
            if !self.pts.contains_key(&node) {
                continue;
            }
            for o in self.pts.get(&node).unwrap() {
                // store: *node = source
                for source in graph.store_sources(&node) {
                    if graph.insert_edge(source, *o, ConstraintEdge::Copy) {
                        worklist.push_back(source);
                    }
                }
                // load: target = *node
                for target in graph.load_targets(&node) {
                    if graph.insert_edge(*o, target, ConstraintEdge::Copy) {
                        worklist.push_back(*o);
                    }
                }
            }
            // copy: target = node
            for target in graph.copy_targets(&node) {
                if self.union_pts(&target, &node) {
                    worklist.push_back(target);
                }
            }
        }
    }

    /// pts(target) = pts(target) U pts(source), return true if pts(target) changed
    fn union_pts(&mut self, target: &ConstraintNode<'tcx>, source: &ConstraintNode<'tcx>) -> bool {
        // skip Alloc target
        if matches!(target, ConstraintNode::Alloc(_)) {
            return false;
        }
        let old_len = self.pts.get(target).unwrap().len();
        let source_pts = self.pts.get(source).unwrap().clone();
        let target_pts = self.pts.get_mut(target).unwrap();
        target_pts.extend(source_pts.into_iter());
        old_len != target_pts.len()
    }

    pub fn finish(self) -> FxHashMap<ConstraintNode<'tcx>, FxHashSet<ConstraintNode<'tcx>>> {
        self.pts
    }
}

/// `ConstraintNode` represents a memory cell, denoted by `Place` in MIR.
/// A `Place` encompasses `Local` and `[ProjectionElem]`, `ProjectionElem`
/// can be a `Field`, `Index`, etc.
/// Since there is no `Alloc` in MIR, we cannot use locations of `Alloc`
/// to uniquely identify the allocation of a memory cell.
/// Instead, we use `Place` itself to represent its allocation,
/// namely, forall Places(p), Alloc(p)--|address|-->Place(p).
/// `Constant` appears on right-hand in assignments like `Place = Constant(c)`.
/// To enable the propagtion of points-to info for `Constant`,
/// we introduce `ConstantDeref` to denote the points-to node of `Constant`,
/// namely, forall Constant(c), Constant(c)--|address|-->ConstantDeref(c).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstraintNode<'tcx> {
    Alloc(PlaceRef<'tcx>),
    Place(PlaceRef<'tcx>),
    Constant(ConstantKind<'tcx>),
    ConstantDeref(ConstantKind<'tcx>),
}

/// The assignments in MIR with default `mir-opt-level` (level 1) are simplified
/// to the following four kinds:
///
/// | Edge   | Assignment | Constraint |
/// | ------ | ---------- | ----------
/// | Address| a = &b     | pts(a)∋b
/// | Copy   | a = b      | pts(a)⊇pts(b)
/// | Load   | a = *b     | ∀o∈pts(b), pts(a)⊇pts(o)
/// | Store  | *a = b     | ∀o∈pts(a), pts(o)⊇pts(b)
///
/// Note that other forms like a = &((*b).0) exists but is uncommon.
/// This is the case when b is an arg. We just treat (*b).0
/// as the mem cell and do not further dereference it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConstraintEdge {
    Address,
    Copy,
    Load,
    Store,
}

enum AccessPattern<'tcx> {
    Ref(PlaceRef<'tcx>),
    Indirect(PlaceRef<'tcx>),
    Direct(PlaceRef<'tcx>),
    Constant(ConstantKind<'tcx>),
}

#[derive(Default)]
struct ConstraintGraph<'tcx> {
    graph: Graph<ConstraintNode<'tcx>, ConstraintEdge, Directed>,
    node_map: FxHashMap<ConstraintNode<'tcx>, NodeIndex>,
}

impl<'tcx> ConstraintGraph<'tcx> {
    fn get_or_insert_node(&mut self, node: ConstraintNode<'tcx>) -> NodeIndex {
        if let Some(idx) = self.node_map.get(&node) {
            *idx
        } else {
            let idx = self.graph.add_node(node);
            self.node_map.insert(node, idx);
            idx
        }
    }

    fn get_node(&self, node: &ConstraintNode<'tcx>) -> Option<NodeIndex> {
        self.node_map.get(node).copied()
    }

    fn add_alloc(&mut self, place: PlaceRef<'tcx>) {
        let lhs = ConstraintNode::Place(place);
        let rhs = ConstraintNode::Alloc(place);
        let lhs = self.get_or_insert_node(lhs);
        let rhs = self.get_or_insert_node(rhs);
        self.graph.add_edge(rhs, lhs, ConstraintEdge::Address);
    }

    fn add_constant(&mut self, constant: ConstantKind<'tcx>) {
        let lhs = ConstraintNode::Constant(constant);
        let rhs = ConstraintNode::ConstantDeref(constant);
        let lhs = self.get_or_insert_node(lhs);
        let rhs = self.get_or_insert_node(rhs);
        self.graph.add_edge(rhs, lhs, ConstraintEdge::Address);
        // For a constant C, there may be deref like *C, **C, ***C, ... in a real program.
        // For simplicity, we only track *C, and treat **C, ***C, ... the same as *C.
        self.graph.add_edge(rhs, rhs, ConstraintEdge::Address);
    }

    fn add_address(&mut self, lhs: PlaceRef<'tcx>, rhs: PlaceRef<'tcx>) {
        let lhs = ConstraintNode::Place(lhs);
        let rhs = ConstraintNode::Place(rhs);
        let lhs = self.get_or_insert_node(lhs);
        let rhs = self.get_or_insert_node(rhs);
        self.graph.add_edge(rhs, lhs, ConstraintEdge::Address);
    }

    fn add_copy(&mut self, lhs: PlaceRef<'tcx>, rhs: PlaceRef<'tcx>) {
        let lhs = ConstraintNode::Place(lhs);
        let rhs = ConstraintNode::Place(rhs);
        let lhs = self.get_or_insert_node(lhs);
        let rhs = self.get_or_insert_node(rhs);
        self.graph.add_edge(rhs, lhs, ConstraintEdge::Copy);
    }

    fn add_copy_constant(&mut self, lhs: PlaceRef<'tcx>, rhs: ConstantKind<'tcx>) {
        let lhs = ConstraintNode::Place(lhs);
        let rhs = ConstraintNode::Constant(rhs);
        let lhs = self.get_or_insert_node(lhs);
        let rhs = self.get_or_insert_node(rhs);
        self.graph.add_edge(rhs, lhs, ConstraintEdge::Copy);
    }

    fn add_load(&mut self, lhs: PlaceRef<'tcx>, rhs: PlaceRef<'tcx>) {
        let lhs = ConstraintNode::Place(lhs);
        let rhs = ConstraintNode::Place(rhs);
        let lhs = self.get_or_insert_node(lhs);
        let rhs = self.get_or_insert_node(rhs);
        self.graph.add_edge(rhs, lhs, ConstraintEdge::Load);
    }

    fn add_store(&mut self, lhs: PlaceRef<'tcx>, rhs: PlaceRef<'tcx>) {
        let lhs = ConstraintNode::Place(lhs);
        let rhs = ConstraintNode::Place(rhs);
        let lhs = self.get_or_insert_node(lhs);
        let rhs = self.get_or_insert_node(rhs);
        self.graph.add_edge(rhs, lhs, ConstraintEdge::Store);
    }

    fn add_store_constant(&mut self, lhs: PlaceRef<'tcx>, rhs: ConstantKind<'tcx>) {
        let lhs = ConstraintNode::Place(lhs);
        let rhs = ConstraintNode::Constant(rhs);
        let lhs = self.get_or_insert_node(lhs);
        let rhs = self.get_or_insert_node(rhs);
        self.graph.add_edge(rhs, lhs, ConstraintEdge::Store);
    }

    fn nodes(&self) -> Vec<ConstraintNode<'tcx>> {
        self.node_map.keys().copied().collect::<_>()
    }

    fn edges(&self) -> Vec<(ConstraintNode<'tcx>, ConstraintNode<'tcx>, ConstraintEdge)> {
        let mut v = Vec::new();
        for edge in self.graph.edge_references() {
            let source = self.graph.node_weight(edge.source()).copied().unwrap();
            let target = self.graph.node_weight(edge.target()).copied().unwrap();
            let weight = *edge.weight();
            v.push((source, target, weight));
        }
        v
    }

    /// *lhs = ?
    /// ?--|store|-->lhs
    fn store_sources(&self, lhs: &ConstraintNode<'tcx>) -> Vec<ConstraintNode<'tcx>> {
        let lhs = self.get_node(lhs).unwrap();
        let mut sources = Vec::new();
        for edge in self.graph.edges_directed(lhs, Direction::Incoming) {
            if *edge.weight() == ConstraintEdge::Store {
                let source = self.graph.node_weight(edge.source()).copied().unwrap();
                sources.push(source);
            }
        }
        sources
    }

    /// ? = *rhs
    /// rhs--|load|-->?
    fn load_targets(&self, rhs: &ConstraintNode<'tcx>) -> Vec<ConstraintNode<'tcx>> {
        let rhs = self.get_node(rhs).unwrap();
        let mut targets = Vec::new();
        for edge in self.graph.edges_directed(rhs, Direction::Outgoing) {
            if *edge.weight() == ConstraintEdge::Load {
                let target = self.graph.node_weight(edge.target()).copied().unwrap();
                targets.push(target);
            }
        }
        targets
    }

    /// ? = rhs
    /// rhs--|copy|-->?
    fn copy_targets(&self, rhs: &ConstraintNode<'tcx>) -> Vec<ConstraintNode<'tcx>> {
        let rhs = self.get_node(rhs).unwrap();
        let mut targets = Vec::new();
        for edge in self.graph.edges_directed(rhs, Direction::Outgoing) {
            if *edge.weight() == ConstraintEdge::Copy {
                let target = self.graph.node_weight(edge.target()).copied().unwrap();
                targets.push(target);
            }
        }
        targets
    }

    /// if edge `from--|weight|-->to` not exists,
    /// then add the edge and return true
    fn insert_edge(
        &mut self,
        from: ConstraintNode<'tcx>,
        to: ConstraintNode<'tcx>,
        weight: ConstraintEdge,
    ) -> bool {
        let from = self.get_node(&from).unwrap();
        let to = self.get_node(&to).unwrap();
        if let Some(edge) = self.graph.find_edge(from, to) {
            if let Some(w) = self.graph.edge_weight(edge) {
                if *w == weight {
                    return false;
                }
            }
        }
        self.graph.add_edge(from, to, weight);
        true
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

/// Generate `ConstraintGraph` by visiting MIR body.
struct ConstraintGraphCollector<'tcx> {
    graph: ConstraintGraph<'tcx>,
}

impl<'tcx> ConstraintGraphCollector<'tcx> {
    fn new() -> Self {
        Self {
            graph: ConstraintGraph::default(),
        }
    }

    fn process_assignment(&mut self, place: &Place<'tcx>, rvalue: &Rvalue<'tcx>) {
        let lhs_pattern = Self::process_place(place.as_ref());
        let rhs_pattern = Self::process_rvalue(rvalue);
        match (lhs_pattern, rhs_pattern) {
            // a = &b
            (AccessPattern::Direct(lhs), Some(AccessPattern::Ref(rhs))) => {
                self.graph.add_address(lhs, rhs);
            }
            // a = b
            (AccessPattern::Direct(lhs), Some(AccessPattern::Direct(rhs))) => {
                self.graph.add_copy(lhs, rhs);
            }
            // a = Constant
            (AccessPattern::Direct(lhs), Some(AccessPattern::Constant(rhs))) => {
                self.graph.add_copy_constant(lhs, rhs);
            }
            // a = *b
            (AccessPattern::Direct(lhs), Some(AccessPattern::Indirect(rhs))) => {
                self.graph.add_load(lhs, rhs);
            }
            // *a = b
            (AccessPattern::Indirect(lhs), Some(AccessPattern::Direct(rhs))) => {
                self.graph.add_store(lhs, rhs);
            }
            // *a = Constant
            (AccessPattern::Indirect(lhs), Some(AccessPattern::Constant(rhs))) => {
                self.graph.add_store_constant(lhs, rhs);
            }
            _ => {}
        }
    }

    fn process_place(place_ref: PlaceRef<'tcx>) -> AccessPattern<'tcx> {
        match place_ref {
            PlaceRef {
                local: l,
                projection: [ProjectionElem::Deref, ref remain @ ..],
            } => AccessPattern::Indirect(PlaceRef {
                local: l,
                projection: remain,
            }),
            _ => AccessPattern::Direct(place_ref),
        }
    }

    fn process_rvalue(rvalue: &Rvalue<'tcx>) -> Option<AccessPattern<'tcx>> {
        match rvalue {
            Rvalue::Use(operand) | Rvalue::Repeat(operand, _) | Rvalue::Cast(_, operand, _) => {
                match operand {
                    Operand::Move(place) | Operand::Copy(place) => {
                        Some(AccessPattern::Direct(place.as_ref()))
                    }
                    Operand::Constant(box Constant {
                        span: _,
                        user_ty: _,
                        literal,
                    }) => Some(AccessPattern::Constant(*literal)),
                }
            }
            // Regard `p = &*q` as `p = q`
            Rvalue::Ref(_, _, place) | Rvalue::AddressOf(_, place) => match place.as_ref() {
                PlaceRef {
                    local: l,
                    projection: [ProjectionElem::Deref, ref remain @ ..],
                } => Some(AccessPattern::Direct(PlaceRef {
                    local: l,
                    projection: remain,
                })),
                _ => Some(AccessPattern::Ref(place.as_ref())),
            },
            _ => None,
        }
    }

    fn process_call_arg_dest(&mut self, arg: PlaceRef<'tcx>, dest: PlaceRef<'tcx>) {
        self.graph.add_copy(dest, arg);
    }

    /// forall (p1, p2) where p1 is prefix of p1, add `p1 = p2`.
    /// e.g. Place1{local1, &[f0]}, Place2{local1, &[f0,f1]},
    /// since they have the same local
    /// and Place1.projection is prefix of Place2.projection,
    /// Add constraint `Place1 = Place2`.
    fn add_partial_copy(&mut self) {
        let nodes = self.graph.nodes();
        for (idx, n1) in nodes.iter().enumerate() {
            for n2 in nodes.iter().skip(idx + 1) {
                if let (ConstraintNode::Place(p1), ConstraintNode::Place(p2)) = (n1, n2) {
                    if p1.local == p2.local {
                        if p1.projection.len() > p2.projection.len() {
                            if &p1.projection[..p2.projection.len()] == p2.projection {
                                self.graph.add_copy(*p2, *p1);
                            }
                        } else {
                            if &p2.projection[..p1.projection.len()] == p1.projection {
                                self.graph.add_copy(*p1, *p2);
                            }
                        }
                    }
                }
            }
        }
    }

    fn finish(mut self) -> ConstraintGraph<'tcx> {
        self.add_partial_copy();
        self.graph
    }
}

impl<'tcx> Visitor<'tcx> for ConstraintGraphCollector<'tcx> {
    fn visit_statement(&mut self, statement: &Statement<'tcx>, _location: Location) {
        match &statement.kind {
            StatementKind::Assign(box (place, rvalue)) => {
                self.process_assignment(place, rvalue);
            }
            StatementKind::FakeRead(_)
            | StatementKind::SetDiscriminant { .. }
            | StatementKind::Deinit(_)
            | StatementKind::StorageLive(_)
            | StatementKind::StorageDead(_)
            | StatementKind::Retag(_, _)
            | StatementKind::AscribeUserType(_, _)
            | StatementKind::Coverage(_)
            | StatementKind::Nop
            | StatementKind::CopyNonOverlapping(_) => {}
        }
    }

    /// Heuristically assumes that
    /// for callsites like `destination = call fn(move args0)`,
    /// destination = args0
    fn visit_terminator(&mut self, terminator: &Terminator<'tcx>, _location: Location) {
        if let TerminatorKind::Call {
            func: _,
            args,
            destination,
            ..
        } = &terminator.kind
        {
            if let (&[Operand::Move(arg)], dest) = (args.as_slice(), destination) {
                self.process_call_arg_dest(arg.as_ref(), dest.as_ref());
            };
        }
    }
}

/// We do not use Must/May/Not since the pointer analysis implementation is overapproximate.
/// Instead, we use probably, possibly, unlikely as alias kinds.
/// We will report bugs of probaly and possibly kinds.
pub enum ApproximateAliasKind {
    Probably,
    Possibly,
    Unlikely,
    Unknown,
}

/// `AliasId` identifies a unique memory cell interprocedurally.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AliasId {
    pub instance_id: InstanceId,
    pub local: Local,
}

/// Basically, `AliasId` and `LockGuardId` share the same info.
impl std::convert::From<LockGuardId> for AliasId {
    fn from(lockguard_id: LockGuardId) -> Self {
        Self {
            instance_id: lockguard_id.instance_id,
            local: lockguard_id.local,
        }
    }
}

/// Alias analysis based on points-to info.
/// It answers if two memory cells alias with each other.
/// It performs an underlying points-to analysis if needed.
/// The points-to info will be cached into `pts` for future queries.
pub struct AliasAnalysis<'a, 'tcx> {
    tcx: TyCtxt<'tcx>,
    callgraph: &'a CallGraph<'tcx>,
    pts: FxHashMap<DefId, PointsToMap<'tcx>>,
}

impl<'a, 'tcx> AliasAnalysis<'a, 'tcx> {
    pub fn new(tcx: TyCtxt<'tcx>, callgraph: &'a CallGraph<'tcx>) -> Self {
        Self {
            tcx,
            callgraph,
            pts: Default::default(),
        }
    }

    /// Check if two memory cells alias with each other.
    pub fn alias(&mut self, aid1: AliasId, aid2: AliasId) -> ApproximateAliasKind {
        let AliasId {
            instance_id: id1,
            local: local1,
        } = aid1;
        let AliasId {
            instance_id: id2,
            local: local2,
        } = aid2;

        let instance1 = self
            .callgraph
            .index_to_instance(id1)
            .map(CallGraphNode::instance);
        let instance2 = self
            .callgraph
            .index_to_instance(id2)
            .map(CallGraphNode::instance);

        match (instance1, instance2) {
            (Some(instance1), Some(instance2)) => {
                let (def_id1, def_id2) = (instance1.def_id(), instance2.def_id());
                let node1 = ConstraintNode::Place(Place::from(local1).as_ref());
                let node2 = ConstraintNode::Place(Place::from(local2).as_ref());
                if def_id1 == def_id2 {
                    self.intraproc_alias(def_id1, instance1, &node1, &node2)
                } else {
                    self.interproc_alias(def_id1, instance1, &node1, def_id2, instance2, &node2)
                }
            }
            _ => ApproximateAliasKind::Unknown,
        }
    }

    /// Get the points-to info from cache `pts`.
    /// If not exists, then perform points-to analysis
    /// and add the obtained points-to info to cache.
    fn get_or_insert_pts(&mut self, def_id: DefId, body: &Body<'tcx>) -> &PointsToMap<'tcx> {
        if self.pts.contains_key(&def_id) {
            self.pts.get(&def_id).unwrap()
        } else {
            let mut pointer_analysis = Andersen::new(body);
            pointer_analysis.analyze();
            let pts = pointer_analysis.finish();
            self.pts.entry(def_id).or_insert(pts)
        }
    }

    /// Check alias of p1 and p2 if they are from the same fn.
    /// if pts(p1) intersect pts(p2) != empty then they probably alias else unlikely
    fn intraproc_alias(
        &mut self,
        def_id: DefId,
        instance: &Instance<'tcx>,
        node1: &ConstraintNode<'tcx>,
        node2: &ConstraintNode<'tcx>,
    ) -> ApproximateAliasKind {
        let body = self.tcx.instance_mir(instance.def);
        let points_to_map = self.get_or_insert_pts(def_id, body);
        if points_to_map[node1]
            .intersection(&points_to_map[node2])
            .next()
            .is_some()
        {
            ApproximateAliasKind::Probably
        } else {
            ApproximateAliasKind::Unlikely
        }
    }

    /// Check alias of p1 and p2 if they are from different fn.
    /// To avoid interproc alias analysis, we use heuristic assumption:
    /// if exists a1 in pts(p1) and a1 is Constant(c1) and
    ///    exists a2 in pts(p2) and a2 is Constant(c2)
    /// then
    ///    if c1 == c2 then probably alias else unlikely
    /// elif exists a1 in pts(p1) and a1.local in Args(fn1) and
    ///    exists a2 in pts(pt2) and a2.local in Args(fn2) and
    ///    a1.local.ty == a2.local.ty and
    ///    a1.projection = a2.projection
    /// then possible alias else unlikely
    fn interproc_alias(
        &mut self,
        def_id1: DefId,
        instance1: &Instance<'tcx>,
        node1: &ConstraintNode<'tcx>,
        def_id2: DefId,
        instance2: &Instance<'tcx>,
        node2: &ConstraintNode<'tcx>,
    ) -> ApproximateAliasKind {
        let body1 = self.tcx.instance_mir(instance1.def);
        let body2 = self.tcx.instance_mir(instance2.def);
        let points_to_map1 = self.get_or_insert_pts(def_id1, body1).clone();
        let points_to_map2 = self.get_or_insert_pts(def_id2, body2);
        let pts1 = &points_to_map1[node1];
        let pts2 = &points_to_map2[node2];
        let mut constants1 = pts1
            .iter()
            .filter(|node| matches!(node, &ConstraintNode::ConstantDeref(_)))
            .peekable();
        let mut constants2 = pts2
            .iter()
            .filter(|node| matches!(node, &ConstraintNode::ConstantDeref(_)))
            .peekable();
        if constants1.any(|c1| constants2.any(|c2| c2 == c1)) {
            return ApproximateAliasKind::Probably;
        } else {
            let is_arg = |local: Local, body: &Body<'tcx>| -> bool {
                body.args_iter().any(|arg| arg == local)
            };
            let mut arg_places1 = pts1.iter().filter_map(|node| match node {
                ConstraintNode::Alloc(place) if is_arg(place.local, body1) => Some(place),
                _ => None,
            });
            let mut arg_places2 = pts2.iter().filter_map(|node| match node {
                ConstraintNode::Alloc(place) if is_arg(place.local, body2) => Some(place),
                _ => None,
            });
            if arg_places1.any(|place1| {
                arg_places2.any(|place2| {
                    body1.local_decls[place1.local].ty == body2.local_decls[place2.local].ty
                        && place1.projection == place2.projection
                })
            }) {
                return ApproximateAliasKind::Possibly;
            }
        }
        ApproximateAliasKind::Unlikely
    }
}
