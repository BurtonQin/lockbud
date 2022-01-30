extern crate rustc_hir;
extern crate rustc_middle;

use rustc_hir::def_id::LocalDefId;
use rustc_middle::mir::{BasicBlock, Body, Operand, TerminatorKind};
use rustc_middle::ty::TyKind;
use std::collections::HashMap;
pub struct Callgraph {
    pub direct: HashMap<LocalDefId, HashMap<BasicBlock, LocalDefId>>,
}

impl Callgraph {
    pub fn new() -> Self {
        Self {
            direct: HashMap::new(),
        }
    }

    fn insert_direct(&mut self, caller: LocalDefId, bb: BasicBlock, callee: LocalDefId) {
        if let Some(callees) = self.direct.get_mut(&caller) {
            callees.insert(bb, callee);
        } else {
            let mut callees: HashMap<BasicBlock, LocalDefId> = HashMap::new();
            callees.insert(bb, callee);
            self.direct.insert(caller, callees);
        }
    }

    pub fn generate(&mut self, caller: LocalDefId, body: &Body, crate_fn_ids: &[LocalDefId]) {
        for (bb, bb_data) in body.basic_blocks().iter_enumerated() {
            let terminator = bb_data.terminator();
            if let TerminatorKind::Call { ref func, .. } = terminator.kind {
                if let Operand::Constant(box constant) = func {
                    match constant.literal.ty().kind() {
                        TyKind::FnDef(callee_def_id, _) | TyKind::Closure(callee_def_id, _) => {
                            if let Some(local_callee_def_id) = callee_def_id.as_local() {
                                if crate_fn_ids.contains(&local_callee_def_id) {
                                    self.insert_direct(caller, bb, local_callee_def_id);
                                } else {
                                    // dbg!("The fn/closure is not body owner");
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    pub fn get(&self, fn_id: &LocalDefId) -> Option<&HashMap<BasicBlock, LocalDefId>> {
        if let Some(callsites) = self.direct.get(fn_id) {
            if !callsites.is_empty() {
                return Some(callsites);
            } else {
                return None;
            }
        }
        None
    }

    pub fn _print(&self) {
        for (caller, callees) in &self.direct {
            println!("caller: {:?}", caller);
            for callee in callees {
                println!("\tcallee: {:?}", callee);
            }
        }
    }
}
