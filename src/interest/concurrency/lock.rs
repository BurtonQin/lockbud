//! Collect LockGuard info.
// extern crate rustc_span;

use smallvec::SmallVec;
use std::cmp::Ordering;

use rustc_hash::FxHashMap;
// use rustc_middle::mir::visit::{MutatingUseContext, NonMutatingUseContext, PlaceContext, Visitor};
// use rustc_middle::mir::{Body, Local, Location, TerminatorKind};
// use rustc_middle::ty::EarlyBinder;
// use rustc_middle::ty::{self, Instance, TypingEnv, TyCtxt};
// use rustc_span::Span;
use stable_mir::{CrateDef, mir::{mono::Instance, visit::Location, Body, Local, MirVisitor, Operand, Statement, StatementKind, Terminator, TerminatorKind, RETURN_LOCAL}, ty::{self, RigidTy, Span, TyKind}};
use crate::analysis::callgraph::InstanceId;

/// Uniquely identify a LockGuard in a crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LockGuardId {
    pub instance_id: InstanceId,
    pub local: Local,
}

impl LockGuardId {
    pub fn new(instance_id: InstanceId, local: Local) -> Self {
        Self { instance_id, local }
    }
}

/// The possibility of deadlock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeadlockPossibility {
    Probably,
    Possibly,
    Unlikely,
    Unknown,
}

impl PartialOrd for DeadlockPossibility {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        use DeadlockPossibility::*;
        match (*self, *other) {
            (Probably, Probably)
            | (Possibly, Possibly)
            | (Unlikely, Unlikely)
            | (Unknown, Unknown) => Some(Ordering::Equal),
            (Probably, _) | (Possibly, Unlikely) | (Possibly, Unknown) | (Unlikely, Unknown) => {
                Some(Ordering::Greater)
            }
            (_, Probably) | (Unlikely, Possibly) | (Unknown, Possibly) | (Unknown, Unlikely) => {
                Some(Ordering::Less)
            }
        }
    }
}

/// LockGuardKind, DataTy
#[derive(Clone, Debug)]
pub enum LockGuardTy {
    StdMutex(ty::Ty),
    ParkingLotMutex(ty::Ty),
    SpinMutex(ty::Ty),
    StdRwLockRead(ty::Ty),
    StdRwLockWrite(ty::Ty),
    ParkingLotRead(ty::Ty),
    ParkingLotWrite(ty::Ty),
    SpinRead(ty::Ty),
    SpinWrite(ty::Ty),
}

impl LockGuardTy {
    pub fn from_local_ty(local_ty: ty::Ty) -> Option<Self> {
        // e.g.
        // extract i32 from
        // sync: MutexGuard<i32, Poison>
        // spin: MutexGuard<i32>
        // parking_lot: MutexGuard<RawMutex, i32>
        // async, tokio, future: currently Unsupported
        if let TyKind::RigidTy(RigidTy::Adt(adt_def, substs)) = local_ty.kind() {
            // let path = tcx.def_path_str_with_args(adt_def.did(), substs);
            let adt_ty = adt_def.ty_with_args(&substs);
            let path = format!("{}", adt_ty);
            println!("{path}");
            // quick fail
            if !path.contains("MutexGuard")
                && !path.contains("RwLockReadGuard")
                && !path.contains("RwLockWriteGuard")
            {
                return None;
            }
            let first_part = path.split('<').next()?;
            if first_part.contains("MutexGuard") {
                if first_part.contains("async")
                    || first_part.contains("tokio")
                    || first_part.contains("future")
                    || first_part.contains("loom")
                {
                    // Currentlly does not support async lock or loom
                    None
                } else if first_part.contains("spin") {
                    // std::sync::MutexGuard<'_, bool>
                    Some(LockGuardTy::SpinMutex(*substs.0.get(1)?.ty()?))
                } else if first_part.contains("lock_api") || first_part.contains("parking_lot") {
                    Some(LockGuardTy::ParkingLotMutex(*substs.0.get(1)?.ty()?))
                } else {
                    // std::sync::Mutex or its wrapper by default
                    Some(LockGuardTy::StdMutex(*substs.0.get(1)?.ty()?))
                }
            } else if first_part.contains("RwLockReadGuard") {
                if first_part.contains("async")
                    || first_part.contains("tokio")
                    || first_part.contains("future")
                    || first_part.contains("loom")
                {
                    // Currentlly does not support async lock or loom
                    None
                } else if first_part.contains("spin") {
                    Some(LockGuardTy::SpinRead(*substs.0.get(0)?.ty()?))
                } else if first_part.contains("lock_api") || first_part.contains("parking_lot") {
                    Some(LockGuardTy::ParkingLotRead(*substs.0.get(1)?.ty()?))
                } else {
                    // std::sync::RwLockReadGuard or its wrapper by default
                    Some(LockGuardTy::StdRwLockRead(*substs.0.get(0)?.ty()?))
                }
            } else if first_part.contains("RwLockWriteGuard") {
                if first_part.contains("async")
                    || first_part.contains("tokio")
                    || first_part.contains("future")
                    || first_part.contains("loom")
                {
                    // Currentlly does not support async lock or loom
                    None
                } else if first_part.contains("spin") {
                    Some(LockGuardTy::SpinWrite(*substs.0.get(0)?.ty()?))
                } else if first_part.contains("lock_api") || first_part.contains("parking_lot") {
                    Some(LockGuardTy::ParkingLotWrite(*substs.0.get(1)?.ty()?))
                } else {
                    // std::sync::RwLockReadGuard or its wrapper by default
                    Some(LockGuardTy::StdRwLockWrite(*substs.0.get(0)?.ty()?))
                }
            } else {
                None
            }
        } else {
            None
        }
    }

