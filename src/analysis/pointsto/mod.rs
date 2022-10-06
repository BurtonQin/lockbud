//! Points-to analysis.
//! It checks if two pointers may point to the same memory& cell.
//! It depends on `CallGraph` and provides support for detectors.
//! Currently I have implemented an Andersen-style pointer analysis.
//! It is basically a field-sensitive intra-procedural pointer analysis
//! with limited support for inter-procedural analysis
//! of methods and closures.
//! See `Andersen` for more details.
extern crate rustc_hash;
extern crate rustc_hir;
extern crate rustc_index;

use std::cmp::{Ordering, PartialOrd};
use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};
use rustc_hir::def_id::DefId;
use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{
    Body, Constant, ConstantKind, Local, Location, Operand, Place, PlaceElem, PlaceRef,
    ProjectionElem, Rvalue, Statement, StatementKind, Terminator, TerminatorKind,
};
use rustc_middle::ty::{Instance, TyCtxt, TyKind};

use petgraph::dot::{Config, Dot};
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use petgraph::{Directed, Direction, Graph};

use crate::analysis::callgraph::{CallGraph, CallGraphNode, CallSiteLocation, InstanceId};
use crate::interest::concurrency::lock::LockGuardId;
use crate::interest::memory::ownership;

/// Field-sensitive intra-procedural Andersen pointer analysis.
/// <https://helloworld.pub/program-analysis-andersen-pointer-analysis-algorithm-based-on-svf.html>
/// 1. collect constraints from MIR to build a `ConstraintGraph`
/// 2. adopt a fixed-point algorithm to update `ConstraintGraph` and points-to info
///
/// There are several changes:
/// 1. Use a place to represent a memroy cell.
/// 2. Create an Alloc node for each place and let the place points to it.
/// 3. Distinguish local places with global ones (denoted as Constant).
/// 4. Treat special functions by names or signatures (e.g., Arc::clone).
/// 5. Interproc methods: Use parameters' type info to guide the analysis heuristically (simple but powerful).
/// 6. Interproc closures: Track the upvars of closures in the functions defining the closures (restricted).
pub struct Andersen<'a, 'tcx> {
    body: &'a Body<'tcx>,
    tcx: TyCtxt<'tcx>,
    pts: PointsToMap<'tcx>,
}

pub type PointsToMap<'tcx> = FxHashMap<ConstraintNode<'tcx>, FxHashSet<ConstraintNode<'tcx>>>;

impl<'a, 'tcx> Andersen<'a, 'tcx> {
    pub fn new(body: &'a Body<'tcx>, tcx: TyCtxt<'tcx>) -> Self {
        Self {
            body,
            tcx,
            pts: Default::default(),
        }
    }

