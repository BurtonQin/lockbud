use std::{cell::RefCell, rc::Rc};
use rustc_index::bit_set::BitSet;
use rustc_middle::{ty::TyCtxt, mir::{Body, Local, BasicBlock, StatementKind, TerminatorKind, Rvalue, Operand}};
use rustc_mir_dataflow::Analysis;
use rustc_mir_dataflow::CallReturnPlaces;
use super::ty::{Lifetimes};



pub fn analyze_lifetimes<'tcx, 'a>(tcx: TyCtxt<'tcx>, body: &'a Body<'tcx>, lifetimes: Rc<RefCell<Lifetimes>>){
    let analysis = LifetimeAnalysis::new(tcx, body, lifetimes);

    analysis
    .into_engine(tcx, body)
    .iterate_to_fixpoint();

}
struct LifetimeAnalysis<'tcx, 'a> {
    tcx: TyCtxt<'tcx>,
    body: &'a Body<'tcx>,
    lifetimes: Rc<RefCell<Lifetimes>>
}

impl<'tcx, 'a> LifetimeAnalysis<'tcx, 'a> {
    fn new(
        tcx: TyCtxt<'tcx>,
        body: &'a Body<'tcx>,
        lifetimes: Rc<RefCell<Lifetimes>>
    ) -> Self {
        Self {
            tcx,
            body,
            lifetimes
        }
    }
}


impl<'tcx, 'a> rustc_mir_dataflow::AnalysisDomain<'tcx> for LifetimeAnalysis<'tcx, 'a> {
    const NAME: &'static str = "MutexLifetimeAnalysis";

    fn bottom_value(&self, body: &Body<'tcx>) -> Self::Domain {
        BitSet::new_empty(body.local_decls.len())
    }

    fn initialize_start_block(&self, _body: &Body<'tcx>, _: &mut Self::Domain) {
        // no borrows of code region_scopes have been taken prior to
        // function execution, so this method has no effect.
    }

    type Domain = BitSet<Local>;

    type Direction = rustc_mir_dataflow::Forward;
}


impl<'tcx, 'a> rustc_mir_dataflow::Analysis<'tcx> for LifetimeAnalysis<'tcx, 'a> {
    fn apply_statement_effect(
        &self,
        state: &mut Self::Domain,
        statement: &rustc_middle::mir::Statement<'tcx>,
        location: rustc_middle::mir::Location,
    ) {
        
        match statement.kind {
            StatementKind::StorageLive(local) => {
                state.insert(local);
            }
            StatementKind::StorageDead(local) => {
                state.remove(local);
            }
            StatementKind::Assign( box (_lhs, ref rhs)) => {
                match rhs {
                    Rvalue::Use(op) => {
                        match op {
                            Operand::Move(moved_place) => {
                                state.remove(moved_place.local);
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }

        let mut lts = self.lifetimes.borrow_mut();
        let body_id = self.body.source.def_id();
        for livelocal in state.iter() {
            lts.add_live_loc(body_id, livelocal, location, statement.source_info.span);
        }
    }

    fn apply_terminator_effect(
        &self,
        state: &mut Self::Domain,
        terminator: &rustc_middle::mir::Terminator<'tcx>,
        location: rustc_middle::mir::Location,
    ) {
        let body_id = self.body.source.def_id();
        let mut lts = self.lifetimes.borrow_mut();

        match &terminator.kind {
            TerminatorKind::Drop {place, ..} => {
                state.remove(place.local);
            }
            TerminatorKind::Call {..} => {
                for livelocal in state.iter() {
                    lts.add_live_loc(body_id, livelocal, location, terminator.source_info.span);
                }
            }
            _ => {}
        }
    }

    fn apply_call_return_effect(
        &self,
        _state: &mut Self::Domain,
        _block: BasicBlock,
        _return_places: CallReturnPlaces<'_, 'tcx>
    ) {
        
    }
}