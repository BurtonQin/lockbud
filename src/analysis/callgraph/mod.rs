use petgraph::dot::{Config, Dot};
use petgraph::graph::NodeIndex;
use petgraph::unionfind::UnionFind;
use petgraph::visit::{Bfs, EdgeRef, IntoNodeReferences, NodeIndexable};
use petgraph::{Directed, Graph};

use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{Body, Location, Terminator, TerminatorKind};
use rustc_middle::ty::{self, Instance, ParamEnv, TyCtxt};

pub type InstanceId = NodeIndex;

#[derive(Copy, Clone, Debug)]
pub enum CallSiteLocation {
    FnDef(Location),
    // FnPtr(Location),  // to be supported
}
pub struct CallGraph<'tcx> {
    pub graph: Graph<Instance<'tcx>, Vec<CallSiteLocation>, Directed>,
}

impl<'tcx> CallGraph<'tcx> {
    pub fn new() -> Self {
        Self {
            graph: Graph::new(),
        }
    }

    pub fn instance_to_index(&self, instance: &Instance<'tcx>) -> Option<NodeIndex> {
        self.graph
            .node_references()
            .find(|(_idx, inst)| *inst == instance)
            .map(|(idx, _)| idx)
    }

    pub fn index_to_instance(&self, idx: NodeIndex) -> Option<&Instance<'tcx>> {
        self.graph.node_weight(idx)
    }

    pub fn analyze(&mut self, instances: Vec<Instance<'tcx>>, tcx: TyCtxt<'tcx>, param_env: ParamEnv<'tcx>) {
        let idx_insts = instances
            .into_iter()
            .map(|inst| {
                let idx = self.graph.add_node(inst);
                (idx, inst)
            })
            .collect::<Vec<_>>();
        for (caller_idx, caller) in idx_insts {
            let body = tcx.instance_mir(caller.def);
            // skip promoted src
            if body.source.promoted.is_some() {
                continue;
            }
            let mut collector =
                CallSiteCollector::new(caller, body, tcx, param_env);
            collector.visit_body(body);
            for (callee, location) in collector.callsites() {
                let callee_idx = if let Some(callee_idx) = self
                    .instance_to_index(&callee) {
                        callee_idx
                    }
                else {
                    continue;
                };
                if let Some(edge_idx) = self.graph.find_edge(caller_idx, callee_idx) {
                    // update edge weight
                    self.graph.edge_weight_mut(edge_idx).unwrap().push(location);
                } else {
                    // add edge if not exists
                    self.graph.add_edge(caller_idx, callee_idx, vec![location]);
                }
            }
        }
    }

    // Print the callgraph in dot format
    pub fn dot(&self) {
        println!(
            "{:?}",
            Dot::with_config(&self.graph, &[Config::GraphContentOnly])
        );
    }
}

// Visit Terminator and record callsites (callee + location).
struct CallSiteCollector<'a, 'tcx> {
    caller: Instance<'tcx>,
    body: &'a Body<'tcx>,
    tcx: TyCtxt<'tcx>,
    param_env: ParamEnv<'tcx>,
    callsites: Vec<(Instance<'tcx>, CallSiteLocation)>,
}

impl<'a, 'tcx> CallSiteCollector<'a, 'tcx> {
    fn new(
        caller: Instance<'tcx>,
        body: &'a Body<'tcx>,
        tcx: TyCtxt<'tcx>,
        param_env: ParamEnv<'tcx>,
    ) -> Self {
        Self {
            caller,
            body,
            tcx,
            param_env,
            callsites: Vec::new(),
        }
    }

    fn callsites(self) -> impl IntoIterator<Item = (Instance<'tcx>, CallSiteLocation)> {
        self.callsites.into_iter()
    }
}

impl<'a, 'tcx> Visitor<'tcx> for CallSiteCollector<'a, 'tcx> {
    // Resolve direct call.
    // Inspired by rustc_mir/src/transform/inline.rs#get_valid_function_call.
    fn visit_terminator(&mut self, terminator: &Terminator<'tcx>, location: Location) {
        if let TerminatorKind::Call { ref func, .. } = terminator.kind {
            let func_ty = func.ty(self.body, self.tcx);
            // Only after monomorphizing can Instance::resolve work
            let func_ty = self.caller.subst_mir_and_normalize_erasing_regions(
                self.tcx,
                self.param_env,
                func_ty,
            );
            match *func_ty.kind() {
                ty::FnDef(def_id, substs) =>
                {
                    if let Some(callee) =
                        Instance::resolve(self.tcx, self.param_env, def_id, substs)
                            .ok()
                            .flatten()
                    {
                        self.callsites
                            .push((callee, CallSiteLocation::FnDef(location)));
                    }
                }
                _ => { }
            }
        }
    }
}