    pub fn analyze(&mut self) {
        let mut collector = ConstraintGraphCollector::new(self.body, self.tcx);
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
            // alias_copy: target = &X; X = ptr::read(node)
            for target in graph.alias_copy_targets(&node) {
                if graph.insert_edge(node, target, ConstraintEdge::Copy) {
                    worklist.push_back(node);
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
/// namely, forall Place(p), Alloc(p)--|address|-->Place(p).
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
/// This is the case when b is an arg. I just treat (*b).0
/// as the mem cell and do not further dereference it.
/// I also introduce `AliasCopy` edge to represent x->y
/// for y=Arc::clone(x) and y=ptr::read(x),
/// where x--|copy|-->pointers of y
/// and x--|load|-->y (y=*x)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConstraintEdge {
    Address,
    Copy,
    Load,
    Store,
    AliasCopy, // Special: y=Arc::clone(x) or y=ptr::read(x)
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

    fn add_alias_copy(&mut self, lhs: PlaceRef<'tcx>, rhs: PlaceRef<'tcx>) {
        let lhs = ConstraintNode::Place(lhs);
        let rhs = ConstraintNode::Place(rhs);
        let lhs = self.get_or_insert_node(lhs);
        let rhs = self.get_or_insert_node(rhs);
        self.graph.add_edge(rhs, lhs, ConstraintEdge::AliasCopy);
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

    /// X = Arc::clone(rhs) or X = ptr::read(rhs)
    /// ? = &X
    ///
    /// rhs--|alias_copy|-->X,
    /// X--|address|-->?
    fn alias_copy_targets(&self, rhs: &ConstraintNode<'tcx>) -> Vec<ConstraintNode<'tcx>> {
        let rhs = self.get_node(rhs).unwrap();
        self.graph
            .edges_directed(rhs, Direction::Outgoing)
            .filter_map(|edge| {
                if *edge.weight() == ConstraintEdge::AliasCopy {
                    Some(edge.target())
                } else {
                    None
                }
            })
            .fold(Vec::new(), |mut acc, copy_alias_target| {
                let address_targets = self
                    .graph
                    .edges_directed(copy_alias_target, Direction::Outgoing)
                    .filter_map(|edge| {
                        if *edge.weight() == ConstraintEdge::Address {
                            Some(self.graph.node_weight(edge.target()).copied().unwrap())
                        } else {
                            None
                        }
                    });
                acc.extend(address_targets);
                acc
            })
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
struct ConstraintGraphCollector<'a, 'tcx> {
    body: &'a Body<'tcx>,
    tcx: TyCtxt<'tcx>,
    graph: ConstraintGraph<'tcx>,
}

impl<'a, 'tcx> ConstraintGraphCollector<'a, 'tcx> {
    fn new(body: &'a Body<'tcx>, tcx: TyCtxt<'tcx>) -> Self {
        Self {
            body,
            tcx,
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

    /// dest: *const T = Vec::as_ptr(arg: &Vec<T>) =>
    /// arg--|copy|-->dest
    fn process_call_arg_dest(&mut self, arg: PlaceRef<'tcx>, dest: PlaceRef<'tcx>) {
        self.graph.add_copy(dest, arg);
    }

    /// dest: Arc<T> = Arc::clone(arg: &Arc<T>) or dest: T = ptr::read(arg: *const T) =>
    /// arg--|load|-->dest and
    /// arg--|alias_copy|-->dest
    fn process_alias_copy(&mut self, arg: PlaceRef<'tcx>, dest: PlaceRef<'tcx>) {
        self.graph.add_load(dest, arg);
        self.graph.add_alias_copy(dest, arg);
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
                        } else if &p2.projection[..p1.projection.len()] == p1.projection {
                            self.graph.add_copy(*p1, *p2);
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

impl<'a, 'tcx> Visitor<'tcx> for ConstraintGraphCollector<'a, 'tcx> {
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
            | StatementKind::Intrinsic(_) => {}
        }
    }

    /// For destination = Arc::clone(move arg0) and destination = ptr::read(move arg0),
    /// destination = alias copy args0
    /// For other callsites like `destination = call fn(move args0)`,
    /// heuristically assumes that
    /// destination = copy args0
    fn visit_terminator(&mut self, terminator: &Terminator<'tcx>, _location: Location) {
        if let TerminatorKind::Call {
            func,
            args,
            destination,
            ..
        } = &terminator.kind
        {
            if let (&[Operand::Move(arg)], dest) = (args.as_slice(), destination) {
                let func_ty = func.ty(self.body, self.tcx);
                match func_ty.kind() {
                    TyKind::FnDef(def_id, substs)
                        if ownership::is_arc_or_rc_clone(*def_id, substs, self.tcx)
                            || ownership::is_ptr_read(*def_id, self.tcx) =>
                    {
                        return self.process_alias_copy(arg.as_ref(), dest.as_ref());
                    }
                    _ => {}
                }
                self.process_call_arg_dest(arg.as_ref(), dest.as_ref());
            };
        }
    }
}

/// We do not use Must/May/Not since the pointer analysis implementation is overapproximate.
/// Instead, we use probably, possibly, unlikely as alias kinds.
/// We will report bugs of probaly and possibly kinds.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ApproximateAliasKind {
    Probably,
    Possibly,
    Unlikely,
    Unknown,
}

/// Probably > Possibly > Unlikey > Unknown
impl PartialOrd for ApproximateAliasKind {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        use ApproximateAliasKind::*;
        match (*self, *other) {
            (Probably, Probably)
            | (Possibly, Possibly)
            | (Unlikely, Unlikely)
            | (Unknown, Unknown) => Some(Ordering::Equal),
            (Probably, _) | (Possibly, Unlikely) | (Possibly, Unknown) | (Unlikely, Unknown) => {
                Some(Ordering::Greater)
            }
            (_, Probably) | (Unlikely, Possibly) | (Unknown, Possibly) | (Unknown, Unlikely) => {
                Some(Ordering::Less)
            }
        }
    }
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
    /// If they are from the same func, then perform intraproc alias analysis;
    /// otherwise, perform interproc alias analysis.
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
                let node1 = ConstraintNode::Place(Place::from(local1).as_ref());
                let node2 = ConstraintNode::Place(Place::from(local2).as_ref());
                if instance1.def_id() == instance2.def_id() {
                    self.intraproc_alias(instance1, &node1, &node2)
                        .unwrap_or(ApproximateAliasKind::Unknown)
                } else {
                    self.interproc_alias(instance1, &node1, instance2, &node2)
                        .unwrap_or(ApproximateAliasKind::Unknown)
                }
            }
            _ => ApproximateAliasKind::Unknown,
        }
    }

    /// Check if `pointer` points to `pointee`.
    /// First get the pts(`pointer`),
    /// then check alias between each node in pts(`pointer`) and `pointee`.
    /// Choose the highest alias kind.
    pub fn points_to(&mut self, pointer: AliasId, pointee: AliasId) -> ApproximateAliasKind {
        let AliasId {
            instance_id: id1,
            local: local1,
        } = pointer;
        let AliasId {
            instance_id: id2,
            local: local2,
        } = pointee;

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
                let node1 = ConstraintNode::Place(Place::from(local1).as_ref());
                let node2 = ConstraintNode::Place(Place::from(local2).as_ref());
                let body1 = self.tcx.instance_mir(instance1.def);
                let points_to_map = self.get_or_insert_pts(instance1.def_id(), body1).clone();
                if instance1.def_id() == instance2.def_id() {
                    let mut final_alias_kind = ApproximateAliasKind::Unknown;
                    for local_pointee in &points_to_map[&node1] {
                        let alias_kind = self
                            .intraproc_alias(instance1, local_pointee, &node2)
                            .unwrap_or(ApproximateAliasKind::Unknown);
                        if alias_kind > final_alias_kind {
                            final_alias_kind = alias_kind;
                        }
                    }
                    final_alias_kind
                } else {
                    let mut final_alias_kind = ApproximateAliasKind::Unknown;
                    for local_pointee in &points_to_map[&node1] {
                        let alias_kind = self
                            .interproc_alias(instance1, local_pointee, instance2, &node2)
                            .unwrap_or(ApproximateAliasKind::Unknown);
                        if alias_kind > final_alias_kind {
                            final_alias_kind = alias_kind;
                        }
                    }
                    final_alias_kind
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
            let mut pointer_analysis = Andersen::new(body, self.tcx);
            pointer_analysis.analyze();
            let pts = pointer_analysis.finish();
            self.pts.entry(def_id).or_insert(pts)
        }
    }

    /// Check alias of p1 and p2 if they are from the same fn.
    /// if pts(p1) intersect pts(p2) != empty then they probably alias else unlikely
    fn intraproc_alias(
        &mut self,
        instance: &Instance<'tcx>,
        node1: &ConstraintNode<'tcx>,
        node2: &ConstraintNode<'tcx>,
    ) -> Option<ApproximateAliasKind> {
        let body = self.tcx.instance_mir(instance.def);
        let points_to_map = self.get_or_insert_pts(instance.def_id(), body);
        if points_to_map
            .get(node1)?
            .intersection(points_to_map.get(node2)?)
            .next()
            .is_some()
        {
            Some(ApproximateAliasKind::Probably)
        } else {
            Some(ApproximateAliasKind::Unlikely)
        }
    }

    /// Check if `pointer` points-to `pointee` in the same function.
    fn intraproc_points_to(
        &mut self,
        instance: &Instance<'tcx>,
        pointer: ConstraintNode<'tcx>,
        pointee: ConstraintNode<'tcx>,
    ) -> Option<ApproximateAliasKind> {
        let body = self.tcx.instance_mir(instance.def);
        let points_to_map = self.get_or_insert_pts(instance.def_id(), body);
        let pointer_pts = points_to_map.get(&pointer)?;
        // 1. if pts(pointer) contains pointee then probably alias
        if pointer_pts.contains(&pointee) {
            return Some(ApproximateAliasKind::Probably);
        }
        let pointee_pts = points_to_map.get(&pointee)?;
        // 2. if exists p: pts(pointer) contains Place(p) and pts(pointee) contains Alloc(p) then possibly alias
        if pointer_pts.iter().any(|n1| {
            let p = match n1 {
                ConstraintNode::Place(p) => *p,
                _ => return false,
            };
            pointee_pts
                .iter()
                .any(|n2| matches!(n2, ConstraintNode::Alloc(p2) if *p2 == p))
        }) {
            Some(ApproximateAliasKind::Possibly)
        } else {
            Some(ApproximateAliasKind::Unlikely)
        }
    }

    /// Check alias of p1 and p2 if they are from different fn.
    /// To avoid interproc analysis, we use heuristic assumption:
    /// p1 and p2 alias if:
    /// 1. they point to the same Constant or
    /// 2. they point to function parameters with the same type and field or
    /// 3. they point to upvars of closures and the upvars alias in the def func.
    /// Formally,
    /// if exists a1 in pts(p1) and a1 is Constant(c1) and
    ///    exists a2 in pts(p2) and a2 is Constant(c2) and
    ///    c1 = c2
    /// then return probably alias
    /// else if exists a1 in pts(p1) and a1.local in Args(fn1) and
    ///    exists a2 in pts(pt2) and a2.local in Args(fn2) and
    ///    (a1.local.ty = a2.local.ty and
    ///    a1.projection = a2.projection)
    /// then return possible alias
    /// else if p1 or p2 are in closures then
    ///    if p2 points to defsite_upvar(p1) or
    ///       p1 points to defsite_upvar(p2) or
    ///       upvar(p1) alias with upvar(p2)
    ///    then return possible alias
    /// return unlikely
    fn interproc_alias(
        &mut self,
        instance1: &Instance<'tcx>,
        node1: &ConstraintNode<'tcx>,
        instance2: &Instance<'tcx>,
        node2: &ConstraintNode<'tcx>,
    ) -> Option<ApproximateAliasKind> {
        let body1 = self.tcx.instance_mir(instance1.def);
        let body2 = self.tcx.instance_mir(instance2.def);
        let points_to_map1 = self.get_or_insert_pts(instance1.def_id(), body1).clone();
        let points_to_map2 = self.get_or_insert_pts(instance2.def_id(), body2).clone();
        let pts1 = points_to_map1.get(node1)?;
        let pts2 = points_to_map2.get(node2)?;
        // 1. Check if `node1` and `node2` points to the same Constant.
        if point_to_same_constant(pts1, pts2) {
            return Some(ApproximateAliasKind::Probably);
        }
        // 2. Check if `node1` and `node2` points to func parameters with the same local's type and projection.
        if point_to_same_type_param(pts1, pts2, body1, body2) {
            return Some(ApproximateAliasKind::Possibly);
        }
        // 3. Check if `node1` and `node2` point to upvars of closures and the upvars alias in the def func.
        // 3.1 Get defsite upvars of `node1` then check if `node2` points to the upvar.
        let mut defsite_upvars1 = None;
        if self.tcx.is_closure(instance1.def_id()) {
            let pts_paths = points_to_paths_to_param(*node1, body1, &points_to_map1);
            for pts_path in pts_paths {
                let defsite_upvars = match self.closure_defsite_upvars(instance1, pts_path) {
                    Some(defsite_upvars) => defsite_upvars,
                    None => continue,
                };
                for (def_inst, upvar) in defsite_upvars.iter() {
                    if def_inst.def_id() == instance2.def_id() {
                        let alias_kind = self
                            .intraproc_points_to(def_inst, *node2, *upvar)
                            .unwrap_or(ApproximateAliasKind::Unknown);
                        if alias_kind > ApproximateAliasKind::Unlikely {
                            return Some(alias_kind);
                        }
                    }
                }
                // Record defsite_upvars.
                defsite_upvars1 = Some(defsite_upvars);
                // The last fields of the paths are usually the same, thus only iterate once.
                break;
            }
        }
        // 3.2 Get defsite upvars of `node2` then check if `node1` points to the upvar.
        let mut defsite_upvars2 = None;
        if self.tcx.is_closure(instance2.def_id()) {
            let pts_paths = points_to_paths_to_param(*node2, body2, &points_to_map2);
            for pts_path in pts_paths {
                let defsite_upvars = match self.closure_defsite_upvars(instance2, pts_path) {
                    Some(defsite_upvars) => defsite_upvars,
                    None => continue,
                };
                for (def_inst, upvar) in defsite_upvars.iter() {
                    if def_inst.def_id() == instance1.def_id() {
                        let alias_kind = self
                            .intraproc_points_to(def_inst, *node1, *upvar)
                            .unwrap_or(ApproximateAliasKind::Unknown);
                        if alias_kind > ApproximateAliasKind::Unlikely {
                            return Some(alias_kind);
                        }
                    }
                }
                // Record defsite_upvars.
                defsite_upvars2 = Some(defsite_upvars);
                // The last fields of the paths are usually the same, thus only iterate once.
                break;
            }
        }
        // 3.3 Check if upvars of `node1` and `node2` alias with each other.
        if let (Some(defsite_upvars1), Some(defsite_upvars2)) = (defsite_upvars1, defsite_upvars2) {
            for (instance1, node1) in defsite_upvars1 {
                for (instance2, node2) in &defsite_upvars2 {
                    if instance1.def_id() == instance2.def_id() {
                        let alias_kind = self
                            .intraproc_alias(instance1, &node1, node2)
                            .unwrap_or(ApproximateAliasKind::Unknown);
                        if alias_kind > ApproximateAliasKind::Unlikely {
                            return Some(alias_kind);
                        }
                    }
                }
            }
        }
        Some(ApproximateAliasKind::Unlikely)
    }

    /// Suppose _1 is the closure parameter and _9 is the arg in the def fn.
    /// For upvar _1.0 in the closure, we get _9.0 in the def fn.
    /// Though PointsToPath enables tracking more fields
    /// like _1.0.0 -> _9.0.0,
    /// I find one field is enough for most cases.
    fn closure_defsite_upvars(
        &self,
        closure: &'a Instance<'tcx>,
        path: PointsToPath<'tcx>,
    ) -> Option<Vec<(&'a Instance<'tcx>, ConstraintNode<'tcx>)>> {
        let projection = path.last()?.0;
        let def_inst_args = closure_defsite_args(closure, self.callgraph);
        let def_inst_upvars = def_inst_args
            .into_iter()
            .map(|(def_inst, arg)| {
                (
                    def_inst,
                    ConstraintNode::Place(PlaceRef {
                        local: arg,
                        projection,
                    }),
                )
            })
            .collect::<Vec<_>>();
        if def_inst_upvars.is_empty() {
            None
        } else {
            Some(def_inst_upvars)
        }
    }
}

/// Check if p1 and p2 point to the same Constant.
/// Return true
/// if exists a1 in pts(p1) and a1 is Constant(c1) and
///    exists a2 in pts(p2) and a2 is Constant(c2) and
///    c1 = c2
fn point_to_same_constant<'tcx>(
    pts1: &FxHashSet<ConstraintNode<'tcx>>,
    pts2: &FxHashSet<ConstraintNode<'tcx>>,
) -> bool {
    let mut constants1 = pts1
        .iter()
        .filter(|node| matches!(node, &ConstraintNode::ConstantDeref(_)));
    let mut constants2 = pts2
        .iter()
        .filter(|node| matches!(node, &ConstraintNode::ConstantDeref(_)));
    constants1.any(|c1| constants2.any(|c2| c2 == c1))
}

/// Check if `local` is a parameter
#[inline]
fn is_parameter(local: Local, body: &Body<'_>) -> bool {
    body.args_iter().any(|arg| arg == local)
}

/// Check if p1 and p2 point to func parameters with the same local's type and projection.
/// Return true
/// if exists a1 in pts(p1) and a1.local is param and
///    exists a2 in pts(p2) and a2.local is param and
///    a1.local.ty = a2.local.ty and
///    a1.projection = a2.projection
fn point_to_same_type_param<'tcx>(
    pts1: &FxHashSet<ConstraintNode<'tcx>>,
    pts2: &FxHashSet<ConstraintNode<'tcx>>,
    body1: &Body<'tcx>,
    body2: &Body<'tcx>,
) -> bool {
    let mut parameter_places1 = pts1.iter().filter_map(|node| match node {
        ConstraintNode::Alloc(place) if is_parameter(place.local, body1) => Some(*place),
        _ => None,
    });
    let mut parameter_places2 = pts2.iter().filter_map(|node| match node {
        ConstraintNode::Alloc(place) if is_parameter(place.local, body2) => Some(*place),
        _ => None,
    });
    parameter_places1.any(|place1| {
        parameter_places2.any(|place2| {
            body1.local_decls[place1.local].ty == body2.local_decls[place2.local].ty
                && place1.projection == place2.projection
        })
    })
}

/// Closure's defsites and the corresponding args
fn closure_defsite_args<'a, 'b: 'a, 'tcx>(
    closure_inst: &'b Instance<'tcx>,
    callgraph: &'a CallGraph<'tcx>,
) -> Vec<(&'a Instance<'tcx>, Local)> {
    let callee_id = callgraph.instance_to_index(closure_inst).unwrap();
    let callers = callgraph.callers(callee_id);
    callers.into_iter().fold(Vec::new(), |mut acc, caller_id| {
        let caller_inst = callgraph.index_to_instance(caller_id).unwrap().instance();
        acc.extend(
            callgraph
                .callsites(caller_id, callee_id)
                .unwrap_or_default()
                .iter()
                .filter_map(|cs_loc| {
                    if let CallSiteLocation::ClosureDef(local) = cs_loc {
                        Some((caller_inst, *local))
                    } else {
                        None
                    }
                }),
        );
        acc
    })
}

type PointsToPath<'tcx> = Vec<(&'tcx [PlaceElem<'tcx>], ConstraintNode<'tcx>)>;

/// Find the points-to paths from the given node to the closure parameters (upvar).
/// A points-to path is like [([], node), ([Field(0)], node1), ([Filed(1)], node2), ..., ([Field(n)], parameter)]
///
/// Current points-to analysis does not support relations like `from _9.1 to _9`,
/// which means pts(_9)⊇pts(_9.1) but not vice versa.
/// The upvars in closures usually share the following pattern:
/// _9 = &_1.1; // pts(_9)∋_1.1
/// _8 = _9.1;  // pts(_8)⊇pts(_9.1)
/// where _1 is the closure parameter.
/// Due to the above reason, pts(_8) does not contain _1.1 and fails to be identified as an upvar.
/// Thus we need to track the pts-to paths from the given node to the parameter.
/// If there exists such a path, then the node is an upvar.
fn points_to_paths_to_param<'a, 'tcx>(
    node: ConstraintNode<'tcx>,
    body: &'tcx Body<'tcx>,
    points_to_map: &'a PointsToMap<'tcx>,
) -> Vec<PointsToPath<'tcx>> {
    let mut result = Vec::new();
    let mut path = Vec::new();
    let mut visited = FxHashSet::default();
    dfs_paths_recur(
        &[],
        node,
        body,
        points_to_map,
        &mut visited,
        &mut path,
        &mut result,
    );
    result
}

