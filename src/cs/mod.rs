

use std::{collections::{HashMap, HashSet}, rc::Rc, cell::RefCell, fs::{File, self}, env, fmt::Display, hash::Hash};

use log::{debug, warn};
use rustc_hir::def_id::{LOCAL_CRATE, LocalDefId};
use rustc_middle::{ty::{TyCtxt, Ty, InstanceDef}, mir::{Body, Local, Location}};
use serde::{Serialize, Deserialize};


use crate::{detector::lock::Report, cs::{range::parse_span_str, diagnostics::AnalysisResult, cli::BeautifiedCallInCriticalSection}};

use self::{ty::{Lifetimes, Lifetime}, lifetime::analyze_lifetimes, lock::parse_lockguard_type, call_graph::CallSite, range::parse_span, diagnostics::{Suspicious, SuspiciousCall, HighlightArea}};


mod ty;
mod lifetime;
mod lock;
mod call_graph;
mod range;
mod diagnostics;
mod cli;


use self::call_graph::analyze_callgraph;




fn callchains_to_spans<'tcx>(callchains:& Vec<CallSite<'tcx>>) -> Vec<(String, u32, u32, u32, u32)> {
    callchains.iter()
    .filter_map(|c| {
        if let Some((filename, rg)) = parse_span(&c.span) {
            return Some((filename, rg.0.0, rg.0.1, rg.1.0, rg.1.1))
        } else {
            None
        }
    })
    .collect()
}



pub fn filter_body_locals(body: &Body, filter: fn(Ty) -> bool) -> Vec<Local> {
    body.local_decls
    .iter_enumerated()
    .filter_map(|(local, decl)|{
        if filter(decl.ty) {
            return Some(local)
        } 

        None
    })
    .collect()
}

type CSCallFilter<'tcx> = dyn Fn(TyCtxt<'tcx>, &CallSite) -> bool;
type CSCallFilterSet<'tcx> = HashMap<Suspicious, &'tcx CSCallFilter<'tcx>>;

