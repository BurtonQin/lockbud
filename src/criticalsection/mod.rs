

use std::{collections::{HashMap, HashSet}, rc::Rc, cell::RefCell, fs::{File, self}, env, ffi::OsString, fmt::Display};

use log::info;
use rustc_hir::def_id::{LOCAL_CRATE, LocalDefId, DefId};
use rustc_middle::{ty::{TyCtxt, Ty, InstanceDef, TypeFoldable}, mir::{Body, Local, Terminator, StatementKind, TerminatorKind}};
use rustc_span::Symbol;
use serde::{Serialize, Deserialize};


use self::{ty::{Lifetimes, Lifetime}, lifetime::analyze_lifetimes, lock::parse_lockguard_type, call_graph::CallSite, range::parse_span};


mod ty;
mod lifetime;
mod lock;
mod call_graph;
mod range;

use self::call_graph::analyze_callgraph;


#[derive(Hash, Eq, PartialEq, Copy, Clone, Serialize, Deserialize, Debug)]
pub enum CriticalSectionCall {
    ChSend,
    ChRecv,
    CondVarWait
}

impl Display for CriticalSectionCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CriticalSectionCall::ChSend => write!(f, "{}", "channel send"),
            CriticalSectionCall::ChRecv => write!(f, "{}", "channel recv"),
            CriticalSectionCall::CondVarWait =>  write!(f, "{}", "conditional variable wait"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HighlightArea {
    // filename, start line & col, end line & col
    pub ranges: Vec<(String, u32, u32, u32, u32)>
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CallInCriticalSection {
    // filename, start line & col, end line & col
    pub callchains: Vec<(String, u32, u32, u32, u32)>,
    pub ty: CriticalSectionCall,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub calls: Vec<CallInCriticalSection>,
    pub critical_sections: Vec<HighlightArea>
}

fn callchains_to_spans<'tcx>(callchains:& Vec<CallSite<'tcx>>) -> Vec<(String, u32, u32, u32, u32)> {
    callchains.iter()
    .map(|c| {
        let (filename, rg) = parse_span(&c.span);
        return (filename, rg.0.0, rg.0.1, rg.1.0, rg.1.1)
    })
    .collect()
}

fn lifetime_to_highlight_area(l: &Lifetime) -> HighlightArea {
    HighlightArea {
        ranges:l.live_span.iter()
        .map(|c| {
            let (filename, rg) = parse_span(&c);
            return (filename, rg.0.0, rg.0.1, rg.1.0, rg.1.1)
        })
        .collect(),
    }
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
type CSCallFilterSet<'tcx> = HashMap<CriticalSectionCall, &'tcx CSCallFilter<'tcx>>;

pub fn find_in_lifetime<'tcx, 'a>(tcx: TyCtxt<'tcx>, body: &'a Body<'tcx>, lt:&Lifetime, callgraph: &call_graph::CallGraph<'tcx>, cs_calls:& mut Vec<CallInCriticalSection>, callchains:Vec<CallSite<'tcx>>, filter_set:&CSCallFilterSet<'tcx>) {
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
        //         info!("checking call: {:?} {:?}", caller_ty_name, fname);
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

        let mut cs_call_type: Option<CriticalSectionCall> = None;
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
                    cs_calls.push(CallInCriticalSection{
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
                cs_calls.push(CallInCriticalSection{
                    callchains: callchains_to_spans(&new_cc),
                    ty: cs_call_type.unwrap(),
                })
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


pub fn analyze(tcx: TyCtxt) -> Result<AnalysisResult, Box<dyn std::error::Error>>  {
    let crate_name = tcx.crate_name(LOCAL_CRATE).to_string();
    let trimmed_name: String = crate_name.trim_matches('\"').to_string();
    let dl_crate_res = env::var("__DL_CRATE");
    let dl_black_crates_res = env::var("__DL_BLACK_CRATES");
    let dl_white_src_prefix_res = env::var("__DL_WHITE_SRC_PREFIX");
    let dl_out_res = env::var("__DL_OUT");
    let mut result = AnalysisResult {
        calls: Vec::new(),
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

    info!("critical section analyzing crate {:?}", trimmed_name);
    
    
    let fn_ids: Vec<LocalDefId> = tcx.mir_keys(())
    .iter()
    .filter(|id| {
        let hir = tcx.hir();
        hir.body_owner_kind(**id)
            .is_fn_or_closure()
    })
    .copied()
    .collect();

    info!("functions: {}", fn_ids.len());
    let lifetimes = Rc::new(RefCell::new(Lifetimes::new()));
    let mut callgraph = call_graph::CallGraph::new();
    fn_ids
    .clone()
    .into_iter()
    .for_each(|fn_id| {
        info!("analyzing {:?}", fn_id);
        let body = tcx.optimized_mir(fn_id);
        analyze_lifetimes(tcx, body, lifetimes.clone());
        analyze_callgraph(tcx, body, &mut callgraph);
    });

    // fill critical section into result
    // let mut all_lifetime:Vec<Lifetime> = (&lifetimes.borrow().body_local_lifetimes).values().into_iter().map(|hm|hm.values()).flatten().map(|l| l.clone()).collect();
    // let mut areas:Vec<HighlightArea> = all_lifetime.iter().map(|l| lifetime_to_highlight_area(l)).collect();
    // result.critical_sections.append(&mut areas);
    
    let mut filter_set: CSCallFilterSet = HashMap::new();

    // send has buffer, so ignore it
    // filter_set.insert(CriticalSectionCall::ChSend, &|tcx, cs|{
    //     match cs.call_by_type {
    //         Some(caller_ty) => {
    //             let fname = tcx.item_name(cs.callee.def_id());
    //             let caller_ty_name = caller_ty.to_string();
    //             return caller_ty_name.contains("std::sync::mpsc::Sender") && fname.to_string() == "send";
    //         },
    //         None => false,
    //     }
    // } );

    filter_set.insert(CriticalSectionCall::ChRecv, &|tcx, cs|{
        match cs.call_by_type {
            Some(caller_ty) => {
                let fname = tcx.item_name(cs.callee.def_id());
                let caller_ty_name = caller_ty.to_string();
                return caller_ty_name.contains("std::sync::mpsc") && fname.to_string() == "recv";
            },
            None => false,
        }
    } );

    filter_set.insert(CriticalSectionCall::CondVarWait, &|tcx, cs|{
        match cs.call_by_type {
            Some(caller_ty) => {
                let fname = tcx.item_name(cs.callee.def_id());
                let caller_ty_name = caller_ty.to_string();
                return caller_ty_name.contains("std::sync::Condvar") && fname.to_string() == "wait";
            },
            None => false,
        }
    } );
    
    for (fn_id, local_lifetimes) in &lifetimes.borrow().body_local_lifetimes {
        let body = tcx.optimized_mir(*fn_id);

        let interested_locals = filter_body_locals(body, |ty| {
            match parse_lockguard_type(&ty) {
                Some(guard) => {
                    return true;
                },
                None => {},
            }
            false
        });

        info!("{:?}: {} lockguards", fn_id, interested_locals.len());

        for il in interested_locals {
            if !local_lifetimes.contains_key(&il) {
                continue;
            }
            let lft = &local_lifetimes[&il];
            find_in_lifetime(tcx, body, lft, &callgraph, &mut result.calls, vec![], &filter_set);
        }


    }

    if let Some(out) = dl_out_res.ok() {
        let out_file = std::path::Path::new(&out);
        fs::create_dir_all(out_file.parent().unwrap())?;
        serde_json::to_writer(&File::create(out_file)?, &result)?;
    }
    
    info!("possible blocking {:?} calls", result.calls.len());
    if result.calls.len() > 0 {
        info!("{:?}", result.calls);
    }

    // for sec in &result.critical_sections {
    //     info!("body Id {:?}", sec.body_id);
    //     for sp in &sec.live_span {
    //         info!("span {:?}", sp);
    //     }
    // }

    return Ok(result)
}
