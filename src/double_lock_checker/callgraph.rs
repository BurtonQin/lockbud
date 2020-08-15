extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_mir;

use rustc_hir::def_id::{DefId, LocalDefId, LOCAL_CRATE};
use rustc_middle::mir::visit::*;
use rustc_middle::mir::*;
use rustc_middle::ty::*;
use rustc_mir::util::def_use::DefUseAnalysis;
use std::collections::{HashMap, HashSet};
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

    pub fn generate_mono(&mut self, tcx: TyCtxt) {
        let (_, cgus) = tcx.collect_and_partition_mono_items(LOCAL_CRATE);
        for cgu in cgus {
            let mono_items = cgu.items();
            for mono_item in mono_items {
                if let (mono::MonoItem::Fn(instance), _) = mono_item {
                    if let Instance {
                        def: InstanceDef::Item(def_id),
                        substs,
                    } = instance
                    {
                        if let Some(local_def_id) = def_id.as_local() {
                            // println!("caller: {:?}", local_def_id);
                            let body = tcx.optimized_mir(local_def_id);
                            let mut def_use_analysis = DefUseAnalysis::new(body);
                            def_use_analysis.analyze(body);
                            let mut closures: Vec<(Local, LocalDefId)> = Vec::new();
                            for (local, local_decl) in body.local_decls.iter_enumerated() {
                                match local_decl.ty.kind {
                                    TyKind::Closure(callee_def_id, substs)
                                    | TyKind::Generator(callee_def_id, substs, _) => {
                                        if !callee_def_id.is_local() {
                                            continue;
                                        }
                                        let instance = Instance::resolve(
                                            tcx,
                                            ParamEnv::reveal_all(),
                                            callee_def_id,
                                            substs,
                                        )
                                        .unwrap()
                                        .unwrap();
                                        match instance.monomorphic_ty(tcx).kind {
                                            TyKind::Closure(mono_def_id, substs)
                                            | TyKind::Generator(mono_def_id, substs, _) => {
                                                if let Some(local_mono_def_id) =
                                                    mono_def_id.as_local()
                                                {
                                                    // println!("\tcallee: {:?}", mono_def_id);
                                                    closures.push((local, local_mono_def_id));
                                                } else {
                                                    // println!("\tno-local: {:?}", mono_def_id);
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            for (local, local_mono_def_id) in closures {
                                let use_info = def_use_analysis.local_info(local);
                                for u in &use_info.defs_and_uses {
                                    if is_terminator_location(&u.location, &body) {
                                        self.insert_direct(
                                            local_def_id,
                                            u.location.block,
                                            local_mono_def_id,
                                        );
                                        break;
                                        // TODO(Boqin): only consider one terminator that uses the closure for now.
                                        // This should have covered almost all the cases.
                                    }
                                }
                            }
                            for (bb, bb_data) in body.basic_blocks().iter_enumerated() {
                                if let TerminatorKind::Call { ref func, .. } =
                                    bb_data.terminator().kind
                                {
                                    if let Operand::Constant(box constant) = func {
                                        match constant.literal.ty.kind {
                                            TyKind::FnDef(callee_def_id, substs)
                                            | TyKind::Closure(callee_def_id, substs) => {
                                                if callee_def_id.is_local() {
                                                    let instance = Instance::resolve(
                                                        tcx,
                                                        ParamEnv::reveal_all(),
                                                        callee_def_id,
                                                        substs,
                                                    )
                                                    .unwrap()
                                                    .unwrap();
                                                    match instance.monomorphic_ty(tcx).kind {
                                                        TyKind::FnDef(mono_def_id, substs)
                                                        | TyKind::Closure(mono_def_id, substs) => {
                                                            if let Some(local_mono_def_id) =
                                                                mono_def_id.as_local()
                                                            {
                                                                // println!(
                                                                //     "\tcallee: {:?}",
                                                                //     mono_def_id
                                                                // );
                                                                self.insert_direct(
                                                                    local_def_id,
                                                                    bb,
                                                                    local_mono_def_id,
                                                                );
                                                            } else {
                                                                // println!(
                                                                //     "\tno-local: {:?}",
                                                                //     mono_def_id
                                                                // );
                                                            }
                                                        }
                                                        _ => {}
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn gen_mono(
        &mut self,
        crate_fn_ids: &[LocalDefId],
        tcx: TyCtxt,
    ) -> HashMap<LocalDefId, HashSet<LocalDefId>> {
        let mut mono_map: HashMap<LocalDefId, HashSet<LocalDefId>> = HashMap::new();
        for caller in crate_fn_ids {
            let caller = *caller;
            let body = tcx.optimized_mir(caller);

            let mut def_use_analysis = DefUseAnalysis::new(body);
            def_use_analysis.analyze(body);
            for (_local, local_decl) in body.local_decls.iter_enumerated() {
                match local_decl.ty.kind {
                    TyKind::Closure(callee_def_id, substs)
                    | TyKind::Generator(callee_def_id, substs, _) => {
                        if let Some(local_callee_def_id) = callee_def_id.as_local() {
                            if substs.has_param_types_or_consts() {
                                if crate_fn_ids.contains(&local_callee_def_id) {
                                    mono_map
                                        .entry(local_callee_def_id)
                                        .or_insert_with(HashSet::new)
                                        .insert(local_callee_def_id);
                                }
                                continue;
                            }
                            if let Ok(Some(instance)) = Instance::resolve(
                                tcx,
                                ParamEnv::reveal_all(),
                                callee_def_id,
                                substs,
                            ) {
                                let ty_kind = {
                                    let ty = tcx.type_of(instance.def.def_id());
                                    if !instance.substs.has_param_types_or_consts() {
                                        Some(
                                            &tcx.subst_and_normalize_erasing_regions(
                                                instance.substs,
                                                ParamEnv::reveal_all(),
                                                &ty,
                                            )
                                            .kind,
                                        )
                                    } else {
                                        None
                                    }
                                };
                                match ty_kind {
                                    Some(TyKind::FnDef(mono_def_id, _))
                                    | Some(TyKind::Closure(mono_def_id, _)) => {
                                        // println!("\tgeneric: {:?}", local_callee_def_id);
                                        if let Some(local_mono_def_id) = mono_def_id.as_local() {
                                            // println!("\tmono: {:?}", mono_def_id);
                                            if !crate_fn_ids.contains(&local_mono_def_id) {
                                                continue;
                                            }
                                            mono_map
                                                .entry(local_callee_def_id)
                                                .or_insert_with(HashSet::new)
                                                .insert(local_mono_def_id);
                                        } else {
                                            // println!("\tno-local: {:?}", mono_def_id);
                                        }
                                    }
                                    _ => {}
                                };
                            }
                        }
                    }
                    _ => {}
                }
            }

            for (_bb, bb_data) in body.basic_blocks().iter_enumerated() {
                let terminator = bb_data.terminator();
                if let TerminatorKind::Call { ref func, .. } = terminator.kind {
                    if let Operand::Constant(box constant) = func {
                        match constant.literal.ty.kind {
                            TyKind::FnDef(callee_def_id, substs)
                            | TyKind::Closure(callee_def_id, substs) => {
                                if let Some(local_callee_def_id) = callee_def_id.as_local() {
                                    if substs.has_param_types_or_consts() {
                                        if crate_fn_ids.contains(&local_callee_def_id) {
                                            mono_map
                                                .entry(local_callee_def_id)
                                                .or_insert_with(HashSet::new)
                                                .insert(local_callee_def_id);
                                        }
                                        continue;
                                    }
                                    if let Ok(Some(instance)) = Instance::resolve(
                                        tcx,
                                        ParamEnv::reveal_all(),
                                        callee_def_id,
                                        substs,
                                    ) {
                                        let ty_kind = {
                                            let ty = tcx.type_of(instance.def.def_id());
                                            if !instance.substs.has_param_types_or_consts() {
                                                Some(
                                                    &tcx.subst_and_normalize_erasing_regions(
                                                        instance.substs,
                                                        ParamEnv::reveal_all(),
                                                        &ty,
                                                    )
                                                    .kind,
                                                )
                                            } else {
                                                None
                                            }
                                        };
                                        match ty_kind {
                                            Some(TyKind::FnDef(mono_def_id, _))
                                            | Some(TyKind::Closure(mono_def_id, _)) => {
                                                // println!("\tgeneric: {:?}", local_callee_def_id);
                                                if let Some(local_mono_def_id) =
                                                    mono_def_id.as_local()
                                                {
                                                    if !crate_fn_ids.contains(&local_mono_def_id) {
                                                        continue;
                                                    }
                                                    // println!("\tmono: {:?}", mono_def_id);
                                                    mono_map
                                                        .entry(local_callee_def_id)
                                                        .or_insert_with(HashSet::new)
                                                        .insert(local_mono_def_id);
                                                } else {
                                                    // println!("\tno-local: {:?}", mono_def_id);
                                                }
                                            }
                                            _ => {}
                                        };
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        mono_map
    }
    pub fn generate(
        &mut self,
        caller: LocalDefId,
        crate_fn_ids: &[LocalDefId],
        mono_map: &HashMap<LocalDefId, HashSet<LocalDefId>>,
        tcx: TyCtxt,
    ) {
        let body = tcx.optimized_mir(caller);
        // println!("caller:{:?}", caller);
        let mut def_use_analysis = DefUseAnalysis::new(body);
        def_use_analysis.analyze(body);
        for (local, local_decl) in body.local_decls.iter_enumerated() {
            match local_decl.ty.kind {
                TyKind::Closure(callee_def_id, _) | TyKind::Generator(callee_def_id, _, _) => {
                    if let Some(local_callee_def_id) = callee_def_id.as_local() {
                        if let Some(mono_callee_def_ids) = mono_map.get(&local_callee_def_id) {
                            let use_info = def_use_analysis.local_info(local);
                            let mut bb = None;
                            for u in &use_info.defs_and_uses {
                                if is_terminator_location(&u.location, &body) {
                                    bb = Some(u.location.block);
                                    break;
                                    // TODO(Boqin): only consider one terminator that uses the closure for now.
                                    // This should have covered almost all the cases.
                                }
                            }
                            if let Some(bb) = bb {
                                for mono_callee_def_id in mono_callee_def_ids {
                                    // TODO(Boqin): the closure has a local that is equal to itself,
                                    // which should be avoided to prevent false recursive calling.
                                    if caller != *mono_callee_def_id {
                                        // println!("\txx_mono: {:?}", mono_callee_def_id);
                                        self.insert_direct(caller, bb, *mono_callee_def_id);
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        for (bb, bb_data) in body.basic_blocks().iter_enumerated() {
            let terminator = bb_data.terminator();
            if let TerminatorKind::Call { ref func, .. } = terminator.kind {
                if let Operand::Constant(box constant) = func {
                    match constant.literal.ty.kind {
                        TyKind::FnDef(callee_def_id, substs)
                        | TyKind::Closure(callee_def_id, substs) => {
                            if let Some(local_callee_def_id) = callee_def_id.as_local() {
                                // println!("callee: {:?},{:?}", callee_def_id, substs);
                                if substs.has_param_types_or_consts() {
                                    if let Some(mono_callee_def_ids) =
                                        mono_map.get(&local_callee_def_id)
                                    {
                                        for mono_callee_def_id in mono_callee_def_ids {
                                            // println!("\tx_mono: {:?}", mono_callee_def_id);
                                            self.insert_direct(caller, bb, *mono_callee_def_id);
                                        }
                                    }
                                    continue;
                                }

                                if crate_fn_ids.contains(&local_callee_def_id) {
                                    if let Ok(Some(instance)) = Instance::resolve(
                                        tcx,
                                        ParamEnv::reveal_all(),
                                        callee_def_id,
                                        substs,
                                    ) {
                                        let ty_kind = {
                                            let ty = tcx.type_of(instance.def.def_id());
                                            if !instance.substs.has_param_types_or_consts() {
                                                Some(
                                                    &tcx.subst_and_normalize_erasing_regions(
                                                        instance.substs,
                                                        ParamEnv::reveal_all(),
                                                        &ty,
                                                    )
                                                    .kind,
                                                )
                                            } else {
                                                None
                                            }
                                        };
                                        match ty_kind {
                                            Some(TyKind::FnDef(mono_def_id, _substs))
                                            | Some(TyKind::Closure(mono_def_id, _substs)) => {
                                                if let Some(local_mono_def_id) =
                                                    mono_def_id.as_local()
                                                {
                                                    if crate_fn_ids.contains(&local_mono_def_id) {
                                                        // println!("mono: {:?}",  mono_def_id);
                                                        self.insert_direct(
                                                            caller,
                                                            bb,
                                                            local_mono_def_id,
                                                        );
                                                    }
                                                } else {
                                                    // println!("\tno-local: {:?}", mono_def_id);
                                                }
                                            }
                                            _ => {}
                                        };
                                    }
                                } else {
                                    // println!("\tnot-local-fn: {:?}", local_callee_def_id);
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

fn is_terminator_location(location: &Location, body: &Body) -> bool {
    location.statement_index >= body.basic_blocks()[location.block].statements.len()
}