pub fn find_in_lifetime<'tcx, 'a>(tcx: TyCtxt<'tcx>, body: &'a Body<'tcx>, lt:&Lifetime, callgraph: &call_graph::CallGraph<'tcx>, cs_calls:& mut HashSet<SuspiciousCall>, callchains:Vec<CallSite<'tcx>>, filter_set:&CSCallFilterSet<'tcx>) {
    if callchains.len() > 20 {
        // eprintln!("find_in_lifetime callchain too long, skip");
        return;
    }
    // let symbols: Vec<Symbol> = callchains.iter().map(|c| tcx.item_name(c.callee.def_id())).collect();
    // eprintln!("find_in_lifetime {:?}", symbols);
    let callsites = &callgraph.callsites[&body.source.def_id()];
    for cs in callsites {
        let callee_id = cs.callee.def_id();
        // match cs.call_by_type {
        //     Some(caller_ty) => {
        //         let fname = tcx.item_name(cs.callee.def_id());
        //         let caller_ty_name = caller_ty.to_string();
        //         debug!("checking call: {:?} {:?}", caller_ty_name, fname);
        //     },
        //     None => {},
        // }
        // check recursion
        for ch in &callchains {
            if ch.callee.def_id() == callee_id {
                // eprintln!("recursive found: {:?}, skip", tcx.item_name(callee_id));

                continue;
            }
        }

        if tcx.is_constructor(cs.callee.def_id()) {
            continue;
        }


        match cs.callee.def {
            InstanceDef::Item(_) => {
                // If there is no MIR available (either because it was not in metadata or
                // because it has no MIR because it's an extern function), then the inliner
                // won't cause cycles on this.
                if !tcx.is_mir_available(callee_id) {
                    continue;
                }
            }
            // These have no own callable MIR.
            InstanceDef::Intrinsic(_) | InstanceDef::Virtual(..) => continue,
            // These have MIR and if that MIR is inlined, substituted and then inlining is run
            // again, a function item can end up getting inlined. Thus we'll be able to cause
            // a cycle that way
            InstanceDef::VtableShim(_)
            | InstanceDef::ReifyShim(_)
            | InstanceDef::FnPtrShim(..)
            | InstanceDef::ClosureOnceShim { .. }
            | InstanceDef::CloneShim(..) => {}
            InstanceDef::DropGlue(..) => {
                // FIXME: A not fully substituted drop shim can cause ICEs if one attempts to
                // have its MIR built. Likely oli-obk just screwed up the `ParamEnv`s, so this
                // needs some more analysis.
                // if cs.callee.needs_subst() {
                //     continue;
                // }
                continue;
            }
        }

        let mut cs_call_type: Option<Suspicious> = None;
        for (t, f) in filter_set {
            if f(tcx, cs) {
                cs_call_type = Some(*t);
                break;
            }
        }
        // if callchain is 0, means body has critical section but looking whether a call is inside of critical section
        if callchains.len() == 0 {
            for loc in &lt.live_locs {
                if cs.location != *loc {
                    continue
                }
                let mut new_cc = callchains.clone();
                new_cc.push(cs.clone());
                if cs_call_type != None {
                    // if this call is in critical section and is our interests
                    cs_calls.insert(SuspiciousCall{
                        callchains: callchains_to_spans(&new_cc),
                        ty:cs_call_type.unwrap(),
                    });
                    break
                } 
                else {
                    // if this call is in critical section and is not our interests
                    if !tcx.is_mir_available(callee_id) {
                        continue;
                    }
                    let callee_body = tcx.optimized_mir(callee_id);
                    find_in_lifetime(tcx, callee_body, lt, callgraph, cs_calls, new_cc, filter_set)
                }
            }
        } else {
            // if the whole body is in the critical section 
            let mut new_cc = callchains.clone();
            new_cc.push(cs.clone());
            if cs_call_type != None {
                // if this call is in critical section and is our interests
                cs_calls.insert(SuspiciousCall{
                    callchains: callchains_to_spans(&new_cc),
                    ty: cs_call_type.unwrap(),
                });
            } else {
                // if this call is in critical section and is not our interests
                if !tcx.is_mir_available(callee_id) {
                    continue;
                }
                let callee_body = tcx.optimized_mir(callee_id);
                find_in_lifetime(tcx, callee_body, lt, callgraph, cs_calls, new_cc, filter_set)
            }
        }
        
    }
}

fn lifetime_to_highlight_area(l: &Lifetime) -> HighlightArea {
    HighlightArea {
        triggers: l.init_at.iter()
        .filter_map(|c| {
            if let Some((filename, rg)) = parse_span(&c) {
                Some((filename, rg.0.0, rg.0.1, rg.1.0, rg.1.1))
            } else {
                None
            }
             
        })
        .collect(),
        ranges:l.live_span.iter()
        .filter_map(|c| {
            if let Some((filename, rg)) = parse_span(&c) {
                Some((filename, rg.0.0, rg.0.1, rg.1.0, rg.1.1))
            } else {
                None
            }
             
        })
        .collect(),
    }
}

