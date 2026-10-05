/// find raw ptrs and drop(place)
/// raw ptr's provenance place
/// raw ptr assigned to other place
/// drop(place)
/// after drop, raw ptr or its assignee is used
extern crate rustc_data_structures;
extern crate rustc_index;
extern crate rustc_middle;

use rustc_data_structures::fx::FxHashSet;
use rustc_index::Idx;
use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{
    AggregateKind, Body, HasLocalDecls, Local, Location, Operand, Place, Rvalue, StatementKind,
};
use rustc_middle::ty::{self, ExistentialPredicate, Instance, TyCtxt, TyKind, TypingEnv};

use petgraph::visit::IntoNodeReferences;

use super::{collect_manual_drop, is_reachable, AutoDropCollector};
use crate::analysis::callgraph::CallGraphNode;
use crate::analysis::defuse::find_uses;
use crate::analysis::pointsto::{ConstraintNode, PointsToMap};
use crate::analysis::{callgraph::CallGraph, pointsto::AliasAnalysis};
use crate::detector::report::{Report, ReportContent};

pub struct UseAfterFreeDetector<'tcx> {
    tcx: TyCtxt<'tcx>,
}

impl<'tcx> UseAfterFreeDetector<'tcx> {
    pub fn new(tcx: TyCtxt<'tcx>) -> Self {
        Self { tcx }
    }

