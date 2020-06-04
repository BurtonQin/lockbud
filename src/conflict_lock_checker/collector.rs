extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_mir;

use super::dataflow::*;
use super::lock::*;
use super::lock::{parse_lockguard_type, LockGuardId, LockGuardInfo, LockGuardSrc, LockGuardType};
use super::tracker::{Tracker, TrackerState};
use rustc_hir::def_id::LocalDefId;
use rustc_middle::mir::visit::{
    MutatingUseContext, NonMutatingUseContext, NonUseContext, PlaceContext,
};
use rustc_middle::mir::{
    Body, Local, LocalInfo, Place, ProjectionElem};
use rustc_mir::util::def_use::DefUseAnalysis;
use std::collections::{HashMap, HashSet};

pub fn collect_lockguard_info(
    fn_id: LocalDefId,
    body: &Body,
) -> HashMap<LockGuardId, LockGuardInfo> {
    let mut lockguards: HashMap<LockGuardId, LockGuardInfo> = HashMap::new();
    for (local, local_decl) in body.local_decls.iter_enumerated() {
        if let Some(type_name) = parse_lockguard_type(&local_decl.ty) {
            let lockguard_id = LockGuardId::new(fn_id, local);
            let lockguard_info = LockGuardInfo {
                type_name,
                src: None,
                span: local_decl.source_info.span,
                gen_bbs: Vec::new(),
                kill_bbs: Vec::new(),
            };
            lockguards.insert(lockguard_id, lockguard_info);
        }
    }
    let mut def_use_analysis = DefUseAnalysis::new(body);
    def_use_analysis.analyze(body);
    let lockguards = collect_lockguard_src_info(lockguards, body, &def_use_analysis);
    collect_gen_kill_bbs(lockguards, body, &def_use_analysis)
}

fn batch_gen_depends_for_all<'a, 'b, 'tcx>(
    lockguards: &HashMap<LockGuardId, LockGuardInfo>,
    body: &'a Body<'tcx>,
    def_use_analysis: &'b DefUseAnalysis,
) -> BatchDependResults<'a, 'b, 'tcx> {
    let mut batch_depend_results = BatchDependResults::new(body, def_use_analysis);
    for (id, _) in lockguards {
        batch_gen_depends(id.local, &mut batch_depend_results);
    }
    batch_depend_results
}

fn batch_gen_depends(local: Local, batch_depend_results: &mut BatchDependResults) {
    let local_place = Place::from(local);
    let mut worklist: Vec<Place> = vec![local_place];
    let mut visited: HashSet<Place> = HashSet::new();
    visited.insert(local_place);
    while let Some(place) = worklist.pop() {
        batch_depend_results.gen_depends(place);
        for depend in batch_depend_results
            .get_depends(place)
            .into_iter()
            .map(|(place, _)| place)
        {
            if !visited.contains(&depend) {
                worklist.push(depend);
                visited.insert(depend);
            }
        }
    }
}

fn collect_lockguard_src_info(
    lockguards: HashMap<LockGuardId, LockGuardInfo>,
    body: &Body,
    def_use_analysis: &DefUseAnalysis,
) -> HashMap<LockGuardId, LockGuardInfo> {
    if lockguards.is_empty() {
        return lockguards;
    }
    let batch_depends = batch_gen_depends_for_all(&lockguards, body, def_use_analysis);
    lockguards.into_iter().map(|(id, mut info)| {
        let (place, tracker_result) = match info.type_name.0 {
            LockGuardType::StdMutexGuard | LockGuardType::StdRwLockGuard => {
                let mut tracker = Tracker::new(Place::from(id.local), true, &batch_depends);
                tracker.track()
            }
            _ => {
                let mut tracker = Tracker::new(Place::from(id.local), false, &batch_depends);
                tracker.track()
            }
        };
        info.src = match tracker_result {
            TrackerState::ParamSrc => {
                let fields = place
                    .projection
                    .clone()
                    .into_iter()
                    .filter_map(|e| {
                        if let ProjectionElem::Field(field, _) = e {
                            Some(field)
                        } else {
                            None
                        }
                    })
                    .fold(String::new(), |acc, field| {
                        acc + &format!("{:?}", field) + ","
                    });
                let mut struct_type = body.local_decls[place.local].ty.to_string();
                if struct_type.starts_with("&") {
                    struct_type = struct_type.chars().skip(1).collect();
                }
                let lockguard_src = LockGuardSrc::ParamSrc(ParamSrcContext {
                        struct_type,
                        fields,
                    });
                Some(lockguard_src)
            }
            TrackerState::LocalSrc => {
                let lockguard_src = LockGuardSrc::LocalSrc(LocalSrcContext { place: format!("{:?}", place) });
                Some(lockguard_src)
            }
            TrackerState::WrapperLock => {
                match body.local_decls[place.local].local_info {
                    Some(box LocalInfo::StaticRef {
                        def_id,
                        is_thread_local: _,
                    }) => {
                        let lockguard_src = LockGuardSrc::GlobalSrc(GlobalSrcContext { global_id: def_id });
                        Some(lockguard_src)
                    }
                    _ => {
                        // TODO(boqin): any other non-static-ref lock wrapper?
                        None
                    }
                }
            }
            _ => {
                None
            }
        };
        (id, info)
    }).collect()
}

fn collect_gen_kill_bbs(
    lockguards: HashMap<LockGuardId, LockGuardInfo>,
    _body: &Body,
    def_use_analysis: &DefUseAnalysis,
) -> HashMap<LockGuardId, LockGuardInfo> {
    if lockguards.is_empty() {
        return lockguards;
    }
    lockguards
        .into_iter()
        .filter_map(|(id, mut info)| {
            let mut retain = true;
            let use_info = def_use_analysis.local_info(id.local);
            for u in &use_info.defs_and_uses {
                match u.context {
                    PlaceContext::NonUse(context) => match context {
                        NonUseContext::StorageLive => info.gen_bbs.push(u.location.block),
                        NonUseContext::StorageDead => info.kill_bbs.push(u.location.block),
                        _ => {}
                    },
                    PlaceContext::NonMutatingUse(context) => match context {
                        NonMutatingUseContext::Move => info.kill_bbs.push(u.location.block),
                        _ => {}
                    },
                    PlaceContext::MutatingUse(context) => match context {
                        MutatingUseContext::Drop => info.kill_bbs.push(u.location.block),
                        MutatingUseContext::Store => {
                            retain = false;
                            break;
                        },
                        MutatingUseContext::Call => {},
                        _ => {}
                    },
                }
            }
            if retain {
                Some((id, info))
            } else {
                None
            }
        })
        .collect::<HashMap<_, _>>()
}