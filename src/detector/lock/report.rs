#[derive(Debug)]
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

#[allow(dead_code)]
#[derive(Debug)]
pub struct ReportContent<D> {
    bug_kind: String,
    possibility: String,
    diagnosis: D,
    explanation: String,
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

#[derive(Debug)]
pub enum Report {
    DoubleLock(ReportContent<DeadlockDiagnosis>),
    ConflictLock(ReportContent<Vec<DeadlockDiagnosis>>),
}
