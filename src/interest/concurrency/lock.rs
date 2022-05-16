// LockGuard
// std::sync::MutexGuard
// filter types
// LockGuard: id, type, gen, kill
// 1. filter locals whose type matches LockGuardTy
// 2. visit local for gen/kill stmt
// 3. apply gen/kill analysis on each fn, get relations between lockguards
// 4. visit callgraph and propogate lockguards inter-procedurally
// 5. if lockguard A has relation with B (A not released when B acquired)
// 6. then add edge(A, B) to directed graph with weight as its path
// 7. find locks that generate A and B
// 8. if A.lock -> A.lock then doublelock
// 9. if A.lock -> B.lock and B.lock -> A.lock then conflictlock
// 10. similarly, if A.lock -> B.lock, B.lock -> C.lock, C.lock -> A.lock then conflictlock
// 11. in a word, if there is a loop on the directed graph, then there is a possible deadlock
extern crate rustc_hash;
extern crate rustc_span;

use smallvec::SmallVec;

use rustc_hash::FxHashMap;
use rustc_middle::mir::visit::{
    MutatingUseContext, NonMutatingUseContext, PlaceContext, Visitor,
};
use rustc_middle::mir::{Body, Local, Location};
use rustc_middle::ty::{self, Instance, ParamEnv, TyCtxt};
use rustc_span::Span;

use crate::analysis::callgraph::InstanceId;

// LockGuardId = (InstanceIdx, Local)
// LockGuardInfo = LockGuardId -> (LockGuardTy, GenLocation, KillLoction)
// LockGuardGraph = { V: LockGuardId, E: CallChain) }
// CallChain = [InstanceIdx]
// DoubleLockCandidate ==
// \A A, B \in LockGuardId, A \X B \in LockGuardGraph
// /\ LockGuardInfo(A).LocKGuardTy = LockGuardInfo(B).LockGuardTy
// ConflictLockCandidate ==
// \A A, B, C, D \in LockGuardId, A \X B, C \X D \in LockGuardGraph
// /\ LockGuardInfo(A).LockGuardTy = LockGuardInfo(D).LockGuardTy
// /\ LockGuardInfo(B).LockGuardTy = LockGuardInfo(C).LockGuardTy
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

#[derive(Clone, Copy, Debug)]
pub enum DeadlockPossibility {
    Probably,
    Possibly,
    Unlikely,
    Unknown,
}

