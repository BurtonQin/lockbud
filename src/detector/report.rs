//! Reports for different kinds of bugs.
//! ReportContent includes bug kind, possibility, diagnosis, and explanation.
//! The diagnosis for different kinds of bugs may be different.
//! e.g., doublelock diagnosis contains one deadlock diagnosis,
//！while conflictlock diagnosis contanis a vector of deadlock diagnosis.
//! Deadlock diagnosis consists of the first & second locks' type and span (a.k.a. src code location),
//! and **all** possible callchains from first to second lock.
use serde::Serialize;

use crate::detector::atomic::report::AtomicityViolationDiagnosis;
use crate::detector::lock::report::{CondvarDeadlockDiagnosis, DeadlockDiagnosis};

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
    AtomicityViolation(ReportContent<AtomicityViolationDiagnosis>),
    InvalidFree(ReportContent<String>),
}