/// DFS search for points-to paths from `node` to the parameter.
fn dfs_paths_recur<'a, 'tcx>(
    prev_proj: &'tcx [PlaceElem<'tcx>],
    node: ConstraintNode<'tcx>,
    body: &'tcx Body<'tcx>,
    points_to_map: &'a PointsToMap<'tcx>,
    visited: &mut FxHashSet<ConstraintNode<'tcx>>,
    path: &mut PointsToPath<'tcx>,
    result: &mut Vec<PointsToPath<'tcx>>,
) {
    // Exit if the node has been visited or is not Alloc or Place.
    if !visited.insert(node) {
        return;
    }
    let place = match node {
        ConstraintNode::Alloc(place) | ConstraintNode::Place(place) => place,
        _ => return,
    };
    path.push((prev_proj, node));
    // If found a path to the parameter, then output it to result.
    if is_parameter(place.local, body) {
        result.push(path.clone());
        path.pop();
        return;
    }
    let pts = match points_to_map.get(&node) {
        Some(pts) => pts,
        None => {
            path.pop();
            return;
        }
    };
    for pointee in pts {
        match pointee {
            ConstraintNode::Alloc(place1) | ConstraintNode::Place(place1)
                if !place1.projection.is_empty() =>
            {
                let node1 = ConstraintNode::Place(Place::from(place1.local).as_ref());
                dfs_paths_recur(
                    place1.projection,
                    node1,
                    body,
                    points_to_map,
                    visited,
                    path,
                    result,
                );
            }
            _ => {}
        }
    }
    path.pop();
}