// LockGuardKind, DataTy
#[derive(Clone, Debug)]
pub enum LockGuardTy<'tcx> {
    StdMutex(ty::Ty<'tcx>),
    ParkingLotMutex(ty::Ty<'tcx>),
    SpinMutex(ty::Ty<'tcx>),
    StdRwLockRead(ty::Ty<'tcx>),
    StdRwLockWrite(ty::Ty<'tcx>),
    ParkingLotRead(ty::Ty<'tcx>),
    ParkingLotWrite(ty::Ty<'tcx>),
    SpinRead(ty::Ty<'tcx>),
    SpinWrite(ty::Ty<'tcx>),
}

impl<'tcx> LockGuardTy<'tcx> {
    pub fn from_local_ty(local_ty: ty::Ty<'tcx>, tcx: TyCtxt<'tcx>) -> Option<Self> {
        // e.g.
        // extract i32 from
        // sync: MutexGuard<i32, Poison>
        // spin: MutexGuard<i32>
        // parking_lot: MutexGuard<RawMutex, i32>
        match local_ty.kind() {
            ty::TyKind::Adt(adt_def, substs) => {
                let path = tcx.def_path_str_with_substs(adt_def.did(), substs);
                if path.starts_with("std::sync::MutexGuard<")
                    || path.starts_with("sync::mutex::MutexGuard<")
                {
                    return Some(LockGuardTy::StdMutex(substs.types().next().unwrap()));
                } else if path.starts_with("lock_api::mutex::MutexGuard<")
                    || path.starts_with("parking_lot::lock_api::MutexGuard<")
                {
                    return Some(LockGuardTy::ParkingLotMutex(substs.types().nth(1).unwrap()));
                } else if path.starts_with("spin::mutex::MutexGuard<") {
                    return Some(LockGuardTy::SpinMutex(substs.types().next().unwrap()));
                } else if path.starts_with("spin::MutexGuard<") {
                    return Some(LockGuardTy::SpinMutex(substs.types().next().unwrap()));
                } else if path.starts_with("std::sync::RwLockReadGuard<") {
                    return Some(LockGuardTy::StdRwLockRead(substs.types().next().unwrap()));
                } else if path.starts_with("std::sync::RwLockWriteGuard<") {
                    return Some(LockGuardTy::StdRwLockWrite(substs.types().next().unwrap()));
                } else if path.starts_with("lock_api::rwlock::RwLockReadGuard<")
                    || path.starts_with("parking_lot::lock_api::RwLockReadGuard<")
                {
                    return Some(LockGuardTy::ParkingLotRead(substs.types().nth(1).unwrap()));
                } else if path.starts_with("lock_api::rwlock::RwLockWriteGuard<")
                    || path.starts_with("parking_lot::lock_api::RwLockWriteGuard<")
                {
                    return Some(LockGuardTy::ParkingLotWrite(substs.types().nth(1).unwrap()));
                } else if path.starts_with("spin::rw_lock::RwLockReadGuard<") {
                    return Some(LockGuardTy::SpinRead(substs.types().next().unwrap()));
                } else if path.starts_with("spin::RwLockReadGuard<") {
                    return Some(LockGuardTy::SpinRead(substs.types().next().unwrap()));
                } else if path.starts_with("spin::rw_lock::RwLockWriteGuard<") {
                    return Some(LockGuardTy::SpinWrite(substs.types().next().unwrap()));
                } else if path.starts_with("spin::RwLockWriteGuard<") {
                    return Some(LockGuardTy::SpinWrite(substs.types().next().unwrap()));
                }
            }
            _ => {}
        };
        None
    }

    pub fn deadlock_with(&self, other: &Self) -> DeadlockPossibility {
        use LockGuardTy::*;
        // println!("deadlock_with: {:?}, {:?}", self, other);
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

#[derive(Clone, Debug)]
pub struct LockGuardInfo<'tcx> {
    pub lockguard_ty: LockGuardTy<'tcx>,
    pub span: Span,
    pub gen_locs: SmallVec<[Location; 4]>,
    pub kill_locs: SmallVec<[Location; 4]>,
}

impl<'tcx> LockGuardInfo<'tcx> {
    pub fn new(lockguard_ty: LockGuardTy<'tcx>, span: Span) -> Self {
        Self {
            lockguard_ty,
            span,
            gen_locs: Default::default(),
            kill_locs: Default::default(),
        }
    }
}

// filter CallGraph has_lockguard
// for all instances
// 	if instance contains lockguard
//    mark the instance on callgraph
//    wcc
//    dfs(visit)
// for each wcc of Callgraph
//   dfs_visit(wcc.root)
//   if inst not contains lockguard, then push it to edge
//   if contains, then make it a node
// fn filter_callgraph(callgraph: CallGraph) {

// }

pub type LockGuardMap<'tcx> = FxHashMap<LockGuardId, LockGuardInfo<'tcx>>;

pub struct LockGuardCollector<'a, 'b, 'tcx> {
    instance_id: InstanceId,
    instance: &'a Instance<'tcx>,
    body: &'b Body<'tcx>,
    tcx: TyCtxt<'tcx>,
    param_env: ParamEnv<'tcx>,
    pub lockguards: LockGuardMap<'tcx>,
}

impl<'a, 'b, 'tcx> LockGuardCollector<'a, 'b, 'tcx> {
    pub fn new(
        instance_id: InstanceId,
        instance: &'a Instance<'tcx>,
        body: &'b Body<'tcx>,
        tcx: TyCtxt<'tcx>,
        param_env: ParamEnv<'tcx>,
    ) -> Self {
        Self {
            instance_id,
            instance,
            body,
            tcx,
            param_env,
            lockguards: Default::default(),
        }
    }

    pub fn analyze(&mut self) {
        for (local, local_decl) in self.body.local_decls.iter_enumerated() {
            let local_ty = self.instance.subst_mir_and_normalize_erasing_regions(
                self.tcx,
                self.param_env,
                local_decl.ty,
            );
            if let Some(lockguard_ty) = LockGuardTy::from_local_ty(local_ty, self.tcx) {
                let lockguard_id = LockGuardId::new(self.instance_id, local);
                let lockguard_info = LockGuardInfo::new(lockguard_ty, local_decl.source_info.span);
                self.lockguards.insert(lockguard_id, lockguard_info);
            }
        }
        self.visit_body(self.body);
    }
}

impl<'a, 'b, 'tcx> Visitor<'tcx> for LockGuardCollector<'a, 'b, 'tcx> {
    fn visit_local(&mut self, local: &Local, context: PlaceContext, location: Location) {
        let lockguard_id = LockGuardId::new(self.instance_id, *local);
        // local is lockguard
        if let Some(info) = self.lockguards.get_mut(&lockguard_id) {
            match context {
                // PlaceContext::NonUse(context) => match context {
                //     NonUseContext::StorageLive => info.gen_locs.push(location),
                //     NonUseContext::StorageDead => info.kill_locs.push(location),
                //     _ => {}
                // },
                PlaceContext::NonMutatingUse(context) => {
                    if let NonMutatingUseContext::Move = context {
                        info.kill_locs.push(location);
                    }
                }
                PlaceContext::MutatingUse(context) => match context {
                    MutatingUseContext::Drop => info.kill_locs.push(location),
                    MutatingUseContext::Store => info.gen_locs.push(location),
                    MutatingUseContext::Call => info.gen_locs.push(location),
                    _ => {}
                },
                _ => {}
            }
        }
    }
}

// lockguard instances = filter instances if instance contains lockguard
// traverse callgraph to find caller-callee relations between lockguard instances, record the reachable callchain
// Graph = (V: lockguard_instance, E: callchain)
// dfs_visit wcc of Graph starting from root instance
// dfs_visit BBs of instance
// fixedpoint:
// 	before[BB] = after[preds(BB)]
// 	after[BB] = before[BB] \ kill[BB] U gen[BB]
// output: relations = LockGuardId X LockGuardId, info = LockGuardId X LockGuardInfo
// get candidate deadlock pairs purely based on data type of Lock
// deadlocks = lockguards pair that (may) deadlock
// if (A, B) in both relations and deadlocks then report
// if (A, B), (C, D) in relations and (A, D), (B, C) in deadlocks then report
// if (A, B), (C, D), (E, F) in relations and (A, ), (B, C),