    pub fn detect(
        &self,
        callgraph: &CallGraph<'tcx>,
        alias_analysis: &mut AliasAnalysis<'_, 'tcx>,
    ) -> Vec<Report> {
        let mut reports = Vec::new();
        let manual_drops = collect_manual_drop(callgraph, self.tcx);
        for (instance_id, node) in callgraph.graph.node_references() {
            let instance = match node {
                CallGraphNode::WithBody(instance) => instance,
                CallGraphNode::WithoutBody(_) => continue,
            };
            let local_manual_drops = manual_drops
                .get(&instance_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            reports.extend(self.detect_instance(instance, alias_analysis, local_manual_drops));
        }
        reports
    }

    fn detect_instance(
        &self,
        instance: &Instance<'tcx>,
        alias_analysis: &mut AliasAnalysis<'_, 'tcx>,
        manual_drops: &[(Location, Place<'tcx>)],
    ) -> Vec<Report> {
        let mut diagnosis_set = FxHashSet::default();
        let body = self.tcx.instance_mir(instance.def);
        let raw_ptrs = self.collect_raw_ptrs(body);
        if raw_ptrs.is_empty() {
            return vec![];
        }
        let drops = self.collect_drops(body, manual_drops);
        let pts = alias_analysis.get_or_insert_pts(instance.def_id(), body);
        diagnosis_set.extend(detect_escape_to_global(pts, &drops, body, self.tcx));
        diagnosis_set.extend(detect_escape_to_return_or_param(
            pts, &drops, body, self.tcx,
        ));
        diagnosis_set.extend(detect_use_after_drop(&raw_ptrs, pts, &drops, body));
        diagnosis_set.extend(detect_captured_ptr_escape(
            &raw_ptrs, pts, &drops, body, self.tcx, instance,
        ));
        diagnosis_set.into_iter().map(|diagnosis| Report::UseAfterFree(ReportContent::new("UseAfterFree".to_owned(), "Possibly".to_owned(), diagnosis, "Raw ptr is used or escapes the current function after the pointed value is dropped".to_owned()))).collect::<Vec<_>>()
    }

    fn collect_raw_ptrs(&self, body: &Body<'tcx>) -> FxHashSet<Local> {
        body.local_decls
            .iter_enumerated()
            .filter_map(|(local, local_decl)| {
                if local_decl.ty.is_raw_ptr() {
                    Some(local)
                } else {
                    None
                }
            })
            .collect()
    }

    fn collect_drops(
        &self,
        body: &'tcx Body<'tcx>,
        manual_drops: &[(Location, Place<'tcx>)],
    ) -> Vec<(Location, Place<'tcx>)> {
        let mut collector = AutoDropCollector::new(self.tcx, body);
        collector.visit_body(body);
        let mut drops = super::filter_moved_out_drops(collector.finish(), body);
        drops.extend(manual_drops.iter().cloned());
        drops
    }
}

/// Collect raw ptrs escaping to Globals.
/// Alloc(ptr) pointed to by ConstantDeref implies Place(ptr) escapes to Global.
/// 1. forall c is ConstantDeref, collect pts(c) into S
/// 2. find Alloc(ptr) in S and map it to Place(ptr)
///
/// Returns (Place(ptr), c)
fn collect_raw_ptrs_escape_to_global<'tcx>(
    pts: &PointsToMap<'tcx>,
    body: &Body<'tcx>,
    tcx: TyCtxt<'tcx>,
) -> FxHashSet<(ConstraintNode<'tcx>, ConstraintNode<'tcx>)> {
    let local_end = Local::new(body.local_decls().len());
    pts.iter()
        .filter_map(|(ptr, ptes)| {
            if let ConstraintNode::ConstantDeref(_) = ptr {
                Some((ptr, ptes))
            } else {
                None
            }
        })
        .flat_map(|(ptr, ptes)| ptes.iter().map(|pte| (pte, ptr.clone())))
        .filter_map(|(pte, ptr)| match pte {
            ConstraintNode::Alloc(place)
                if place.local < local_end && place.ty(body, tcx).ty.is_raw_ptr() =>
            {
                Some((ConstraintNode::Place(*place), ptr))
            }
            _ => None,
        })
        .collect::<FxHashSet<_>>()
}

/// Raw ptr escapes to Global and points to a dropped place
fn detect_escape_to_global<'tcx>(
    pts: &PointsToMap<'tcx>,
    drops: &[(Location, Place<'tcx>)],
    body: &Body<'tcx>,
    tcx: TyCtxt<'tcx>,
) -> FxHashSet<String> {
    let mut diagnosis_set = FxHashSet::default();
    let escapes = collect_raw_ptrs_escape_to_global(pts, body, tcx);
    for (escape, constant) in escapes {
        let ptes = match pts.get(&escape) {
            Some(ptes) => ptes,
            None => continue,
        };
        for pte in ptes {
            let place = match pte {
                ConstraintNode::Place(place) => place,
                _ => continue,
            };
            for (location, drop) in drops.iter() {
                // See `detect_escape_to_return_or_param`: an `Arc`/`Rc`
                // handle drop is not a definite free.
                if super::is_refcounted_handle_drop(tcx, body, drop) {
                    continue;
                }
                if drop.as_ref() == *place {
                    let escape_span = match escape {
                        ConstraintNode::Place(ptr) => body.local_decls[ptr.local].source_info.span,
                        _ => continue,
                    };
                    let diagnosis = format!("Escape to Global: Raw ptr {:?} at {:?} escapes to {:?} but pointee is dropped at {:?}", escape, escape_span, constant, body.source_info(*location).span);
                    diagnosis_set.insert(diagnosis);
                }
            }
        }
    }
    diagnosis_set
}

/// Raw ptr escapes to return/params and points to a dropped place
/// Raw ptr points to Alloc(return/params) implies ptr escapes to return/params
/// 1. Find places X alias with param/return: Place(X) -> Alloc(Param/Return)
/// 2. Find raw ptr Y alias with X: Place(X) -> Alloc(Y)
/// 3. Find pts(Y) that is dropped
fn detect_escape_to_return_or_param<'tcx>(
    pts: &PointsToMap<'tcx>,
    drops: &[(Location, Place<'tcx>)],
    body: &Body<'tcx>,
    tcx: TyCtxt<'tcx>,
) -> FxHashSet<String> {
    let mut diagnosis_set = FxHashSet::default();
    let first_non_param_local = Local::new(body.arg_count);
    let local_end = Local::new(body.local_decls().len());
    for (ptr, ptes) in pts {
        // Place X
        let ptr = match ptr {
            ConstraintNode::Place(ptr) => ptr,
            _ => continue,
        };
        // Alias with param/return and raw ptr
        let mut alias_with_params = Vec::new();
        let mut alias_with_raw_ptrs = Vec::new();
        for pte in ptes {
            match pte {
                ConstraintNode::Alloc(pte) => {
                    if pte.local < first_non_param_local {
                        alias_with_params.push(pte)
                    } else if pte.local < local_end
                        && pte.projection.is_empty()
                        && pte.ty(body, tcx).ty.is_raw_ptr()
                    {
                        alias_with_raw_ptrs.push(pte)
                    }
                }
                _ => continue,
            };
        }
        if alias_with_params.is_empty() {
            continue;
        }
        // Raw ptr points to a dropped place
        for raw_ptr in alias_with_raw_ptrs {
            let ptes = match pts.get(&ConstraintNode::Place(*raw_ptr)) {
                Some(ptes) => ptes,
                None => continue,
            };
            for pte in ptes {
                let pte_place = match pte {
                    ConstraintNode::Place(place) => place,
                    _ => continue,
                };
                for (location, drop_place) in drops {
                    if body.basic_blocks[location.block].is_cleanup {
                        continue;
                    }
                    // Dropping an `Arc`/`Rc` handle only frees the pointee when
                    // it is the last reference, which is not knowable here.
                    if super::is_refcounted_handle_drop(tcx, body, drop_place) {
                        continue;
                    }
                    if drop_place.as_ref() == *pte_place {
                        let ptr_span = body.local_decls[ptr.local].source_info.span;
                        let diagnosis = format!("Escape to Param/Return: Raw ptr {:?} at {:?} escapes to {:?} but pointee is dropped at {:?}", ptr, ptr_span, alias_with_params, body.source_info(*location).span);
                        diagnosis_set.insert(diagnosis);
                    }
                }
            }
        }
    }
    diagnosis_set
}

/// Detect raw pointers captured by closures that escape the function
/// through its return value (#123): e.g. `foo() -> Box<dyn Fn()>` captures
/// `data.as_ptr()` into a `move` closure and returns it, while `data` is
/// dropped when `foo` returns. Calling the returned closure later uses a
/// dangling pointer.
fn detect_captured_ptr_escape<'tcx>(
    raw_ptrs: &FxHashSet<Local>,
    pts: &PointsToMap<'tcx>,
    drops: &[(Location, Place<'tcx>)],
    body: &'tcx Body<'tcx>,
    tcx: TyCtxt<'tcx>,
    instance: &Instance<'tcx>,
) -> FxHashSet<String> {
    // Only functions returning an Fn-like type let a closure escape their
    // lifetime; spawned closures are a separate pattern.
    let fn_like = returns_fn_like(tcx, instance);
    if !fn_like {
        return FxHashSet::default();
    }
    let mut diagnosis_set = FxHashSet::default();
    for (block, block_data) in body.basic_blocks.iter_enumerated() {
        for (idx, statement) in block_data.statements.iter().enumerate() {
            let StatementKind::Assign(boxed) = &statement.kind else {
                continue;
            };
            let (dest, rvalue) = &**boxed;
            let Rvalue::Aggregate(kind, ops) = rvalue else {
                continue;
            };
            if !matches!(&**kind, AggregateKind::Closure(..)) {
                continue;
            }
            for op in ops.iter() {
                // A `move` closure captures non-Copy upvars with `Move`, but
                // raw pointers are `Copy`, so their capture lowers to `Copy`
                // while the original local stays alive in the function.
                let (Operand::Move(place) | Operand::Copy(place)) = op else {
                    continue;
                };
                if !place.projection.is_empty() || !raw_ptrs.contains(&place.local) {
                    continue;
                }
                let capture_loc = Location {
                    block,
                    statement_index: idx,
                };
                let ptr_node = ConstraintNode::Place(Place::from(place.local).as_ref());
                let Some(ptes) = pts.get(&ptr_node) else {
                    continue;
                };
                for (drop_loc, drop_place) in drops {
                    if body.basic_blocks[drop_loc.block].is_cleanup {
                        continue;
                    }
                    // A bare drop frees everything rooted at the local.
                    if !ptes.iter().any(|pte| {
                        matches!(pte, ConstraintNode::Place(p) | ConstraintNode::Alloc(p) if p.local == drop_place.local)
                    }) {
                        continue;
                    }
                    let capture_span = body.source_info(capture_loc).span;
                    let diagnosis = format!(
                        "Raw ptr {:?} at {:?} is captured by a closure returned from this function; the pointee is dropped at {:?}",
                        place.local,
                        capture_span,
                        body.source_info(*drop_loc).span
                    );
                    diagnosis_set.insert(diagnosis);
                }
            }
            let _ = dest;
        }
    }
    diagnosis_set
}

/// Whether the function returns an Fn-like type (`dyn Fn`/`impl Fn`/fn
/// pointer, possibly wrapped, e.g. `Box<dyn Fn()>`).
fn returns_fn_like<'tcx>(tcx: TyCtxt<'tcx>, instance: &Instance<'tcx>) -> bool {
    let ty = instance.ty(tcx, TypingEnv::fully_monomorphized());
    let TyKind::FnDef(..) = ty.kind() else {
        return false;
    };
    let sig = tcx.fn_sig(instance.def_id());
    let sig = instance.instantiate_mir_and_normalize_erasing_regions(
        tcx,
        TypingEnv::fully_monomorphized(),
        sig,
    );
    contains_fn_like(tcx, sig.skip_binder().output())
}

fn contains_fn_like(tcx: TyCtxt<'_>, ty: ty::Ty<'_>) -> bool {
    match ty.kind() {
        TyKind::FnPtr(..) => true,
        TyKind::Dynamic(predicates, ..) => predicates.iter().any(|predicate| {
            matches!(predicate.skip_binder(), ExistentialPredicate::Trait(trait_ref) if {
                let path = tcx.def_path_str(trait_ref.def_id);
                path.ends_with("::Fn") || path.ends_with("::FnMut") || path.ends_with("::FnOnce")
            })
        }),
        TyKind::Adt(_, substs) => substs.types().any(|t| contains_fn_like(tcx, t)),
        TyKind::Ref(_, inner, _) | TyKind::RawPtr(inner, _) => contains_fn_like(tcx, *inner),
        TyKind::Tuple(fields) => fields.iter().any(|t| contains_fn_like(tcx, t)),
        _ => false,
    }
}

// drop(place): raw_ptr -> place
// drop_loc reaches use(raw_ptr)
fn detect_use_after_drop<'tcx>(
    raw_ptrs: &FxHashSet<Local>,
    pts: &PointsToMap<'tcx>,
    drops: &[(Location, Place<'tcx>)],
    body: &Body<'tcx>,
) -> FxHashSet<String> {
    let mut diagnosis_set = FxHashSet::default();
    for raw_ptr in raw_ptrs {
        let raw_ptr_node = ConstraintNode::Place(Place::from(*raw_ptr).as_ref());
        let ptes = match pts.get(&raw_ptr_node) {
            Some(ptes) => ptes,
            None => continue,
        };
        let raw_ptr_use_locations = find_uses(body, *raw_ptr);
        for pte in ptes {
            let pte = match pte {
                ConstraintNode::Place(pte) => pte,
                _ => continue,
            };
            for (drop_loc, drop_place) in drops {
                if drop_place.as_ref() != *pte {
                    continue;
                }
                for use_loc in &raw_ptr_use_locations {
                    if is_reachable(*drop_loc, *use_loc, body) {
                        let diagnosis = format!(
                            "Raw ptr is used at {:?} after dropped at {:?}",
                            body.source_info(*use_loc).span,
                            body.source_info(*drop_loc).span
                        );
                        diagnosis_set.insert(diagnosis);
                    }
                }
            }
        }
    }
    diagnosis_set
}
