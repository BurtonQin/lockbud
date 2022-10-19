//! Detect atomicity violation caused by misuse of atomic variables.
//! Currently only support the following two patterns:
//! ```no_run
//! // atomic::store is control dep on atomic::load
//! if atomic.load(order) == v1 {
//!     atomic.store(v2, order);
//! }
//!
//! // atomic::store is data dep on atomic::load
//! let v1 = atomic_load(order);
//! let v2 = v1 + 1;
//! atomic.store(v2, order);
//! ```
extern crate rustc_data_structures;
extern crate rustc_hash;
extern crate rustc_middle;
use rustc_hash::FxHashMap;
use rustc_middle::mir::{BasicBlock, Body, Local, Location, Place, TerminatorKind};
use rustc_middle::ty::TyCtxt;

pub mod report;
use crate::analysis::callgraph::{CallGraph, InstanceId};
use crate::analysis::controldep;
use crate::analysis::datadep;
use crate::analysis::defuse;
use crate::analysis::pointsto::{AliasAnalysis, AliasId, ApproximateAliasKind};
use crate::detector::report::{Report, ReportContent};
use crate::interest::concurrency::atomic::AtomicApi;
use report::AtomicityViolationDiagnosis;

use petgraph::visit::IntoNodeReferences;
use std::collections::BTreeSet;

pub struct AtomicityViolationDetector<'tcx> {
    tcx: TyCtxt<'tcx>,
}

impl<'tcx> AtomicityViolationDetector<'tcx> {
    pub fn new(tcx: TyCtxt<'tcx>) -> Self {
        Self { tcx }
    }

    /// Collect atomic APIs.
    /// Rerturn the atomic API's InstanceId and kind.
    fn collect_atomics(&self, callgraph: &CallGraph<'tcx>) -> FxHashMap<InstanceId, AtomicApi> {
        callgraph
            .graph
            .node_references()
            .filter_map(|(instance_id, node)| {
                AtomicApi::from_instance(*node.instance(), self.tcx)
                    .map(|atomic_api| (instance_id, atomic_api))
            })
            .collect()
    }

    /// Detect atomicity violation intra-procedurally and returns bug report.
    pub fn detect<'a>(
        &mut self,
        callgraph: &'a CallGraph<'tcx>,
        alias_analysis: &mut AliasAnalysis<'a, 'tcx>,
    ) -> Vec<Report> {
        let mut reports = Vec::new();
        let atomic_apis = self.collect_atomics(callgraph);
        if atomic_apis.is_empty() {
            return Vec::new();
        }
        let mut atomic_reads = FxHashMap::default();
        let mut atomic_writes = FxHashMap::default();
        let mut atomic_read_writes = FxHashMap::default();
        for (instance_id, atomic_api) in atomic_apis {
            let callers = callgraph.callers(instance_id);
            match atomic_api {
                AtomicApi::Read => atomic_reads.insert(instance_id, callers),
                AtomicApi::Write => atomic_writes.insert(instance_id, callers),
                AtomicApi::ReadWrite => atomic_read_writes.insert(instance_id, callers),
            };
        }
        for (atomic_read, read_callers) in atomic_reads {
            for (atomic_write, write_callers) in atomic_writes.iter() {
                // Only track direct callers of atomic APIs.
                // Find the caller that calls both Atomic::load and Atomic::store
                let common_callers = read_callers
                    .iter()
                    .filter(|caller| write_callers.contains(caller))
                    .collect::<Vec<_>>();
                // Find the callsites of Atomic APIs
                for caller in common_callers {
                    let read_write_callsites = atomic_read_writes
                        .iter()
                        .filter_map(|(atomic_read_write, read_write_callers)| {
                            if read_write_callers.contains(caller) {
                                callsite_locations(callgraph, *caller, *atomic_read_write)
                            } else {
                                None
                            }
                        })
                        .flatten()
                        .collect::<Vec<Location>>();
                    let body = self
                        .tcx
                        .instance_mir(callgraph.index_to_instance(*caller).unwrap().instance().def);
                    let mut uses_cache = FxHashMap::default();
                    let control_deps = controldep::control_deps(&body.basic_blocks);
                    let data_deps = datadep::data_deps(body);
                    let read_callsites =
                        callsite_locations(callgraph, *caller, atomic_read).unwrap();
                    let write_callsites =
                        callsite_locations(callgraph, *caller, *atomic_write).unwrap();
                    for read_callsite in read_callsites {
                        for write_callsite in &write_callsites {
                            let dep_kind = match atomic_uses_influences(
                                read_callsite,
                                *write_callsite,
                                (*caller, body),
                                alias_analysis,
                                &mut uses_cache,
                                &data_deps,
                                &control_deps,
                            ) {
                                Some(dep_kind) => dep_kind,
                                None => {
                                    continue;
                                }
                            };
                            if !read_write_callsites.iter().any(|read_write_callsite| {
                                matches!(
                                    atomic_uses_influences(
                                        *read_write_callsite,
                                        *write_callsite,
                                        (*caller, body),
                                        alias_analysis,
                                        &mut uses_cache,
                                        &data_deps,
                                        &control_deps,
                                    ),
                                    Some(DependenceKind::Control) | Some(DependenceKind::Both)
                                )
                            }) {
                                let fn_name = self.tcx.def_path_str(
                                    callgraph
                                        .index_to_instance(*caller)
                                        .unwrap()
                                        .instance()
                                        .def_id(),
                                );
                                let atomic_reader =
                                    format!("{:?}", body.source_info(read_callsite).span);
                                let atomic_writer =
                                    format!("{:?}", body.source_info(*write_callsite).span);
                                let dep_kind = format!("{dep_kind:?}");
                                let diagnosis = AtomicityViolationDiagnosis {
                                    fn_name,
                                    atomic_reader,
                                    atomic_writer,
                                    dep_kind,
                                };
                                let report_content = ReportContent::new(
                                    "AtomicityViolation".to_owned(),
                                    "Possibly".to_owned(),
                                    diagnosis,
                                    "atomic::store is data/control dependent on atomic::load"
                                        .to_owned(),
                                );
                                reports.push(Report::AtomicityViolation(report_content));
                            }
                        }
                    }
                }
            }
        }
        reports
    }
}

