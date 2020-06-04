/// A simple assumption to track lockguard to lock:
/// For two places: A and B
/// if A = move B:
/// then A depends on B by move
/// if A = B:
/// then A depends on B by copy
/// if A = &B or A = &mut B
/// then A depends on B by ref
/// if A = call func(move B)
/// then A depends on B by call
extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_mir;

use rustc_middle::mir::visit::*;
use rustc_middle::mir::*;
use rustc_mir::util::def_use::DefUseAnalysis;

use std::collections::HashMap;

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct DependPair<'tcx>(Place<'tcx>, Place<'tcx>);

pub type DependCache<'tcx> = HashMap<DependPair<'tcx>, DependResult>;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum DependResult {
    MoveDepend,
    CopyDepend,
    RefDepend,
    CallDepend,
}

pub struct BatchDependResults<'a, 'b, 'tcx> {
    depend_query_info: DependQueryInfo<'tcx>,
    pub body: &'a Body<'tcx>,
    def_use_analysis: &'b DefUseAnalysis,
}

impl<'a, 'b, 'tcx> BatchDependResults<'a, 'b, 'tcx> {
    pub fn new(body: &'a Body<'tcx>, def_use_analysis: &'b DefUseAnalysis) -> Self {
        Self {
            depend_query_info: DependQueryInfo::<'tcx>::new(),
            body,
            def_use_analysis,
        }
    }

    pub fn get_depends(&self, place: Place<'tcx>) -> Vec<(Place<'tcx>, DependResult)> {
        self.depend_query_info.get_depends(place)
    }

    pub fn gen_depends(&mut self, place: Place<'tcx>) {
        let use_info = self.def_use_analysis.local_info(place.local);
        for u in &use_info.defs_and_uses {
            match u.context {
                PlaceContext::MutatingUse(MutatingUseContext::Store) => {
                    assert!(!is_terminator_location(&u.location, self.body));
                    let stmt = &self.body.basic_blocks()[u.location.block].statements
                        [u.location.statement_index];
                    if let StatementKind::Assign(box (lhs, ref rvalue)) = stmt.kind {
                        if lhs != place {
                            continue;
                        }
                        match rvalue {
                            Rvalue::Use(operand) => {
                                match operand {
                                    Operand::Move(rhs) => {
                                        self.depend_query_info.add_depend(
                                            DependPair(lhs, *rhs),
                                            DependResult::MoveDepend,
                                        );
                                    }
                                    Operand::Copy(rhs) => {
                                        self.depend_query_info.add_depend(
                                            DependPair(lhs, *rhs),
                                            DependResult::CopyDepend,
                                        );
                                    }
                                    _ => {
                                        // TODO
                                    }
                                };
                            }
                            Rvalue::Ref(_, _, rhs) => {
                                self.depend_query_info
                                    .add_depend(DependPair(lhs, *rhs), DependResult::RefDepend);
                            }
                            _ => {
                                // TODO
                            }
                        }
                    }
                }
                PlaceContext::MutatingUse(MutatingUseContext::Call) => {
                    assert!(is_terminator_location(&u.location, self.body));
                    let term = self.body.basic_blocks()[u.location.block].terminator();
                    if let TerminatorKind::Call {
                        func: _,
                        ref args,
                        destination: Some((lhs, _)),
                        ..
                    } = term.kind
                    {
                        if lhs != place {
                            continue;
                        }
                        // heuristically consider the first move arg to be associated with return.
                        // TODO: check the type relations to decide if they are related.
                        for arg in args {
                            if let Operand::Move(rhs) = arg {
                                self.depend_query_info
                                    .add_depend(DependPair(lhs, *rhs), DependResult::CallDepend);
                                break;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

pub struct DependQueryInfo<'tcx> {
    depend_cache: DependCache<'tcx>,
}

impl<'tcx> DependQueryInfo<'tcx> {
    pub fn new() -> Self {
        Self {
            depend_cache: HashMap::<DependPair<'tcx>, DependResult>::new(),
        }
    }

    pub fn get_depends(&self, place: Place<'tcx>) -> Vec<(Place<'tcx>, DependResult)> {
        self.depend_cache
            .iter()
            .filter_map(|(pair, result)| {
                if pair.0 == place {
                    Some((pair.1, *result))
                } else {
                    None
                }
            })
            .collect()
    }

    fn add_depend(&mut self, pair: DependPair<'tcx>, result: DependResult) {
        self.depend_cache.entry(pair).or_insert(result);
    }
}

fn is_terminator_location(location: &Location, body: &Body) -> bool {
    location.statement_index >= body.basic_blocks()[location.block].statements.len()
}
