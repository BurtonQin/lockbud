//! Detect Invalid Free
//! For `a = mem::uninitialized()`:
//! 1. if `a` is of a type that is not simple
//! 2. and there exists `drop(a)`
//! 3. then report invalid-free
//!
//! For `b = MaybeUninit::uninit()` and `c = assume_init(b)`:
//! 1. if there is no `write(b)` in between
//! 2. and `c` is of a type that is not simple
//! 3. and there exists `drop(c)`
//! 4. then report invalid-free
extern crate rustc_data_structures;
extern crate rustc_index;
extern crate rustc_middle;

use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use rustc_index::bit_set::BitSet;
use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{BasicBlock, Body, Location, Place, TerminatorKind};
use rustc_middle::ty::{self, EarlyBinder, Instance, Ty, TyCtxt};

use petgraph::visit::IntoNodeReferences;

use super::{collect_manual_drop, dest_args0, AutoDropCollector};
use crate::analysis::callgraph::{CallSiteLocation, InstanceId};
use crate::analysis::pointsto::{AliasId, ApproximateAliasKind};
use crate::analysis::{callgraph::CallGraph, pointsto::AliasAnalysis};
use crate::detector::report::{Report, ReportContent};
use crate::interest::memory::uninit::UninitApi;

pub struct InvalidFreeDetector<'tcx> {
    tcx: TyCtxt<'tcx>,
}

impl<'tcx> InvalidFreeDetector<'tcx> {
    pub fn new(tcx: TyCtxt<'tcx>) -> Self {
        Self { tcx }
    }

    pub fn detect(
        &self,
        callgraph: &CallGraph<'tcx>,
        alias_analysis: &mut AliasAnalysis,
    ) -> Vec<Report> {
        let uninits = self.collect_uninit(callgraph);
        let caller_callsites = self.collect_caller_callsites(uninits, callgraph);
        let manual_drops = collect_manual_drop(callgraph, self.tcx);
        let mut reports = Vec::new();
        for (caller_id, callsites) in caller_callsites {
            let manual_drops = match manual_drops.get(&caller_id) {
                Some(v) => v.clone(),
                None => Vec::new(),
            };
            if let Some(diagnosis) = self.detect_caller_callsites(
                caller_id,
                &callsites,
                callgraph,
                alias_analysis,
                &manual_drops,
            ) {
                for d in diagnosis {
                    let content = ReportContent::new("InvalidFree".to_owned(), "Possibly".to_owned(), d, "Call mem::uninitialized() or MaybeUninit::uninit() followed by assume_init() without actually write on not simple types".to_owned());
                    reports.push(Report::InvalidFree(content));
                }
            }
        }
        reports
    }

    /// Collect Uninit APIs.
    fn collect_uninit(&self, callgraph: &CallGraph<'tcx>) -> FxHashMap<InstanceId, UninitApi> {
        callgraph
            .graph
            .node_references()
            .filter_map(|(instance_id, node)| {
                UninitApi::from_instance(*node.instance(), self.tcx)
                    .map(|uninit_api| (instance_id, uninit_api))
            })
            .collect()
    }

    /// collect CallerId X (CallSiteLocation X UninitApi X Callee).
    fn collect_caller_callsites(
        &self,
        uninits: FxHashMap<InstanceId, UninitApi>,
        callgraph: &CallGraph<'tcx>,
    ) -> FxHashMap<InstanceId, FxHashSet<(Location, UninitApi, InstanceId)>> {
        let mut caller_callsites: FxHashMap<InstanceId, FxHashSet<_>> = FxHashMap::default();
        for (callee, uninit_api) in uninits.iter() {
            let callers = callgraph.callers(*callee);
            for caller in callers {
                if let Some(callsites) = callgraph.callsites(caller, *callee) {
                    let entry = caller_callsites.entry(caller).or_default();
                    for callsite in callsites {
                        if let CallSiteLocation::Direct(loc) = callsite {
                            entry.insert((loc, *uninit_api, *callee));
                        }
                    }
                }
            }
        }
        caller_callsites
    }

