use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct BeautifiedCallInCriticalSection {
    pub callchains: Vec<String>,
    pub ty: String,
}