    /// In parking_lot, the read lock is by default non-recursive if not specified.
    /// if two recursively acquired read locks in one thread are interleaved
    /// by a write lock from another thread, a deadlock may happen.
    /// The reason is write lock has higher priority than read lock in parking_lot.
    /// In std::sync, the implementation of read lock depends on the underlying OS.
    /// AFAIK, the implementation on Windows and Mac have write priority.
    /// So read lock in std::sync cannot be acquired recursively on the two systems.
    /// spin explicitly documents no write priority. So the read lock in spin can
    /// be acquired recursively.
    pub fn deadlock_with(&self, other: &Self) -> DeadlockPossibility {
        use LockGuardTy::*;
        match (self, other) {
            (StdMutex(a), StdMutex(b))
            | (ParkingLotMutex(a), ParkingLotMutex(b))
            | (SpinMutex(a), SpinMutex(b))
            | (StdRwLockWrite(a), StdRwLockWrite(b))
            | (StdRwLockWrite(a), StdRwLockRead(b))
            | (StdRwLockRead(a), StdRwLockWrite(b))
            | (ParkingLotWrite(a), ParkingLotWrite(b))
            | (ParkingLotWrite(a), ParkingLotRead(b))
            | (ParkingLotRead(a), ParkingLotWrite(b))
            | (SpinWrite(a), SpinWrite(b))
            | (SpinWrite(a), SpinRead(b))
            | (SpinRead(a), SpinWrite(b))
                if a == b =>
            {
                DeadlockPossibility::Probably
            }
            (StdRwLockRead(a), StdRwLockRead(b)) | (ParkingLotRead(a), ParkingLotRead(b))
                if a == b =>
            {
                DeadlockPossibility::Possibly
            }
            _ => DeadlockPossibility::Unlikely,
        }
    }
}

/// The lockguard info. `span` is for report.
#[derive(Clone, Debug)]
pub struct LockGuardInfo {
    pub lockguard_ty: LockGuardTy,
    pub span: Span,
    pub gen_locs: SmallVec<[Location; 4]>,
    pub move_gen_locs: SmallVec<[Location; 4]>,
    pub recursive_gen_locs: SmallVec<[Location; 4]>,
    pub kill_locs: SmallVec<[Location; 4]>,
}

impl LockGuardInfo {
    pub fn new(lockguard_ty: LockGuardTy, span: Span) -> Self {
        Self {
            lockguard_ty,
            span,
            gen_locs: Default::default(),
            move_gen_locs: Default::default(),
            recursive_gen_locs: Default::default(),
            kill_locs: Default::default(),
        }
    }

    pub fn is_gen_only_by_move(&self) -> bool {
        self.gen_locs == self.move_gen_locs
    }

    pub fn is_gen_only_by_recursive(&self) -> bool {
        self.gen_locs == self.recursive_gen_locs
    }
}

pub type LockGuardMap = FxHashMap<LockGuardId, LockGuardInfo>;

/// Collect lockguard info.
pub struct LockGuardCollector<'a, 'b> {
    instance_id: InstanceId,
    instance: &'a Instance,
    body: &'b Body,
    pub lockguards: LockGuardMap,
}

impl<'a, 'b> LockGuardCollector<'a, 'b> {
    pub fn new(
        instance_id: InstanceId,
        instance: &'a Instance,
        body: &'b Body,
    ) -> Self {
        Self {
            instance_id,
            instance,
            body,
            lockguards: Default::default(),
        }
    }

    pub fn analyze(&mut self) {
        for (local, local_decl) in self.body.local_decls() {
            // let local_ty = self.instance.instantiate_mir_and_normalize_erasing_regions(
            //     self.tcx,
            //     self.typing_env,
            //     EarlyBinder::bind(local_decl.ty),
            // );
            let local_ty = local_decl.ty;
            if let Some(lockguard_ty) = LockGuardTy::from_local_ty(local_ty) {
                let lockguard_id = LockGuardId::new(self.instance_id, local);
                let lockguard_info = LockGuardInfo::new(lockguard_ty, local_decl.span);
                self.lockguards.insert(lockguard_id, lockguard_info);
            }
        }
        self.visit_body(self.body);
    }
}

