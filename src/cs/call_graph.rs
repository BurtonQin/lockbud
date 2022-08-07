use std::collections::{HashMap, HashSet, VecDeque};

use rustc_hir::def_id::DefId;
use rustc_middle::{ty::{TyCtxt, Instance, PolyFnSig, InstanceDef, Ty, subst::Subst}, mir::{Operand, Body, TerminatorKind, visit::Visitor, Location}};
use rustc_span::{Span, Symbol};
use rustc_middle::ty::FnDef;

#[derive(Clone, Debug, Hash, PartialEq)]
pub struct CallSite<'tcx> {
    pub callee: Instance<'tcx>,
    pub fn_sig: PolyFnSig<'tcx>,
    pub args: Vec<Operand<'tcx>>,
    pub call_by_type: Option<Ty<'tcx>>,
    pub span: Span,
    pub location: Location
}

struct BodyCallVisitor<'tcx, 'a> {
    tcx: TyCtxt<'tcx>,
    body: &'a Body<'tcx>,
    callsites: Vec<CallSite<'tcx>>
}

pub struct CallGraph<'tcx> {
    pub calls: HashMap<DefId, HashSet<DefId>>,
    pub callsites: HashMap<DefId, Vec<CallSite<'tcx>>>
}

impl<'tcx> CallGraph<'tcx> {

    pub fn new() -> Self {
        return Self {
            calls: HashMap::new(),
            callsites: HashMap::new(),
        }
    }

    #[allow(dead_code)]
    fn raw_pretty_print(&self) {
        for (caller, callees) in &self.calls {
            println!("caller {:?}, callees: {:?}", caller, callees);
        
        }

    }

    #[allow(dead_code)]
    fn pretty_print(&self, tcx: TyCtxt<'tcx>){
        for (caller, callees) in &self.calls {
            let caller_name = tcx.item_name(*caller);
            let callee_names:Vec<Symbol> = callees.iter().map(|callee| tcx.item_name(*callee)).collect();
            println!("caller {:?}, callees: {:?}", caller_name, callee_names);
        
        }
    }
}
pub fn analyze_callgraph<'tcx, 'a>(tcx: TyCtxt<'tcx>, body: &'a Body<'tcx>, callgraph: & mut CallGraph<'tcx>) {
    let mut worklist: VecDeque<&Body> = VecDeque::new();
    let mut visited_bodies: HashSet<DefId> = HashSet::new();
    worklist.push_back(body);

    while let Some(body_to_visit) = worklist.pop_front() {
        let caller_def_id = body_to_visit.source.def_id();
        if callgraph.calls.contains_key(&caller_def_id) {
            continue
        }
        visited_bodies.insert(caller_def_id);
        let mut visiter = BodyCallVisitor{
            tcx,
            body:body_to_visit,
            callsites: Vec::new()
        };

        
        visiter.visit_body(body_to_visit);
        for cs in &visiter.callsites {
            let callee_def_id = cs.callee.def_id();
            if visited_bodies.contains(&callee_def_id) {
                continue
            }
            match check_mir_is_available(tcx, body_to_visit, &cs.callee) {
                Ok(()) => {},
                Err(_reason) => {
                    //let callee_name = tcx.item_name(callee_def_id);
                    // eprintln!("MIR of {} is unavailable: {}", callee_name, reason);
                    continue;
                }
            }
            let body = tcx.instance_mir(cs.callee.def);
            worklist.push_back(body);
            

            // record call relationship
            if !callgraph.calls.contains_key(&caller_def_id) {
                callgraph.calls.insert(caller_def_id, HashSet::new());
            }
            
            callgraph.calls.get_mut(&caller_def_id).unwrap().insert(callee_def_id);

        }
        callgraph.callsites.insert(caller_def_id, visiter.callsites);
        

    }
}


impl<'tcx, 'a> Visitor<'tcx> for BodyCallVisitor<'tcx, 'a> {

    fn visit_terminator(&mut self, terminator: &rustc_middle::mir::Terminator< 'tcx>, location:rustc_middle::mir::Location) {
        let bbdata = &self.body.basic_blocks()[location.block];
        if bbdata.is_cleanup {
            return self.super_terminator(terminator, location);
        }

        
        if let TerminatorKind::Call { ref args, ref func, .. } = terminator.kind {
            let func_ty = func.ty(self.body, self.tcx);
            if let FnDef(def_id, substs) = *func_ty.kind() {
                // To resolve an instance its substs have to be fully normalized.
                let param_env = self.tcx.param_env_reveal_all_normalized(self.body.source.def_id());
                let substs_res = self.tcx.try_normalize_erasing_regions(param_env, substs);
                if substs_res.is_err() {
                    return self.super_terminator(terminator, location);
                }
                let substs = substs_res.unwrap();

                let result =
                    Instance::resolve(self.tcx, param_env, def_id, substs);

                let calleeopt = match result {
                    Ok(opt) => {
                        opt
                    },
                    Err(_) => {
                        None
                    },
                };

                match calleeopt {
                    Some(callee) => {
                        if let InstanceDef::Virtual(..) | InstanceDef::Intrinsic(_) = callee.def {
                            return self.super_terminator(terminator, location);
                        }
                        let fn_sig = self.tcx.bound_fn_sig(def_id).subst(self.tcx, substs);
                        
                        // if function is called like a.hello(arg1, arg2)
                        // args' length is 3, a, arg1, arg2
                        // substs' length is 2, arg1 and arg 2
                        let mut call_by_type = None;
                        if args.len() != substs.len() && args.len() > 0 {
                            let caller_obj = &args[0];
                            let caller_ty = caller_obj.ty(self.body, self.tcx);
                            call_by_type = Some(caller_ty);
                        }
                        let callsite = CallSite {
                            callee,
                            fn_sig,
                            args: args.clone(),
                            call_by_type,
                            span: self.body.source_info(location).span,
                            location,
                        };

                        self.callsites.push(callsite);
                    },
                    None => {

                    },
                }

                

                
                self.super_terminator(terminator, location);
            }
        }
    }


}

fn check_mir_is_available<'tcx>(
    tcx: TyCtxt,
    caller_body: &Body<'tcx>,
    callee: &Instance<'tcx>,
) -> Result<(), &'static str> {
    if callee.def_id() == caller_body.source.def_id() {
        return Err("self-recursion");
    }

    match callee.def {
        InstanceDef::Item(_) => {
            // If there is no MIR available (either because it was not in metadata or
            // because it has no MIR because it's an extern function), then the inliner
            // won't cause cycles on this.
            if !tcx.is_mir_available(callee.def_id()) {
                return Err("item MIR unavailable");
            }
        }
        // These have no own callable MIR.
        InstanceDef::Intrinsic(_) | InstanceDef::Virtual(..) => {
            return Err("instance without MIR (intrinsic / virtual)");
        }
        // This cannot result in an immediate cycle since the callee MIR is a shim, which does
        // not get any optimizations run on it. Any subsequent inlining may cause cycles, but we
        // do not need to catch this here, we can wait until the inliner decides to continue
        // inlining a second time.
        InstanceDef::VtableShim(_)
        | InstanceDef::ReifyShim(_)
        | InstanceDef::FnPtrShim(..)  // TODO: debug here
        | InstanceDef::ClosureOnceShim { .. }
        | InstanceDef::DropGlue(..)
        | InstanceDef::CloneShim(..) => return Err("ignore shim or drop glue"),
    }


    Ok(())
    
}