/// Get the first arg and the destination of an Atomic API.
/// e.g.,
/// ```let value = atomic::load(atomic, ordering);```
/// Returns Some((atomic, value));
fn first_arg_and_dest<'tcx>(
    location: Location,
    body: &Body<'tcx>,
) -> Option<(Place<'tcx>, Place<'tcx>)> {
    if let TerminatorKind::Call {
        func: _func,
        args,
        destination,
        ..
    } = &body[location.block].terminator().kind
    {
        args.get(0)
            .and_then(|arg| arg.place())
            .map(|place| (place, *destination))
    } else {
        None
    }
}

fn first_two_args<'tcx>(
    location: Location,
    body: &Body<'tcx>,
) -> Option<(Place<'tcx>, Option<Place<'tcx>>)> {
    if let TerminatorKind::Call {
        func: _func, args, ..
    } = &body[location.block].terminator().kind
    {
        let place0 = args.get(0)?.place()?;
        let place1 = args.get(1).and_then(|arg1| arg1.place());
        Some((place0, place1))
    } else {
        None
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
enum DependenceKind {
    Control,
    Data,
    Both,
}

/// Returns the uses locations of `influencer` that may data or control influence `influencee`
/// For
fn atomic_uses_influences<'tcx>(
    influencer: Location,
    influencee: Location,
    instance_body: (InstanceId, &Body<'tcx>),
    alias_analysis: &mut AliasAnalysis<'_, 'tcx>,
    uses_cache: &mut FxHashMap<Local, BTreeSet<Location>>,
    data_deps: &datadep::DataDeps,
    control_deps: &controldep::ControlDeps<BasicBlock>,
) -> Option<DependenceKind> {
    let (instance_id, body) = instance_body;
    let (influencer_arg, influencer_dest) = first_arg_and_dest(influencer, body)?;
    let (influencee_arg, value_arg) = first_two_args(influencee, body)?;
    // atomic in influencer may alias atomic in influencee
    let influencer_id = AliasId {
        instance_id,
        local: influencer_arg.local,
    };
    let influencee_id = AliasId {
        instance_id,
        local: influencee_arg.local,
    };
    let alias_kind = alias_analysis.alias(influencer_id, influencee_id);
    if alias_kind < ApproximateAliasKind::Possibly {
        return None;
    }
    let local = influencer_dest.local;
    // locals that is data dep on influencer
    let deps = datadep::all_data_dep_on(local, data_deps);
    // data dependent
    let is_data_dep = match value_arg {
        Some(value_arg) => deps.contains(&value_arg.local),
        None => false,
    };
    // uses of influencer and its deps influence influencee
    let mut use_locs_influenced = Vec::new();
    for dep in deps.into_iter().chain(std::iter::once(local)) {
        let use_locs = uses_cache
            .entry(dep)
            .or_insert_with(|| defuse::find_uses(body, dep));
        use_locs_influenced.extend(
            use_locs
                .iter()
                .cloned()
                .filter(|use_loc| controldep::influences(*use_loc, influencee, control_deps)),
        );
    }
    if use_locs_influenced.is_empty() {
        if is_data_dep {
            Some(DependenceKind::Data)
        } else {
            None
        }
    } else if is_data_dep {
        Some(DependenceKind::Both)
    } else {
        Some(DependenceKind::Control)
    }
}

/// CallSite Locations from source to target
fn callsite_locations(
    callgraph: &CallGraph<'_>,
    source: InstanceId,
    target: InstanceId,
) -> Option<Vec<Location>> {
    Some(
        callgraph
            .callsites(source, target)?
            .into_iter()
            .filter_map(|callsite| callsite.location())
            .collect(),
    )
}
