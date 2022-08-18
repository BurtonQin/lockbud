//! Denotes Condvar APIs in std and parking_lot.
//!
//! 1. std::Condvar::wait.*(&Condvar, MutexGuard,.*) -> MutexGuard
//! 2. std::Condvar::notify.*(&Condvar)
//! 3. parking_lot::Condvar::wait.*(&Condvar, &mut MutexGuard,.*)
//! 4. parking_lot::Condvar::notify.*(&Condvar)
use rustc_middle::ty::{Instance, TyCtxt};

#[derive(Clone, Copy, Debug)]
pub enum CondvarApi {
    Std(StdCondvarApi),
    ParkingLot(ParkingLotCondvarApi),
}

impl CondvarApi {
    pub fn from_instance<'tcx>(instance: &Instance<'tcx>, tcx: TyCtxt<'tcx>) -> Option<Self> {
        let path = tcx.def_path_str_with_substs(instance.def_id(), instance.substs);
        let std_condvar = "std::sync::Condvar::";
        let parking_lot_condvar = "parking_lot::Condvar::";
        if path.starts_with(std_condvar) {
            let tail = &path.as_bytes()[std_condvar.len()..];
            let std_condvar_api = if tail.starts_with("wait::".as_bytes()) {
                StdCondvarApi::Wait(StdWait::Wait)
            } else if tail.starts_with("wait_timeout::".as_bytes()) {
                StdCondvarApi::Wait(StdWait::WaitTimeout)
            } else if tail.starts_with("wait_timeout_ms::".as_bytes()) {
                StdCondvarApi::Wait(StdWait::WaitTimeoutMs)
            } else if tail.starts_with("wait_timeout_while::".as_bytes()) {
                StdCondvarApi::Wait(StdWait::WaitTimeoutWhile)
            } else if tail.starts_with("wait_while::".as_bytes()) {
                StdCondvarApi::Wait(StdWait::WaitWhile)
            } else if tail == "notify_all".as_bytes() {
                StdCondvarApi::Notify(StdNotify::NotifyAll)
            } else if tail == "notify_one".as_bytes() {
                StdCondvarApi::Notify(StdNotify::NotifyOne)
            } else {
                return None;
            };
            Some(CondvarApi::Std(std_condvar_api))
        } else if path.starts_with(parking_lot_condvar) {
            let tail = &path.as_bytes()[parking_lot_condvar.len()..];
            let parking_lot_condvar_api = if tail.starts_with("wait::".as_bytes()) {
                ParkingLotCondvarApi::Wait(ParkingLotWait::Wait)
            } else if tail.starts_with("wait_for::".as_bytes()) {
                ParkingLotCondvarApi::Wait(ParkingLotWait::WaitFor)
            } else if tail.starts_with("wait_until::".as_bytes()) {
                ParkingLotCondvarApi::Wait(ParkingLotWait::WaitUntil)
            } else if tail.starts_with("wait_while::".as_bytes()) {
                ParkingLotCondvarApi::Wait(ParkingLotWait::WaitWhile)
            } else if tail.starts_with("wait_while_for::".as_bytes()) {
                ParkingLotCondvarApi::Wait(ParkingLotWait::WaitWhileFor)
            } else if tail.starts_with("wait_while_until::".as_bytes()) {
                ParkingLotCondvarApi::Wait(ParkingLotWait::WaitWhileUntil)
            } else if tail == "notify_all".as_bytes() {
                ParkingLotCondvarApi::Notify(ParkingLotNotify::NotifyAll)
            } else if tail == "notify_one".as_bytes() {
                ParkingLotCondvarApi::Notify(ParkingLotNotify::NotifyOne)
            } else {
                return None;
            };
            Some(CondvarApi::ParkingLot(parking_lot_condvar_api))
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum StdCondvarApi {
    Wait(StdWait),
    Notify(StdNotify),
}

#[derive(Clone, Copy, Debug)]
pub enum StdWait {
    Wait,
    WaitTimeout,
    WaitTimeoutMs,
    WaitTimeoutWhile,
    WaitWhile,
}

#[derive(Clone, Copy, Debug)]
pub enum StdNotify {
    NotifyAll,
    NotifyOne,
}

#[derive(Clone, Copy, Debug)]
pub enum ParkingLotCondvarApi {
    Wait(ParkingLotWait),
    Notify(ParkingLotNotify),
}

#[derive(Clone, Copy, Debug)]
pub enum ParkingLotWait {
    Wait,
    WaitFor,
    WaitUntil,
    WaitWhile,
    WaitWhileFor,
    WaitWhileUntil,
}

#[derive(Clone, Copy, Debug)]
pub enum ParkingLotNotify {
    NotifyAll,
    NotifyOne,
}