    /// Detect each caller that calls Uninit APIs.
    fn detect_caller_callsites(
        &self,
        caller_id: InstanceId,
        callsites: &FxHashSet<(Location, UninitApi, InstanceId)>,
        callgraph: &CallGraph<'tcx>,
        alias_analysis: &mut AliasAnalysis,
        manual_drops: &[(Location, Place<'tcx>)],
    ) -> Option<Vec<String>> {
        let mut diagnosis_vec = Vec::new();
        let caller = callgraph
            .index_to_instance(caller_id)
            .map(|n| n.instance())?;
        let body = self.tcx.instance_mir(caller.def);
        let mut maybe_uninits = Vec::new();
        let mut assume_inits = Vec::new();
        let mut writes = Vec::new();
        let mut auto_drop_collector = AutoDropCollector::new();
        auto_drop_collector.visit_body(body);
        let mut drops = auto_drop_collector.finish();
        drops.extend(manual_drops.iter().cloned());
        for (loc, api, _callee_id) in callsites {
            let loc = *loc;
            match api {
                UninitApi::Uninitialized => {
                    if let Some(diagnosis) = self.detect_uninitialized(
                        caller_id,
                        caller,
                        body,
                        loc,
                        &drops,
                        alias_analysis,
                    ) {
                        diagnosis_vec.push(diagnosis);
                    }
                }
                UninitApi::MaybeUninit => {
                    if let Some((dest, _)) = dest_args0(body, loc) {
                        maybe_uninits.push((loc, dest));
                    }
                }
                UninitApi::MaybeUninitWrite | UninitApi::PtrWrite => {
                    if let Some((_, Some(place0))) = dest_args0(body, loc) {
                        writes.push((loc, place0));
                    }
                }
                UninitApi::AssumeInit => {
                    if let Some((dest, Some(place0))) = dest_args0(body, loc) {
                        assume_inits.push((loc, dest, place0));
                    }
                }
            }
        }
        // candidates = MaybeUninits X AssumeInits X Dest(MaybeUninits)
        let mut candidates = Vec::new();
        for (loc1, dest1) in maybe_uninits {
            for (loc2, dest2, first_arg) in &assume_inits {
                // let obj: T = assume_init();
                // T should not be simple types like bool, i32, etc.
                // T should be complex types like Vec, MyStruct, etc.
                let dest2_ty = self.monomorphize(caller, dest2.ty(body, self.tcx).ty);
                if dest2_ty.is_simple_ty() {
                    continue;
                }
                let dest1_ty = self.monomorphize(caller, dest1.ty(body, self.tcx).ty);
                let first_arg_ty = self.monomorphize(caller, first_arg.ty(body, self.tcx).ty);
                // let obj1: MaybeUninit<T> = uninit();
                // let obj3: T = obj.assume_init(obj2);
                // Here obj1 returned by uninit and obj2 taken by assume_init should be of the same type.
                if dest1_ty != first_arg_ty {
                    continue;
                }
                // obj1 and obj2 should alias with each other
                let aid1 = AliasId {
                    instance_id: caller_id,
                    local: dest1.local,
                };
                let aid2 = AliasId {
                    instance_id: caller_id,
                    local: first_arg.local,
                };
                let is_aliased = alias_analysis.alias(aid1, aid2) > ApproximateAliasKind::Unlikely;
                // find drop(obj3) or mem::drop(obj3)
                let mut is_dropped = false;
                for (_, drop_place) in &drops {
                    let aid_assume_init = AliasId {
                        instance_id: caller_id,
                        local: dest2.local,
                    };
                    let aid_drop = AliasId {
                        instance_id: caller_id,
                        local: drop_place.local,
                    };
                    if alias_analysis.alias(aid_assume_init, aid_drop)
                        > ApproximateAliasKind::Unlikely
                    {
                        is_dropped = true;
                        break;
                    }
                }
                if is_aliased && is_dropped {
                    candidates.push((loc1, *loc2, dest1));
                }
            }
        }
        // Find paths from MaybeUninit::uninit to assume_init
        // uninit -> write? -> assume_init
        // If there is a write in between, and first_arg(write) points to dest(uninit)
        // then we conservatively consider it not a bug, otherwise diagnosis the bug.
        for (loc1, loc2, dest1) in candidates {
            let paths = self.paths_from_to(loc1, loc2, body);
            for path in paths {
                let mut write_in_between = false;
                for (loc3, first_arg) in &writes {
                    if path.contains(&loc3.block) {
                        let aid1 = AliasId {
                            instance_id: caller_id,
                            local: dest1.local,
                        };
                        let aid2 = AliasId {
                            instance_id: caller_id,
                            local: first_arg.local,
                        };
                        if alias_analysis.points_to(aid2, aid1) > ApproximateAliasKind::Unlikely {
                            write_in_between = true;
                            break;
                        }
                    }
                }
                if !write_in_between {
                    let span1 = body.source_info(loc1).span;
                    let span_str1 = format!("{span1:?}");
                    // skip std lib
                    if span_str1.contains(".rustup/toolchains")
                        && span_str1.contains("lib/rustlib/src/rust/library")
                    {
                        continue;
                    }
                    // skip std lib
                    let span2 = body.source_info(loc2).span;
                    let span_str2 = format!("{span2:?}");
                    if span_str2.contains(".rustup/toolchains")
                        && span_str2.contains("lib/rustlib/src/rust/library")
                    {
                        continue;
                    }
                    let ty = self.monomorphize(caller, dest1.ty(body, self.tcx).ty);
                    let diagnosis = format!(
                        "{:?} = uninit at {:?}, assume_init at {:?}",
                        ty, span1, span2
                    );
                    diagnosis_vec.push(diagnosis);
                }
            }
        }
        Some(diagnosis_vec)
    }

    fn monomorphize(&self, instance: &Instance<'tcx>, ty: Ty<'tcx>) -> Ty<'tcx> {
        instance.instantiate_mir_and_normalize_erasing_regions(
            self.tcx,
            ty::ParamEnv::reveal_all(),
            EarlyBinder::bind(ty),
        )
    }

    /// Detect mem::uninitialized()
    fn detect_uninitialized(
        &self,
        instance_id: InstanceId,
        instance: &Instance<'tcx>,
        body: &Body<'tcx>,
        loc: Location,
        drops: &[(Location, Place<'tcx>)],
        alias_analysis: &mut AliasAnalysis,
    ) -> Option<String> {
        if let TerminatorKind::Call {
            func: _func,
            args: _args,
            destination,
            ..
        } = &body[loc.block].terminator().kind
        {
            let ty = destination.ty(body, self.tcx).ty;
            let ty = self.monomorphize(instance, ty);
            if !ty.is_simple_ty() {
                let span = body.source_info(loc).span;
                let span_str = format!("{span:?}");
                // skip std lib
                if !(span_str.contains(".rustup/toolchains")
                    && span_str.contains("lib/rustlib/src/rust/library"))
                {
                    // find drop(place) s.t. place alias with destination
                    for (_drop_loc, drop_place) in drops {
                        let aid1 = AliasId {
                            instance_id,
                            local: drop_place.local,
                        };
                        let aid2 = AliasId {
                            instance_id,
                            local: destination.local,
                        };
                        if alias_analysis.alias(aid1, aid2) > ApproximateAliasKind::Unlikely {
                            let diagnosis = format!("{ty:?} = mem::uninitialized() at {span_str}");
                            return Some(diagnosis);
                        }
                    }
                }
            }
        }
        None
    }

    /// Find all the paths from loc1 to loc2
    fn paths_from_to(
        &self,
        loc1: Location,
        loc2: Location,
        body: &Body<'tcx>,
    ) -> Vec<Vec<BasicBlock>> {
        let mut paths = Vec::new();
        let mut visited = BitSet::new_empty(body.basic_blocks.len());
        let mut path = Vec::new();
        find_path_recursive(
            loc1.block,
            loc2.block,
            body,
            &mut path,
            &mut visited,
            &mut paths,
        );
        paths
    }
}

fn find_path_recursive(
    u: BasicBlock,
    d: BasicBlock,
    body: &Body<'_>,
    path: &mut Vec<BasicBlock>,
    visited: &mut BitSet<BasicBlock>,
    paths: &mut Vec<Vec<BasicBlock>>,
) {
    visited.insert(u);
    path.push(u);
    if u == d {
        // output
        paths.push(path.clone());
    } else {
        let data = &body[u];
        if let Some(ref term) = data.terminator {
            for succ in term.successors() {
                if !visited.contains(succ) {
                    find_path_recursive(succ, d, body, path, visited, paths);
                }
            }
        }
    }
    path.pop();
    visited.remove(u);
}