pub fn check_cond_var_waits<'tcx, 'a>(tcx: TyCtxt<'tcx>, body: &'a Body<'tcx>, lt:&Lifetime, callgraph: &call_graph::CallGraph<'tcx>, cs_calls:& mut HashSet<SuspiciousCall>, callchains:Vec<CallSite<'tcx>>, loc_to_locals: &HashMap<Location, HashSet<Local>>) {

    if callchains.len() > 10 {
        debug!("find_in_lifetime callchain too long, skip");
        return;
    }

    let callsites = &callgraph.callsites[&body.source.def_id()];
    for cs in callsites {
        let callee_id = cs.callee.def_id();
        match cs.call_by_type {
            Some(caller_ty) => {
                let fname = tcx.item_name(cs.callee.def_id());
                let caller_ty_name = caller_ty.to_string();
                debug!("checking call: {:?} {:?}", caller_ty_name, fname);
            },
            None => {},
        }
        // check recursion
        for ch in &callchains {
            if ch.callee.def_id() == callee_id {
                debug!("recursive found: {:?}, skip", tcx.item_name(callee_id));
                continue;
            }
        }

        if tcx.is_constructor(cs.callee.def_id()) {
            continue;
        }


        match cs.callee.def {
            InstanceDef::Item(_) => {
                // If there is no MIR available (either because it was not in metadata or
                // because it has no MIR because it's an extern function), then the inliner
                // won't cause cycles on this.
                if !tcx.is_mir_available(callee_id) {
                    continue;
                }
            }
            // These have no own callable MIR.
            InstanceDef::Intrinsic(_) | InstanceDef::Virtual(..) => continue,
            InstanceDef::VtableShim(_)
            | InstanceDef::ReifyShim(_)
            | InstanceDef::FnPtrShim(..)
            | InstanceDef::ClosureOnceShim { .. }
            | InstanceDef::CloneShim(..) => {}
            InstanceDef::DropGlue(..) => {

                continue;
            }
        }

        let mut is_cond_wait = false;
        match cs.call_by_type {
            Some(caller_ty) => {
                let fname = tcx.item_name(cs.callee.def_id());
                let caller_ty_name = caller_ty.to_string();
                is_cond_wait = caller_ty_name.contains("std::sync::Condvar") && fname.to_string() == "wait";
            },
            None => {},
        }
        // if callchain is 0, means body has critical section but looking whether a call is inside of critical section
        if callchains.len() == 0 {
            for loc in &lt.live_locs {
                if cs.location != *loc {
                    continue
                }
                let mut new_cc = callchains.clone();
                new_cc.push(cs.clone());
                let default_set = HashSet::new();
                let guards_set = loc_to_locals.get(loc).unwrap_or(&default_set);
                let num_of_guards = guards_set.len();

                // 2 is because is the original lock + the alias of the lock (we can update to 1 if we add alias analysis)
                if is_cond_wait && num_of_guards > 2 {

                    // if this call is in critical section and is our interests
                    cs_calls.insert(SuspiciousCall  {
                        callchains: callchains_to_spans(&new_cc),
                        ty:Suspicious::CondVarWait,
                    });
                    break
                } 
                else {
                    // if this call is in critical section and is not our interests
                    if !tcx.is_mir_available(callee_id) {
                        continue;
                    }
                    //let callee_body = tcx.optimized_mir(callee_id);
                    //check_cond_var_waits(tcx, callee_body, lt, callgraph, cs_calls, new_cc)
                }
            }
        } else {
            // if the whole body is in the critical section 
            let mut new_cc = callchains.clone();
            new_cc.push(cs.clone());
            if is_cond_wait {
                // if this call is in critical section and is our interests
                cs_calls.insert(SuspiciousCall {
                    callchains: callchains_to_spans(&new_cc),
                    ty: Suspicious::CondVarWait,
                });
            } else {
                // if this call is in critical section and is not our interests
                if !tcx.is_mir_available(callee_id) {
                    continue;
                }
                //let callee_body = tcx.optimized_mir(callee_id);
                //check_cond_var_waits(tcx, callee_body, lt, callgraph, cs_calls, new_cc)
            }
        }
        
    }
}

