extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_span;

use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_middle::mir::{BasicBlock, Local};
use rustc_middle::ty::Ty;
use rustc_span::Span;

use std::hash::Hash;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum LockGuardSrc {
    ParamSrc(ParamSrcContext),
    LocalSrc(LocalSrcContext),
    GlobalSrc(GlobalSrcContext),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ParamSrcContext {
    pub struct_type: String,
    pub fields: String,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct LocalSrcContext {
    pub place: String,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct GlobalSrcContext {
    pub global_id: DefId,
}

#[derive(Debug, Clone)]
pub struct LockGuardInfo {
    pub type_name: (LockGuardType, String),
    pub src: Option<LockGuardSrc>,
    pub span: Span,
    pub gen_bbs: Vec<BasicBlock>,
    pub kill_bbs: Vec<BasicBlock>,
}

impl PartialEq for LockGuardInfo {
    fn eq(&self, other: &Self) -> bool {
        self.type_name == other.type_name
            && if let Some(self_src) = &self.src {
                if let Some(other_src) = &other.src {
                    *self_src == *other_src
                } else {
                    false
                }
            } else {
                false
            }
    }
}
#[derive(Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub struct LockGuardId {
    pub fn_id: LocalDefId,
    pub local: Local,
}

impl LockGuardId {
    pub fn new(fn_id: LocalDefId, local: Local) -> Self {
        Self { fn_id, local }
    }
}
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum LockGuardType {
    StdMutexGuard,
    StdRwLockGuard,
    ParkingLotMutexGuard,
    ParkingLotRwLockGuard,
    SpinMutexGuard,
    SpinRwLockGuard,
}

pub fn parse_lockguard_type(ty: &Ty) -> Option<(LockGuardType, String)> {
    let type_name = ty.to_string();
    if type_name.starts_with("std::sync::MutexGuard<") {
        Some((
            LockGuardType::StdMutexGuard,
            extract_data_type("std::sync::MutexGuard<", &type_name),
        ))
    } else if type_name.starts_with("std::sync::RwLockReadGuard<") {
        Some((
            LockGuardType::StdRwLockGuard,
            extract_data_type("std::sync::RwLockReadGuard<", &type_name),
        ))
    } else if type_name.starts_with("std::sync::RwLockWriteGuard<") {
        Some((
            LockGuardType::StdRwLockGuard,
            extract_data_type("std::sync::RwLockWriteGuard<", &type_name),
        ))
    } else if type_name.starts_with("lock_api::mutex::MutexGuard<") {
        Some((
            LockGuardType::ParkingLotMutexGuard,
            extract_data_type("lock_api::mutex::MutexGuard<", &type_name),
        ))
    } else if type_name.starts_with("lock_api::rwlock::RwLockReadGuard<") {
        Some((
            LockGuardType::ParkingLotRwLockGuard,
            extract_data_type("lock_api::rwlock::RwLockReadGuard<", &type_name),
        ))
    } else if type_name.starts_with("lock_api::rwlock::RwLockWriteGuard<") {
        Some((
            LockGuardType::ParkingLotRwLockGuard,
            extract_data_type("lock_api::rwlock::RwLockWriteGuard<", &type_name),
        ))
    } else if type_name.starts_with("parking_lot::lock_api::MutexGuard<") {
        Some((
            LockGuardType::ParkingLotRwLockGuard,
            extract_data_type("parking_lot::lock_api::MutexGuard<", &type_name),
        ))
    } else if type_name.starts_with("parking_lot::lock_api::RwLockReadGuard<") {
        Some((
            LockGuardType::ParkingLotRwLockGuard,
            extract_data_type("parking_lot::lock_api::RwLockReadGuard<", &type_name),
        ))
    } else if type_name.starts_with("parking_lot::lock_api::RwLockWriteGuard<") {
        Some((
            LockGuardType::ParkingLotRwLockGuard,
            extract_data_type("parking_lot::lock_api::RwLockWriteGuard<", &type_name),
        ))
    } else if type_name.starts_with("spin::mutex::MutexGuard<") {
        Some((
            LockGuardType::SpinMutexGuard,
            extract_data_type("spin::mutex::MutexGuard<", &type_name),
        ))
    } else if type_name.starts_with("spin::rw_lock::RwLockReadGuard<") {
        Some((
            LockGuardType::SpinRwLockGuard,
            extract_data_type("spin::rw_lock::RwLockReadGuard<", &type_name),
        ))
    } else if type_name.starts_with("spin::rw_lock::RwLockWriteGuard<") {
        Some((
            LockGuardType::SpinRwLockGuard,
            extract_data_type("spin::rw_lock::RwLockWriteGuard<", &type_name),
        ))
    } else {
        None
    }
}

fn extract_data_type(lockguard_type: &str, type_name: &str) -> String {
    assert!(type_name.starts_with(lockguard_type) && type_name.ends_with('>'));
    type_name[lockguard_type.len()..type_name.len() - 1].to_string()
}

#[test]
fn test_extract_data_type() {
    assert!(
        extract_data_type(
            "std::sync::MutexGuard<",
            "std::sync::MutexGuard<std::vec::Vec<Foo>>"
        ) == "std::vec::Vec<Foo>"
    );
    assert!(
        extract_data_type(
            "lock_api::mutex::MutexGuard<",
            "lock_api::mutex::MutexGuard<parking_lot::raw_mutex::RawMutex, i32>"
        ) == "parking_lot::raw_mutex::RawMutex, i32"
    );
}
pub struct DoubleLockInfo {
    pub first: LockGuardId,
    pub second: LockGuardId,
}
