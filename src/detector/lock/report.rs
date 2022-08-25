//! Reports for different kinds of bugs.
//! ReportContent includes bug kind, possibility, diagnosis, and explanation.
//! The diagnosis for different kinds of bugs may be different.
//! e.g., doublelock diagnosis contains one deadlock diagnosis,
//！while conflictlock diagnosis contanis a vector of deadlock diagnosis.
//! Deadlock diagnosis consists of the first & second locks' type and span (a.k.a. src code location),
//! and **all** possible callchains from first to second lock.
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct DeadlockDiagnosis {
    pub first_lock_type: String,
    pub first_lock_span: String,
    pub second_lock_type: String,
    pub second_lock_span: String,
    pub callchains: Vec<Vec<Vec<String>>>,
}

impl DeadlockDiagnosis {
    pub fn new(
        first_lock_type: String,
        first_lock_span: String,
        second_lock_type: String,
        second_lock_span: String,
        callchains: Vec<Vec<Vec<String>>>,
    ) -> Self {
        Self {
            first_lock_type,
            first_lock_span,
            second_lock_type,
            second_lock_span,
            callchains,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct WaitNotifyLocks {
    pub wait_lock_type: String,
    pub wait_lock_span: String,
    pub notify_lock_type: String,
    pub notify_lock_span: String,
}

impl WaitNotifyLocks {
    pub fn new(
        wait_lock_type: String,
        wait_lock_span: String,
        notify_lock_type: String,
        notify_lock_span: String,
    ) -> Self {
        Self {
            wait_lock_type,
            wait_lock_span,
            notify_lock_type,
            notify_lock_span,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CondvarDeadlockDiagnosis {
    pub condvar_wait_type: String,
    pub condvar_wait_callsite_span: String,
    pub condvar_notify_type: String,
    pub condvar_notify_callsite_span: String,
    pub deadlocks: Vec<WaitNotifyLocks>,
}

impl CondvarDeadlockDiagnosis {
    pub fn new(
        condvar_wait_type: String,
        condvar_wait_callsite_span: String,
        condvar_notify_type: String,
        condvar_notify_callsite_span: String,
        deadlocks: Vec<WaitNotifyLocks>,
    ) -> Self {
        Self {
            condvar_wait_type,
            condvar_wait_callsite_span,
            condvar_notify_type,
            condvar_notify_callsite_span,
            deadlocks,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct ReportContent<D> {
    pub bug_kind: String,
    pub possibility: String,
    pub diagnosis: D,
    pub explanation: String,
}

impl<D: std::fmt::Debug> ReportContent<D> {
    pub fn new(bug_kind: String, possibility: String, diagnosis: D, explanation: String) -> Self {
        Self {
            bug_kind,
            possibility,
            diagnosis,
            explanation,
        }
    }
}

#[derive(Debug, Serialize)]
pub enum Report {
    DoubleLock(ReportContent<DeadlockDiagnosis>),
    ConflictLock(ReportContent<Vec<DeadlockDiagnosis>>),
    CondvarDeadlock(ReportContent<CondvarDeadlockDiagnosis>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deadlock_diagnosis() {
        let d = DeadlockDiagnosis::new(
            "ParkingLotRead(loader::ModuleCache)".to_owned(),
            "language/move-vm/runtime/src/loader.rs:510:13: 510:18 (#0)".to_owned(),
            "ParkingLotRead(loader::ModuleCache)".to_owned(),
            "language/move-vm/runtime/src/loader.rs:510:13: 510:18 (#0)".to_owned(),
            vec![vec![vec![
                "language/move-vm/runtime/src/loader.rs:518:13: 518:55 (#0)".to_owned(),
            ]]],
        );
        assert_eq!(
            format!("{:?}", d),
            r#"DeadlockDiagnosis { first_lock_type: "ParkingLotRead(loader::ModuleCache)", first_lock_span: "language/move-vm/runtime/src/loader.rs:510:13: 510:18 (#0)", second_lock_type: "ParkingLotRead(loader::ModuleCache)", second_lock_span: "language/move-vm/runtime/src/loader.rs:510:13: 510:18 (#0)", callchains: [[["language/move-vm/runtime/src/loader.rs:518:13: 518:55 (#0)"]]] }"#
        )
    }

    #[test]
    fn test_report_content() {
        let d = DeadlockDiagnosis::new(
            "ParkingLotRead(loader::ModuleCache)".to_owned(),
            "language/move-vm/runtime/src/loader.rs:510:13: 510:18 (#0)".to_owned(),
            "ParkingLotRead(loader::ModuleCache)".to_owned(),
            "language/move-vm/runtime/src/loader.rs:510:13: 510:18 (#0)".to_owned(),
            vec![vec![vec![
                "language/move-vm/runtime/src/loader.rs:518:13: 518:55 (#0)".to_owned(),
            ]]],
        );
        let report_content = ReportContent::new(
            "DoubleLock".to_owned(),
            "Possibly".to_owned(),
            format!("{:?}", d),
            "The first lock is not released when acquiring the second lock".to_owned(),
        );
        assert_eq!(
            format!("{:?}", report_content),
            r#"ReportContent { bug_kind: "DoubleLock", possibility: "Possibly", diagnosis: "DeadlockDiagnosis { first_lock_type: \"ParkingLotRead(loader::ModuleCache)\", first_lock_span: \"language/move-vm/runtime/src/loader.rs:510:13: 510:18 (#0)\", second_lock_type: \"ParkingLotRead(loader::ModuleCache)\", second_lock_span: \"language/move-vm/runtime/src/loader.rs:510:13: 510:18 (#0)\", callchains: [[[\"language/move-vm/runtime/src/loader.rs:518:13: 518:55 (#0)\"]]] }", explanation: "The first lock is not released when acquiring the second lock" }"#
        );
    }
}
