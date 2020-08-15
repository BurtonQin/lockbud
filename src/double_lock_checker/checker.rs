extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_span;
use super::callgraph::Callgraph;
use super::collector::collect_lockguard_info;
use super::config::{CrateNameLists, CALLCHAIN_DEPTH};
use super::genkill::GenKill;
use super::lock::{DoubleLockInfo, LockGuardId, LockGuardInfo};
use super::report::DoubleLockReports;
use rustc_hir::def_id::{LocalDefId, LOCAL_CRATE};
use rustc_middle::mir::BasicBlock;
use rustc_middle::ty::TyCtxt;
use rustc_span::Span;

use std::collections::HashMap;
use std::collections::HashSet;
use std::cell::RefCell;
struct FnLockContext {
    fn_id: LocalDefId,
    context: HashSet<LockGuardId>,
    callchain: Vec<(LocalDefId, BasicBlock)>,
}

impl FnLockContext {
    fn new(
        fn_id: LocalDefId,
        context: HashSet<LockGuardId>,
        callchain: Vec<(LocalDefId, BasicBlock)>,
    ) -> Self {
        Self {
            fn_id,
            context,
            callchain,
        }
    }
}
pub struct DoubleLockChecker {
    crate_name_lists: CrateNameLists,
    crate_lockguards: HashMap<LockGuardId, LockGuardInfo>,
    crate_callgraph: Callgraph,
    crate_doublelock_reports: RefCell<DoubleLockReports>, 
}

impl DoubleLockChecker {
    pub fn new(is_white: bool, crate_name_lists: Vec<String>) -> Self {
        if is_white {
            Self {
                crate_name_lists: CrateNameLists::White(crate_name_lists),
                crate_lockguards: HashMap::new(),
                crate_callgraph: Callgraph::new(),
                crate_doublelock_reports: RefCell::new(DoubleLockReports::new()),
            }
        } else {
            Self {
                crate_name_lists: CrateNameLists::Black(crate_name_lists),
                crate_lockguards: HashMap::new(),
                crate_callgraph: Callgraph::new(),
                crate_doublelock_reports: RefCell::new(DoubleLockReports::new()),
            }
        }
    }
    pub fn check(&mut self, tcx: TyCtxt) {
        let crate_name = tcx.crate_name(LOCAL_CRATE).to_string();
        match &self.crate_name_lists {
            CrateNameLists::White(lists) => {
                if !lists.contains(&crate_name) {
                    return;
                }
            }
            CrateNameLists::Black(lists) => {
                if lists.contains(&crate_name) {
                    return;
                }
            }
        }
        // println!("{}", crate_name);
        // collect fn
        let ids = tcx.mir_keys(LOCAL_CRATE);
        let fn_ids: Vec<LocalDefId> = ids
            .clone()
            .into_iter()
            .filter(|id| {
                let hir = tcx.hir();
                hir.body_owner_kind(hir.as_local_hir_id(*id))
                    .is_fn_or_closure()
            })
            .collect();
        // println!("fn_ids: {:#?}", fn_ids);
        // collect lockguard_info
        let lockguards: HashMap<LocalDefId, HashMap<LockGuardId, LockGuardInfo>> = fn_ids
            .clone()
            .into_iter()
            .filter_map(|fn_id| {
                // println!("{:?}", fn_id);
                let body = tcx.optimized_mir(fn_id);
                let lockguards = collect_lockguard_info(fn_id, body);
                if lockguards.is_empty() {
                    None
                } else {
                    Some((fn_id, lockguards))
                }
            })
            .collect();
        if lockguards.is_empty() {
            return;
        }
        // println!("{:#?}", lockguards);
        for (_, info) in lockguards.iter() {
            self.crate_lockguards.extend(info.clone().into_iter());
        }
        println!(
            "fn with locks: {}, lockguards num: {}, local fn num: {}",
            lockguards.len(),
            self.crate_lockguards.len(),
            fn_ids.len()
        );
        // generate callgraph
        // self.crate_callgraph.generate_mono(tcx);
        let mono_map = self.crate_callgraph.gen_mono(&fn_ids, tcx);
        // println!("{:#?}", mono_map);
        for fn_id in &fn_ids {
            self.crate_callgraph
                .generate(*fn_id, &fn_ids, &mono_map, tcx);
        }
        // self.crate_callgraph._print();
        for (fn_id, _) in lockguards.iter() {
            // self.check_entry_fn(&tcx, *fn_id);
            self.check_entry_fn2(&tcx, *fn_id);
        }
        self.crate_doublelock_reports.borrow().pretty_print();
    }