impl MirVisitor for LockGuardCollector<'_, '_> {
    // https://github.com/rust-lang/rust/blob/6d3db555e614eb50bbb40559e696414e69b6eff9/compiler/rustc_middle/src/mir/visit.rs#L596
    fn visit_terminator(&mut self, term: &Terminator, location: Location) {
        // terminator, RETURN_LOCAL => NonMutatingUseContext::Move
        let lockguard_id = LockGuardId::new(self.instance_id, RETURN_LOCAL);
        if let Some(info) = self.lockguards.get_mut(&lockguard_id) {
            info.kill_locs.push(location);
        }
        // Drop(place, ..) => MuatatinguseContext::Drop
        match term.kind {
            TerminatorKind::Drop { ref place, .. } => {
                let lockguard_id = LockGuardId::new(self.instance_id, place.local);
                if let Some(info) = self.lockguards.get_mut(&lockguard_id) {
                    info.kill_locs.push(location);
                }
            }
            TerminatorKind::Call { ref func, args: _, ref destination, target: _, unwind: _ } => {
                let lockguard_id = LockGuardId::new(self.instance_id, destination.local);
                if let Some(info) = self.lockguards.get_mut(&lockguard_id) {
                    // if lockguard = parking_lot::recursive_read() then record to recursive_gen_locs
                    if let LockGuardTy::ParkingLotRead(_) = info.lockguard_ty {
                        let func_ty = func.ty(self.body.locals()).unwrap();
                        if let TyKind::RigidTy(RigidTy::FnDef(fn_def, _)) = func_ty.kind() {
                            if fn_def.name().contains("read_recursive") {
                                info.recursive_gen_locs.push(location);
                            }
                        }
                    }
                    info.gen_locs.push(location);
                }
            }
            _ => {}
        }


    }
    fn visit_operand(&mut self, operand: &Operand, location: Location) {
        // operand, Move(place) => NonMutatingUseContext::Move
        if let Operand::Move(place) = operand {
            let lockguard_id = LockGuardId::new(self.instance_id, place.local);
            if let Some(info) = self.lockguards.get_mut(&lockguard_id) {
                info.kill_locs.push(location);
            }
        }
        
    }
    fn visit_statement(&mut self, stmt: &Statement, location: Location) {
        // // operand, Store(place) => MutatingUseContext::Store
        match stmt.kind {
            StatementKind::Assign(ref place, _) => {
                let lockguard_id = LockGuardId::new(self.instance_id, place.local);
                if let Some(info) = self.lockguards.get_mut(&lockguard_id) {
                    info.gen_locs.push(location);
                    info.move_gen_locs.push(location);
                }
            }
            _ => {}
        }
    }    

    // visit_local is not supported in stable_mir for now.
    // The above implementation is as-is the following non-stable mir code

    // fn visit_local(&mut self, local: &Local, context: PlaceContext, location: Location) {
    //     let lockguard_id = LockGuardId::new(self.instance_id, *local);
    //     // local is lockguard
    //     if let Some(info) = self.lockguards.get_mut(&lockguard_id) {
    //         match context {
    //             PlaceContext::NonMutatingUse(NonMutatingUseContext::Move) => {
    //                 info.kill_locs.push(location);
    //             }
    //             PlaceContext::MutatingUse(context) => match context {
    //                 MutatingUseContext::Drop => info.kill_locs.push(location),
    //                 MutatingUseContext::Store => {
    //                     info.gen_locs.push(location);
    //                     info.move_gen_locs.push(location);
    //                 }
    //                 MutatingUseContext::Call => {
    //                     // if lockguard = parking_lot::recursive_read() then record to recursive_gen_locs
    //                     if let LockGuardTy::ParkingLotRead(_) = info.lockguard_ty {
    //                         let term = self.body[location.block].terminator();
    //                         if let TerminatorKind::Call { ref func, .. } = term.kind {
    //                             let func_ty = func.ty(self.body, self.tcx);
    //                             // Only after monomorphizing can Instance::try_resolve work
    //                             let func_ty =
    //                                 self.instance.instantiate_mir_and_normalize_erasing_regions(
    //                                     self.tcx,
    //                                     self.typing_env,
    //                                     EarlyBinder::bind(func_ty),
    //                                 );
    //                             if let ty::FnDef(def_id, _) = *func_ty.kind() {
    //                                 let fn_name = self.tcx.def_path_str(def_id);
    //                                 if fn_name.contains("read_recursive") {
    //                                     info.recursive_gen_locs.push(location);
    //                                 }
    //                             }
    //                         }
    //                     }
    //                     info.gen_locs.push(location);
    //                 }
    //                 _ => {}
    //             },
    //             _ => {}
    //         }
    //     }
    // }
}