// FIXME: temporary patch for print boqin's report and this together
pub fn analyze(tcx: TyCtxt, boqin_reports: Option<Vec<Report>>) -> Result<AnalysisResult, Box<dyn std::error::Error>>  {
    let crate_name = tcx.crate_name(LOCAL_CRATE).to_string();
    let trimmed_name: String = crate_name.trim_matches('\"').to_string();
    let dl_crate_res = env::var("__DL_CRATE");
    let dl_black_crates_res = env::var("__DL_BLACK_CRATES");
    let dl_white_src_prefix_res = env::var("__DL_WHITE_SRC_PREFIX");
    let dl_out_res = env::var("__DL_OUT");
    let mut result = AnalysisResult {
        calls: HashSet::new(),
        critical_sections: Vec::new(),
    };
    
    if let Some(c) = dl_crate_res.ok() {
        if c != trimmed_name {
                return Ok(result);
            }
    }

    if let Some(black_list_crates) = dl_black_crates_res.ok() {
        let bcs = black_list_crates.split(",");
        for bc in bcs {
            if bc == trimmed_name {
                return Ok(result);
            }
        }
    }

    if let Some(dl_white_src_prefix) = dl_white_src_prefix_res.ok() {
        if let Some(src) = &tcx.sess.local_crate_source_file {
            let abs_src = src.canonicalize().unwrap();
            if !abs_src.starts_with(dl_white_src_prefix) {
                return Ok(result);
            }

        }
    }

    debug!("critical section analyzing crate {:?}", trimmed_name);
    
    
    let fn_ids: Vec<LocalDefId> = tcx.mir_keys(())
    .iter()
    .filter(|id| {
        let hir = tcx.hir();
        hir.body_owner_kind(**id)
            .is_fn_or_closure()
    })
    .copied()
    .collect();

    debug!("functions: {}", fn_ids.len());
    let lifetimes = Rc::new(RefCell::new(Lifetimes::new()));
    let mut callgraph = call_graph::CallGraph::new();
    fn_ids
    .clone()
    .into_iter()
    .for_each(|fn_id| {
        debug!("analyzing {:?}", fn_id);
        let body = tcx.optimized_mir(fn_id);
        analyze_lifetimes(tcx, body, lifetimes.clone());
        analyze_callgraph(tcx, body, &mut callgraph);
    });

    // fill critical section into result
    let all_lifetime:Vec<Lifetime> = (&lifetimes.borrow().body_local_lifetimes).values().into_iter().map(|hm|hm.values()).flatten().map(|l| l.clone()).collect();
    let mut areas:Vec<HighlightArea> = all_lifetime.iter().map(|l| lifetime_to_highlight_area(l)).collect();
    result.critical_sections.append(&mut areas);
    
    let mut filter_set: CSCallFilterSet = HashMap::new();

    // send has buffer, so ignore it
    // filter_set.insert(Suspicious::ChSend, &|tcx, cs|{
    //     match cs.call_by_type {
    //         Some(caller_ty) => {
    //             let fname = tcx.item_name(cs.callee.def_id());
    //             let caller_ty_name = caller_ty.to_string();
    //             return caller_ty_name.contains("std::sync::mpsc::Sender") && fname.to_string() == "send";
    //         },
    //         None => false,
    //     }
    // } );

    filter_set.insert(Suspicious::ChRecv, &|tcx, cs|{
        match cs.call_by_type {
            Some(caller_ty) => {
                let fname = tcx.item_name(cs.callee.def_id());
                let caller_ty_name = caller_ty.to_string();
                return caller_ty_name.contains("std::sync::mpsc") && fname.to_string() == "recv";
            },
            None => false,
        }
    } );

    // condition variable requires more logic
    // filter_set.insert(Suspicious::CondVarWait, &|tcx, cs|{
    //     match cs.call_by_type {
    //         Some(caller_ty) => {
    //             let fname = tcx.item_name(cs.callee.def_id());
    //             let caller_ty_name = caller_ty.to_string();
    //             return caller_ty_name.contains("std::sync::Condvar") && fname.to_string() == "wait";
    //         },
    //         None => false,
    //     }
    // } );
    
    for (fn_id, local_lifetimes) in &lifetimes.borrow().body_local_lifetimes {
        let body = tcx.optimized_mir(*fn_id);

        let interested_locals = filter_body_locals(body, |ty| {
            match parse_lockguard_type(&ty) {
                Some(_guard) => {
                    return true;
                },
                None => {},
            }
            false
        });

        debug!("{:?}: {} lockguards", fn_id, interested_locals.len());
        let mut loc_to_locals :HashMap<Location, HashSet<Local>> = HashMap::new();
        for il in &interested_locals {
            let lft = &local_lifetimes[&il];
            for lfloc in &lft.live_locs {
                let _ = loc_to_locals.try_insert(*lfloc, HashSet::new());
                loc_to_locals.get_mut(lfloc).unwrap().insert(*il);
            }
        }
        for il in interested_locals {
            if !local_lifetimes.contains_key(&il) {
                continue;
            }
            let lft = &local_lifetimes[&il];
            find_in_lifetime(tcx, body, lft, &callgraph, &mut result.calls, vec![], &filter_set);

            check_cond_var_waits(tcx, body, lft, &callgraph, &mut result.calls, vec![], &loc_to_locals);

        }


    }

    if let Some(breports) = boqin_reports {
        for report in breports {
            match report {
                Report::DoubleLock(content) => {
                    let mut cs = SuspiciousCall {
                        callchains: Vec::new(),
                        ty: Suspicious::DoubleLock
                    };

                    

                    if let Some((filename, rg)) = parse_span_str(&content.diagnosis.first_lock_span) {
                        cs.callchains.push((filename, rg.0.0, rg.0.1, rg.1.0, rg.1.1));

                    } 

                    if let Some((filename, rg)) = parse_span_str(&content.diagnosis.second_lock_span) {
                        cs.callchains.push((filename, rg.0.0, rg.0.1, rg.1.0, rg.1.1));

                    } 
                    for c in &content.diagnosis.callchains {
                        for cc in c {
                            for ccc in cc {

                                if let Some((filename, rg)) = parse_span_str(ccc) {
                                    cs.callchains.push((filename, rg.0.0, rg.0.1, rg.1.0, rg.1.1));

                                } 

                            }
                        }
                    }
                    result.calls.insert(cs);
                    
                },
                Report::ConflictLock(content) => {
                    let mut cs = SuspiciousCall {
                        callchains: Vec::new(),
                        ty: Suspicious::ConflictLock
                    };
                    for d in content.diagnosis {
                        if let Some((filename, rg)) = parse_span_str(&d.first_lock_span) {
                            cs.callchains.push((filename, rg.0.0, rg.0.1, rg.1.0, rg.1.1));
    
                        } 
    
                        if let Some((filename, rg)) = parse_span_str(&d.second_lock_span) {
                            cs.callchains.push((filename, rg.0.0, rg.0.1, rg.1.0, rg.1.1));
    
                        } 
                        for c in &d.callchains {
                            for cc in c {
                                for ccc in cc {
                                    if let Some((filename, rg)) = parse_span_str(ccc) {
                                        cs.callchains.push((filename, rg.0.0, rg.0.1, rg.1.0, rg.1.1))
    
                                    } 
    
                                }
                            }
                        }
                    }   
                    
                    result.calls.insert(cs);
                },
            }
        }
    }


    if let Some(out) = dl_out_res.ok() {
        let out_file = std::path::Path::new(&out);
        fs::create_dir_all(out_file.parent().unwrap())?;
        serde_json::to_writer(&File::create(out_file)?, &result)?;
    }
    
    if result.calls.len() > 0 {
        // beautify output

        let mut bcalls:Vec<BeautifiedCallInCriticalSection> = Vec::new();
        for c in &result.calls  {
            let tystr;
            if c.ty == Suspicious::ChRecv {
                tystr = "RecvInCriticalSection"
            } else if c.ty == Suspicious::CondVarWait {
                tystr = "WaitInCriticalSection"
            } else {
                continue;
            }
            let mut callshains:Vec<String> = Vec::new();
            for cs in &c.callchains {
                callshains.push(format!("{}:{}:{}: {}:{}", cs.0, cs.1, cs.2, cs.3, cs.4))
            }
            let bc = BeautifiedCallInCriticalSection{
                callchains: callshains,
                ty: tystr.to_string(),
            };
            bcalls.push(bc);
        }
        let j = serde_json::to_string_pretty(&bcalls).unwrap();
        warn!("{}", j);
    }


    return Ok(result)
}
