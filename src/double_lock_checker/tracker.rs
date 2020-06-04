extern crate rustc_middle;
use super::dataflow::{DependResult, BatchDependResults};
use rustc_middle::mir::*;

#[derive(Debug, Clone, Copy)]
pub enum TrackerState {
    Init,
    Guard,
    Result,
    RefLock,
    WrapperLock,
    LocalSrc,
    ParamSrc,
}

pub struct Tracker<'a, 'b, 'c, 'tcx> {
    state: TrackerState,
    place: Place<'tcx>,
    contain_result: bool,
    batch_depend_results: &'c BatchDependResults<'a, 'b, 'tcx>,
}

impl<'a, 'b, 'c, 'tcx> Tracker<'a, 'b, 'c, 'tcx> {
    pub fn new(
        place: Place<'tcx>,
        contain_result: bool,
        batch_depend_results: &'c BatchDependResults<'a, 'b, 'tcx>,
    ) -> Self {
        Self {
            state: TrackerState::Init,
            place,
            contain_result,
            batch_depend_results,
        }
    }

    pub fn track(&mut self) -> (Place<'tcx>, TrackerState) {
        loop {
            match self.state {
                TrackerState::Init => {
                    if let Some(place) = self.handle_init(self.place, self.contain_result) {
                        self.place = place
                    } else {
                        return (self.place, self.state);
                    }
                }
                TrackerState::Guard => {
                    if let Some(place) = self.handle_guard(self.place) {
                        self.place = place
                    } else {
                        return (self.place, self.state);
                    }
                }
                TrackerState::Result => {
                    if let Some(place) = self.handle_result(self.place) {
                        self.place = place
                    } else {
                        return (self.place, self.state);
                    }
                }
                TrackerState::RefLock => {
                    if let Some(place) = self.handle_reflock(self.place) {
                        self.place = place
                    } else {
                        return (self.place, self.state);
                    }
                }
                TrackerState::WrapperLock => {
                    if let Some(place) = self.handle_wrapperlock(self.place) {
                        self.place = place
                    } else {
                        return (self.place, self.state);
                    }
                }
                TrackerState::LocalSrc => return (self.place, self.state),
                TrackerState::ParamSrc => return (self.place, self.state),
            }
        }
    }

    fn handle_init(&mut self, place: Place<'tcx>, contain_result: bool) -> Option<Place<'tcx>> {
        if self.batch_depend_results.body.local_kind(place.local) == LocalKind::Arg {
            self.state = TrackerState::ParamSrc;
            return None;
        }
        let rhses = self.batch_depend_results.get_depends(place);
        // heuristically assume that any Call is prior than Move, no Call or Move directly return
        // For multi Calls, only randomly select one of the Call
        // MIR is not SSA.
        // Skip checking when there are two lhs defs
        let mut defs = rhses.iter().filter(|(_, result)| {
            *result == DependResult::CallDepend || *result == DependResult::MoveDepend
        });
        if defs.clone().count() > 1 {
            return None;
        }
        match defs.next() {
            Some((place, DependResult::CallDepend)) => {
                if contain_result {
                    self.state = TrackerState::Result;
                } else {
                    self.state = TrackerState::RefLock;
                }
                Some(*place)
            }
            Some((place, DependResult::MoveDepend)) => {
                if contain_result {
                    self.state = TrackerState::Guard;
                } else {
                    self.state = TrackerState::Result;
                }
                Some(*place)
            }
            _ => None,
        }
    }

    fn handle_guard(&mut self, place: Place<'tcx>) -> Option<Place<'tcx>> {
        if self.batch_depend_results.body.local_kind(place.local) == LocalKind::Arg {
            self.state = TrackerState::ParamSrc;
            return None;
        }
        let rhses = self.batch_depend_results.get_depends(place);
        let mut defs = rhses.iter().filter(|(_, result)| {
            *result == DependResult::CallDepend || *result == DependResult::MoveDepend
        });
        if defs.clone().count() > 1 {
            return None;
        }
        match defs.next() {
            Some((place, DependResult::CallDepend)) => {
                self.state = TrackerState::Result;
                Some(*place)
            }
            Some((place, DependResult::MoveDepend)) => {
                self.state = TrackerState::Guard;
                Some(*place)
            }
            _ => None,
        }
    }

    fn handle_result(&mut self, place: Place<'tcx>) -> Option<Place<'tcx>> {
        if self.batch_depend_results.body.local_kind(place.local) == LocalKind::Arg {
            self.state = TrackerState::ParamSrc;
            return None;
        }
        let rhses = self.batch_depend_results.get_depends(place);
        let mut defs = rhses.iter().filter(|(_, result)| {
            *result == DependResult::CallDepend || *result == DependResult::MoveDepend
        });
        if defs.clone().count() > 1 {
            return None;
        }
        match defs.next() {
            Some((place, DependResult::CallDepend)) => {
                self.state = TrackerState::RefLock;
                Some(*place)
            }
            Some((place, DependResult::MoveDepend)) => {
                self.state = TrackerState::Result;
                Some(*place)
            }
            _ => {
                None
            }
        }
    }

    fn handle_reflock(&mut self, place: Place<'tcx>) -> Option<Place<'tcx>> {
        if self.batch_depend_results.body.local_kind(place.local) == LocalKind::Arg {
            self.state = TrackerState::ParamSrc;
            return None;
        }
        let defs = self.batch_depend_results.get_depends(place);
        // heuristically only consider the first one
        match defs.into_iter().next() {
            Some((place, DependResult::RefDepend)) => {
                if self.batch_depend_results.body.local_kind(place.local) == LocalKind::Arg {
                    self.state = TrackerState::ParamSrc;
                } else {
                    self.state = TrackerState::LocalSrc;
                }
                Some(place)
            }
            Some((place, DependResult::CopyDepend)) => {
                Some(place)
            }
            Some((place, DependResult::CallDepend)) => {
                self.state = TrackerState::WrapperLock;
                Some(place)
            }
            _ => {
                None
            }
        }
    }
    fn handle_wrapperlock(&mut self, place: Place<'tcx>) -> Option<Place<'tcx>> {
        if self.batch_depend_results.body.local_kind(place.local) == LocalKind::Arg {
            self.state = TrackerState::ParamSrc;
            return None;
        }
        let defs = self.batch_depend_results.get_depends(place);
        // heuristically only consider the first one
        match defs.into_iter().next() {
            Some((place, DependResult::RefDepend)) => {
                if self.batch_depend_results.body.local_kind(place.local) == LocalKind::Arg {
                    self.state = TrackerState::ParamSrc;
                } else {
                    self.state = TrackerState::LocalSrc;
                }
                Some(place)
            }
            Some((place, DependResult::CopyDepend)) => {
                Some(place)
            }
            Some((place, DependResult::CallDepend)) => {
                self.state = TrackerState::WrapperLock;
                Some(place)
            }
            _ => {
                None
            }
        }
    }
}
