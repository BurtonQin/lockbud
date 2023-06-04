use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct AtomicityViolationDiagnosis {
    pub fn_name: String,
    pub atomic_reader: String,
    pub atomic_writer: String,
    pub dep_kind: String,
}
