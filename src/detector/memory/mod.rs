extern crate rustc_data_structures;
extern crate rustc_middle;

use rustc_data_structures::fx::FxHashMap;
use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{Body, Location, Place, Terminator, TerminatorKind};
use rustc_middle::ty::TyCtxt;

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
        let args0 = args.get(0).and_then(|op| op.place());
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
        let path = tcx.def_path_str_with_substs(instance.def_id(), instance.substs);
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
    drop_locations: Vec<(Location, Place<'tcx>)>,
}

impl<'tcx> AutoDropCollector<'tcx> {
    fn new() -> Self {
        Self {
            drop_locations: Vec::new(),
        }
    }

    fn finish(self) -> Vec<(Location, Place<'tcx>)> {
        self.drop_locations
    }
}

impl<'tcx> Visitor<'tcx> for AutoDropCollector<'tcx> {
    fn visit_terminator(&mut self, terminator: &Terminator<'tcx>, location: Location) {
        if let TerminatorKind::Drop { place, .. } = &terminator.kind {
            self.drop_locations.push((location, *place));
        }
    }
}

fn is_reachable(from: Location, to: Location, body: &Body<'_>) -> bool {
    let mut worklist = Vec::new();
    worklist.push(from);
    while let Some(curr) = worklist.pop() {
        if curr == to {
            return true;
        }
        if body.terminator_loc(curr.block) == curr {
            for succ in body.basic_blocks()[curr.block].terminator().successors() {
                worklist.push(Location {
                    block: succ,
                    statement_index: 0,
                });
            }
        } else {
            worklist.push(curr.successor_within_block());
        }
    }
    false
}
