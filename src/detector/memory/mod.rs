extern crate rustc_data_structures;
extern crate rustc_middle;

use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{
    BasicBlock, Body, Local, Location, Operand, Place, Rvalue, StatementKind, Terminator,
    TerminatorKind, START_BLOCK,
};
use rustc_middle::ty::{self, TyCtxt};

use petgraph::visit::IntoNodeReferences;

use crate::analysis::callgraph::{CallGraph, InstanceId};

mod invalid_free;
mod use_after_free;

pub use invalid_free::InvalidFreeDetector;
pub use use_after_free::UseAfterFreeDetector;

/// Find dest and the first arg of a Call
fn dest_args0<'tcx>(
    body: &Body<'tcx>,
    loc: Location,
) -> Option<(Place<'tcx>, Option<Place<'tcx>>)> {
    if let TerminatorKind::Call {
        func: _func,
        args,
        destination,
        ..
    } = &body[loc.block].terminator().kind
    {
        let args0 = args.first().and_then(|op| op.node.place());
        return Some((*destination, args0));
    }
    None
}
/// std::mem::drop(place);
fn collect_manual_drop<'tcx>(
    callgraph: &CallGraph<'tcx>,
    tcx: TyCtxt<'tcx>,
) -> FxHashMap<InstanceId, Vec<(Location, Place<'tcx>)>> {
    let mut manual_drops: FxHashMap<InstanceId, Vec<_>> = FxHashMap::default();
    for (callee_id, node) in callgraph.graph.node_references() {
        let instance = node.instance();
        let path = tcx.def_path_str_with_args(instance.def_id(), instance.args);
        if !path.starts_with("std::mem::drop") && !path.starts_with("core::mem::drop") {
            continue;
        }
        let caller_ids = callgraph.callers(callee_id);
        for caller_id in caller_ids {
            let callsites = match callgraph.callsites(caller_id, callee_id) {
                Some(callsites) => callsites,
                None => continue,
            };
            let caller_node = match callgraph.index_to_instance(caller_id) {
                Some(caller_node) => caller_node,
                None => continue,
            };
            let caller = caller_node.instance();
            let body = tcx.instance_mir(caller.def);
            for loc in callsites {
                let loc = match loc.location() {
                    Some(loc) => loc,
                    None => continue,
                };
                let places0 = match dest_args0(body, loc) {
                    Some((_, Some(places0))) => places0,
                    _ => continue,
                };
                manual_drops
                    .entry(caller_id)
                    .or_default()
                    .push((loc, places0));
            }
        }
    }
    manual_drops
}

/// Collect TerminatorKind::Drop
struct AutoDropCollector<'tcx> {
    tcx: TyCtxt<'tcx>,
    body: &'tcx Body<'tcx>,
    drop_locations: Vec<(Location, Place<'tcx>)>,
}

impl<'tcx> AutoDropCollector<'tcx> {
    fn new(tcx: TyCtxt<'tcx>, body: &'tcx Body<'tcx>) -> Self {
        Self {
            tcx,
            body,
            drop_locations: Vec::new(),
        }
    }

    fn finish(self) -> Vec<(Location, Place<'tcx>)> {
        self.drop_locations
    }

    /// Dropping a borrow guard releases the borrow but never frees the
    /// borrowed value, so it cannot be the free that a use-after-free is
    /// measured against: e.g. `let p = data.borrow().as_ptr()` keeps `data`
    /// alive after the temporary `Ref` dies (#110).
    fn is_borrow_guard(&self, place: &Place<'tcx>) -> bool {
        let ty = place.ty(&self.body.local_decls, self.tcx).ty;
        match ty.kind() {
            ty::TyKind::Adt(adt_def, _) => {
                // def_path_str prints the std facade path (std::cell::Ref)
                // for these core types, so match on the tail.
                let path = self.tcx.def_path_str(adt_def.did());
                path.ends_with("cell::Ref") || path.ends_with("cell::RefMut")
            }
            _ => false,
        }
    }
}

impl<'tcx> Visitor<'tcx> for AutoDropCollector<'tcx> {
    fn visit_terminator(&mut self, terminator: &Terminator<'tcx>, location: Location) {
        if let TerminatorKind::Drop { place, .. } = &terminator.kind {
            if !self.is_borrow_guard(place) {
                self.drop_locations.push((location, *place));
            }
        }
    }
}

/// Whether dropping `place` is not a definite free of its pointee because
/// the type contains a reference-counted handle (`Arc`/`Rc`): the pointed-to
/// allocation is only freed when the last handle is dropped, which cannot be
/// decided intraprocedurally. Escaping a raw pointer and then dropping such a
/// handle is therefore not reported as a definite use-after-free escape
/// (#107). Direct use-after-drop within the same function is still reported.
pub(super) fn is_refcounted_handle_drop<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &Body<'tcx>,
    place: &Place<'tcx>,
) -> bool {
    let ty = place.ty(&body.local_decls, tcx).ty;
    contains_refcounted_handle(tcx, ty)
}

fn contains_refcounted_handle(tcx: TyCtxt<'_>, ty: ty::Ty<'_>) -> bool {
    match ty.kind() {
        ty::TyKind::Adt(adt_def, substs) => {
            let path = tcx.def_path_str(adt_def.did());
            path.ends_with("sync::Arc")
                || path.ends_with("rc::Rc")
                || substs.types().any(|t| contains_refcounted_handle(tcx, t))
        }
        ty::TyKind::Array(elem, _) | ty::TyKind::Slice(elem) => {
            contains_refcounted_handle(tcx, *elem)
        }
        ty::TyKind::Tuple(fields) => fields.iter().any(|t| contains_refcounted_handle(tcx, t)),
        _ => false,
    }
}

