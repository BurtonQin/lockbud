/**
 * This file contains json interfaces for diagnostics consumed by the LSP server
 */


use std::collections::HashSet;
use serde::{Serialize, Deserialize};



#[derive(Hash, Eq, PartialEq, Copy, Clone, Serialize, Deserialize, Debug)]
pub enum Suspicious {
    ChSend,
    ChRecv,
    CondVarWait,
    DoubleLock,
    ConflictLock
}

// filename, start line & col, end line & col
type RangeInFile = (String, u32, u32, u32, u32);


#[derive(Debug, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub struct SuspiciousCall {
    pub callchains: Vec<RangeInFile>,
    pub ty: Suspicious,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HighlightArea {
    pub triggers: Vec<RangeInFile>,
    // filename, start line & col, end line & col
    pub ranges: Vec<RangeInFile>
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub calls: HashSet<SuspiciousCall>,
    pub critical_sections: Vec<HighlightArea>
}