    fn check_entry_fn2(&mut self, tcx: &TyCtxt, fn_id: LocalDefId) {
        let context: HashSet<LockGuardId> = HashSet::new();
        let callchain: Vec<(LocalDefId, BasicBlock)> = Vec::new();

        let mut worklist: Vec<FnLockContext> = Vec::new();
        // visited: fn_id is inserted at most twice (u8 is insertion times, must <= 2)
        let mut visited: HashMap<LocalDefId, u8> = HashMap::new();
        worklist.push(FnLockContext::new(fn_id, context, callchain));
        visited.insert(fn_id, 1);
        while let Some(FnLockContext {
            fn_id,
            context,
            callchain,
        }) = worklist.pop()
        {
            let body = tcx.optimized_mir(fn_id);
            let mut genkill = GenKill::new(fn_id, body, &self.crate_lockguards, &context);
            let double_lock_bugs = genkill.analyze(body);
            if !double_lock_bugs.is_empty() {
                let double_lock_reports = double_lock_bugs
                    .into_iter()
                    .map(|DoubleLockInfo { first, second }| {
                        (
                            self.crate_lockguards.get(&first).unwrap(),
                            self.crate_lockguards.get(&second).unwrap(),
                        )
                    })
                    .collect::<Vec<(&LockGuardInfo, &LockGuardInfo)>>();
                // println!(
                //     "DoubleLockReport: {:#?}",
                //     double_lock_reports
                //         .iter()
                //         .map(|p| (p.0.span, p.1.span))
                //         .collect::<Vec<_>>()
                // );
                let callchain_reports = callchain
                    .iter()
                    .map(move |(fn_id, bb)| {
                        tcx.optimized_mir(*fn_id).basic_blocks()[*bb]
                            .terminator()
                            .source_info
                            .span
                    })
                    .collect::<Vec<Span>>();
                // println!("Callchain: {:#?}", callchain_reports);
                for report in double_lock_reports {
                    self.crate_doublelock_reports.borrow_mut().add(report, &callchain_reports);
                }
            }
            if let Some(callsites) = self.crate_callgraph.get(&fn_id) {
                for (bb, callee_id) in callsites {
                    if let Some(context) = genkill.get_live_lockguards(bb) {
                        // println!("Before {:?} calling {:?}: {:#?}", fn_id, callee_id, context);
                        let mut callchain = callchain.clone();
                        callchain.push((fn_id, *bb));
                        if let Some(times) = visited.get(callee_id) {
                            if *times == 1 {
                                visited.insert(*callee_id, 2);
                                worklist.push(FnLockContext::new(
                                    *callee_id,
                                    context.clone(),
                                    callchain,
                                ));
                            }
                        } else {
                            visited.insert(*callee_id, 1);
                            worklist.push(FnLockContext::new(
                                *callee_id,
                                context.clone(),
                                callchain,
                            ));
                        }
                    }
                }
            }
        }
    }

    fn _check_entry_fn(&self, tcx: &TyCtxt, fn_id: LocalDefId) {
        // println!("checking entry fn: {:?}", fn_id);
        let body = tcx.optimized_mir(fn_id);
        let context = HashSet::new();
        let mut genkill = GenKill::new(fn_id, body, &self.crate_lockguards, &context);
        let double_lock_bugs = genkill.analyze(body);
        if !double_lock_bugs.is_empty() {
            let double_lock_reports = double_lock_bugs
                .into_iter()
                .map(|DoubleLockInfo { first, second }| {
                    (
                        self.crate_lockguards.get(&first).unwrap(),
                        self.crate_lockguards.get(&second).unwrap(),
                    )
                })
                .collect::<Vec<(&LockGuardInfo, &LockGuardInfo)>>();
            println!("DoubleLockReport: {:#?}", double_lock_reports);
        }

        let mut callchain: Vec<(LocalDefId, BasicBlock)> = Vec::new();
        if let Some(callsites) = self.crate_callgraph.get(&fn_id) {
            for (bb, callee_id) in callsites {
                if let Some(context) = genkill.get_live_lockguards(bb) {
                    callchain.push((fn_id, *bb));
                    self.check_fn(&tcx, *callee_id, context, &mut callchain);
                    callchain.pop();
                }
            }
        }
    }

    fn check_fn(
        &self,
        tcx: &TyCtxt,
        fn_id: LocalDefId,
        context: &HashSet<LockGuardId>,
        callchain: &mut Vec<(LocalDefId, BasicBlock)>,
    ) {
        if callchain.len() > CALLCHAIN_DEPTH {
            return;
        }
        // println!("checking fn: {:?}", fn_id);
        let body = tcx.optimized_mir(fn_id);
        let mut genkill = GenKill::new(fn_id, body, &self.crate_lockguards, context);
        let double_lock_bugs = genkill.analyze(body);
        if !double_lock_bugs.is_empty() {
            let double_lock_reports = double_lock_bugs
                .into_iter()
                .map(|DoubleLockInfo { first, second }| {
                    (
                        self.crate_lockguards.get(&first).unwrap(),
                        self.crate_lockguards.get(&second).unwrap(),
                    )
                })
                .collect::<Vec<(&LockGuardInfo, &LockGuardInfo)>>();
            println!("DoubleLockReport: {:#?}", double_lock_reports);
            let callchain_reports = callchain
                .iter()
                .map(move |(fn_id, bb)| {
                    tcx.optimized_mir(*fn_id).basic_blocks()[*bb]
                        .terminator()
                        .source_info
                        .span
                })
                .collect::<Vec<Span>>();
            println!("{:#?}", callchain_reports);
        }

        if let Some(callsites) = self.crate_callgraph.get(&fn_id) {
            for (bb, callee_id) in callsites {
                if let Some(context) = genkill.get_live_lockguards(bb) {
                    callchain.push((fn_id, *bb));
                    self.check_fn(tcx, *callee_id, context, callchain);
                    callchain.pop();
                }
            }
        }
    }
}