fn is_reachable(from: Location, to: Location, body: &Body<'_>) -> bool {
    if from.block == to.block {
        return from.statement_index <= to.statement_index;
    }
    let from_block = from.block;
    let to_block = to.block;
    let mut worklist = Vec::new();
    let mut visited = FxHashSet::default();
    worklist.push(from_block);
    visited.insert(from_block);
    while let Some(curr) = worklist.pop() {
        if curr == to_block {
            return true;
        }
        for succ in body.basic_blocks[curr].terminator().successors() {
            if !visited.insert(succ) {
                continue;
            }
            worklist.push(succ);
        }
    }
    false
}

/// Whether every path from `start` to `target` goes through `skipped`.
fn is_only_reachable_via(
    start: BasicBlock,
    target: BasicBlock,
    skipped: BasicBlock,
    body: &Body<'_>,
) -> bool {
    if start == target {
        return false;
    }
    let mut worklist = Vec::new();
    let mut visited = FxHashSet::default();
    worklist.push(start);
    visited.insert(start);
    while let Some(curr) = worklist.pop() {
        if curr == target {
            return true;
        }
        for succ in body.basic_blocks[curr].terminator().successors() {
            if succ != skipped && visited.insert(succ) {
                worklist.push(succ);
            }
        }
    }
    false
}

fn collect_moves_in_rvalue<'tcx>(
    rvalue: &Rvalue<'tcx>,
    loc: Location,
    moves: &mut FxHashMap<Local, Vec<Location>>,
) {
    let operands: Vec<&Operand<'tcx>> = match rvalue {
        Rvalue::Use(op)
        | Rvalue::Repeat(op, _)
        | Rvalue::UnaryOp(_, op)
        | Rvalue::Cast(_, op, _)
        | Rvalue::ShallowInitBox(op, _) => vec![op],
        Rvalue::BinaryOp(_, pair) => vec![&pair.0, &pair.1],
        Rvalue::Aggregate(_, ops) => ops.iter().collect(),
        _ => vec![],
    };
    for op in operands {
        if let Operand::Move(place) = op {
            if place.projection.is_empty() {
                moves.entry(place.local).or_default().push(loc);
            }
        }
    }
}

/// Remove auto-drops that can never execute. MIR's drop elaboration leaves a
/// flag-guarded `Drop` behind for a local that was moved out, and that dead
/// drop must not be treated as a real free of the pointee: e.g.
/// `_rc = Rc::new(move _value)` keeps the pointee alive on the heap while a
/// dead `Drop(_value)` remains in the MIR (#110, #107).
///
/// A drop of a bare local is considered dead when some move-out of that local
/// (a) is ordered before the drop, (b) every path to the drop passes it, and
/// (c) no path from it reaches another write of the local, i.e. the local is
/// definitely moved out at the drop. Manual `mem::drop` callsites are real
/// frees and are never filtered.
pub(super) fn filter_moved_out_drops<'tcx>(
    drops: Vec<(Location, Place<'tcx>)>,
    body: &'tcx Body<'tcx>,
) -> Vec<(Location, Place<'tcx>)> {
    let mut writes: FxHashMap<Local, Vec<Location>> = FxHashMap::default();
    let mut moves: FxHashMap<Local, Vec<Location>> = FxHashMap::default();
    for (block, block_data) in body.basic_blocks.iter_enumerated() {
        for (idx, statement) in block_data.statements.iter().enumerate() {
            let loc = Location {
                block,
                statement_index: idx,
            };
            if let StatementKind::Assign(boxed) = &statement.kind {
                let (dest, rvalue) = &**boxed;
                writes.entry(dest.local).or_default().push(loc);
                collect_moves_in_rvalue(rvalue, loc, &mut moves);
            }
        }
        let term_loc = Location {
            block,
            statement_index: block_data.statements.len(),
        };
        if let TerminatorKind::Call {
            args, destination, ..
        } = &block_data.terminator().kind
        {
            writes.entry(destination.local).or_default().push(term_loc);
            for arg in args {
                if let Operand::Move(place) = &arg.node {
                    if place.projection.is_empty() {
                        moves.entry(place.local).or_default().push(term_loc);
                    }
                }
            }
        }
    }
    drops
        .into_iter()
        .filter(|(drop_loc, drop_place)| {
            if !drop_place.projection.is_empty() {
                return true;
            }
            let Some(move_locs) = moves.get(&drop_place.local) else {
                return true;
            };
            let no_write_after = |move_loc: &Location| {
                writes.get(&drop_place.local).is_none_or(|write_locs| {
                    write_locs
                        .iter()
                        .all(|w| !is_reachable(*move_loc, *w, body))
                })
            };
            !move_locs.iter().any(|move_loc| {
                let cut = if move_loc.block == drop_loc.block {
                    move_loc.statement_index < drop_loc.statement_index
                } else {
                    is_only_reachable_via(START_BLOCK, drop_loc.block, move_loc.block, body)
                };
                cut && no_write_after(move_loc)
            })
        })
        .collect()
}
