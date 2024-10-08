extern crate rustc_hir;
extern crate rustc_span;

use once_cell::sync::Lazy;
use regex::Regex;
use rustc_hir::def_id::{DefId, LOCAL_CRATE};
use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{Body, Location, Terminator, TerminatorKind, OUTERMOST_SOURCE_SCOPE};
use rustc_middle::ty::EarlyBinder;
use rustc_middle::ty::{self, TyCtxt, TyKind};
use rustc_middle::ty::{Instance, InstanceKind};
use rustc_span::Span;
use std::collections::HashMap;

#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum PanicAPI {
    ResultUnwrap,
    ResultExpect,
    OptionUnwrap,
    OptionExpect,
    PanicFmt,
    AssertFailed,
    Panic,
}

static PANIC_API_REGEX: Lazy<HashMap<PanicAPI, Regex>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert(
        PanicAPI::ResultUnwrap,
        Regex::new(r"Result::<.+>::unwrap").unwrap(),
    );
    m.insert(
        PanicAPI::ResultExpect,
        Regex::new(r"Result::<.+>::expect").unwrap(),
    );
    m.insert(
        PanicAPI::OptionUnwrap,
        Regex::new(r"Option::<.+>::unwrap").unwrap(),
    );
    m.insert(
        PanicAPI::OptionExpect,
        Regex::new(r"Option::<.+>::expect").unwrap(),
    );
    m.insert(PanicAPI::PanicFmt, Regex::new(r"rt::panic_fmt").unwrap());
    m.insert(
        PanicAPI::AssertFailed,
        Regex::new(r"panicking::assert_failed").unwrap(),
    );
    m.insert(PanicAPI::Panic, Regex::new(r"panicking::panic").unwrap());
    m
});

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_panic_api_regex() {
        assert!(PANIC_API_REGEX[&PanicAPI::ResultUnwrap].is_match("Result::<i32, String>::unwrap"));
        assert!(PANIC_API_REGEX[&PanicAPI::ResultExpect].is_match("Result::<i32, String>::expect"));
        assert!(PANIC_API_REGEX[&PanicAPI::OptionUnwrap].is_match("Option::<i32>::unwrap"));
        assert!(PANIC_API_REGEX[&PanicAPI::OptionExpect].is_match("Option::<i32>::expect"));
        assert!(PANIC_API_REGEX[&PanicAPI::PanicFmt].is_match("rt::panic_fmt"));
        assert!(PANIC_API_REGEX[&PanicAPI::AssertFailed].is_match("core::panicking::assert_failed"));
        assert!(PANIC_API_REGEX[&PanicAPI::Panic].is_match("core::panicking::panic"));
        assert!(!PANIC_API_REGEX[&PanicAPI::Panic].is_match("no_panic"));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PanicInstance<'tcx> {
    ResultUnwrap(Instance<'tcx>),
    ResultExpect(Instance<'tcx>),
    OptionUnwrap(Instance<'tcx>),
    OptionExpect(Instance<'tcx>),
    PanicFmt(Instance<'tcx>),
    AssertFailed(Instance<'tcx>),
    Panic(Instance<'tcx>),
}

impl<'tcx> PanicInstance<'tcx> {
    fn new(instance: Instance<'tcx>, tcx: TyCtxt<'tcx>) -> Option<Self> {
        let def_path_str = tcx.def_path_str_with_args(instance.def_id(), instance.args);
        if PANIC_API_REGEX[&PanicAPI::ResultUnwrap].is_match(&def_path_str) {
            Some(PanicInstance::ResultUnwrap(instance))
        } else if PANIC_API_REGEX[&PanicAPI::ResultExpect].is_match(&def_path_str) {
            Some(PanicInstance::ResultExpect(instance))
        } else if PANIC_API_REGEX[&PanicAPI::OptionUnwrap].is_match(&def_path_str) {
            Some(PanicInstance::OptionUnwrap(instance))
        } else if PANIC_API_REGEX[&PanicAPI::OptionExpect].is_match(&def_path_str) {
            Some(PanicInstance::OptionExpect(instance))
        } else if PANIC_API_REGEX[&PanicAPI::PanicFmt].is_match(&def_path_str) {
            Some(PanicInstance::PanicFmt(instance))
        } else if PANIC_API_REGEX[&PanicAPI::AssertFailed].is_match(&def_path_str) {
            Some(PanicInstance::AssertFailed(instance))
        } else if PANIC_API_REGEX[&PanicAPI::Panic].is_match(&def_path_str) {
            Some(PanicInstance::Panic(instance))
        } else {
            None
        }
    }

    fn to_panic_api(&self) -> PanicAPI {
        match self {
            PanicInstance::ResultUnwrap(_) => PanicAPI::ResultUnwrap,
            PanicInstance::ResultExpect(_) => PanicAPI::ResultExpect,
            PanicInstance::OptionUnwrap(_) => PanicAPI::OptionUnwrap,
            PanicInstance::OptionExpect(_) => PanicAPI::OptionExpect,
            PanicInstance::PanicFmt(_) => PanicAPI::PanicFmt,
            PanicInstance::AssertFailed(_) => PanicAPI::AssertFailed,
            PanicInstance::Panic(_) => PanicAPI::Panic,
        }
    }
}

pub struct PanicDetector<'tcx> {
    tcx: TyCtxt<'tcx>,
    result: HashMap<(DefId, Location), (Span, Span, PanicInstance<'tcx>)>,
}

impl<'tcx> PanicDetector<'tcx> {
    pub fn new(tcx: TyCtxt<'tcx>) -> Self {
        Self {
            tcx,
            result: Default::default(),
        }
    }
    pub fn detect(&mut self, instance: Instance<'tcx>) {
        if let Some(mut panic_finder) = PanicFinder::new(instance, self.tcx) {
            self.result.extend(panic_finder.detect());
        }
    }
    pub fn result(&self) -> &HashMap<(DefId, Location), (Span, Span, PanicInstance<'tcx>)> {
        &self.result
    }
    pub fn statistics(&self) -> HashMap<PanicAPI, usize> {
        let mut tally: HashMap<PanicAPI, usize> = HashMap::new();
        self.result.iter().for_each(|(_, (_, _, panic_instance))| {
            *tally.entry(panic_instance.to_panic_api()).or_default() += 1;
        });
        tally
    }
}

struct PanicFinder<'tcx> {
    instance: Instance<'tcx>,
    body: &'tcx Body<'tcx>,
    tcx: TyCtxt<'tcx>,
    callsites: HashMap<Location, PanicInstance<'tcx>>,
}

impl<'tcx> PanicFinder<'tcx> {
    fn new(instance: Instance<'tcx>, tcx: TyCtxt<'tcx>) -> Option<Self> {
        if skip_detecting(&instance, tcx) {
            return None;
        }
        // Only detect instances in local crate.
        if instance.def_id().krate != LOCAL_CRATE {
            return None;
        }
        let body = tcx.instance_mir(instance.def);
        Some(Self {
            instance,
            body,
            tcx,
            callsites: Default::default(),
        })
    }

    fn detect(&mut self) -> HashMap<(DefId, Location), (Span, Span, PanicInstance<'tcx>)> {
        self.visit_body(self.body);
        self.callsites
            .iter()
            .map(|(loc, instance)| {
                let def_id = self.instance.def_id();
                let span = self.body.source_info(*loc).span;
                let outermost_span = self.body.source_scopes[OUTERMOST_SOURCE_SCOPE].span;
                ((def_id, *loc), (span, outermost_span, instance.clone()))
            })
            .collect::<_>()
    }
}

impl<'tcx> Visitor<'tcx> for PanicFinder<'tcx> {
    fn visit_terminator(&mut self, terminator: &Terminator<'tcx>, location: Location) {
        if let TerminatorKind::Call { ref func, .. } = terminator.kind {
            let func_ty = func.ty(self.body, self.tcx);
            let func_ty = self.instance.instantiate_mir_and_normalize_erasing_regions(
                self.tcx,
                ty::ParamEnv::reveal_all(),
                EarlyBinder::bind(func_ty),
            );
            if let TyKind::FnDef(def_id, subst_ref) = func_ty.kind() {
                if let Some(callee_instance) =
                    Instance::try_resolve(self.tcx, ty::ParamEnv::reveal_all(), *def_id, subst_ref)
                        .ok()
                        .flatten()
                {
                    if let Some(panic_instance) = PanicInstance::new(callee_instance, self.tcx) {
                        self.callsites.insert(location, panic_instance);
                    }
                }
            }
        }
    }
}

fn skip_detecting<'tcx>(instance: &Instance<'tcx>, tcx: TyCtxt<'tcx>) -> bool {
    if let InstanceKind::Item(_) = instance.def {
        !tcx.is_mir_available(instance.def_id())
    } else {
        true
    }
}
