extern crate rustc_span;
use std::collections::{HashMap, HashSet};
use super::lock::{LockGuardType, LockGuardSrc, LockGuardInfo};
use rustc_span::Span;
#[derive(PartialEq, Eq, Hash, Debug)]
struct DoubleLockPair {
    first_lock_type_name: (LockGuardType, String),
    first_lock_span: Span,
    second_lock_type_name: (LockGuardType, String),
    second_lock_span: Span,
}
// LockGuardSrc, DoubleLockPair, Callchains
pub struct DoubleLockReports {
    reports: HashMap<LockGuardSrc, HashMap<DoubleLockPair, HashSet<Vec<Span>>>>,
}

impl DoubleLockReports {
    pub fn new() -> Self {
        Self {
            reports: HashMap::new(),
        }
    }

    pub fn add(&mut self, pair: (&LockGuardInfo, &LockGuardInfo), callchain: &Vec<Span>) {
        let src1 = pair.0.src.as_ref().unwrap().clone();
        let src2 = pair.1.src.as_ref().unwrap().clone();
        assert!(src1 == src2);
        self.reports.entry(src1).or_insert(HashMap::new()).entry(DoubleLockPair {
            first_lock_type_name: pair.0.type_name.clone(),
            first_lock_span: pair.0.span,
            second_lock_type_name: pair.1.type_name.clone(),
            second_lock_span: pair.1.span,
        }).or_insert(HashSet::new()).insert(callchain.clone());
    }

    pub fn _print(&self) {
        println!("{:#?}", self.reports);
    }

    pub fn pretty_print(&self) {
        for (src, pairs_chains) in self.reports.iter() {
            println!("LockGuardSrc: {:?}", src);
            for (pair, chains) in pairs_chains {
                println!("{{\tFirstLock: {:?}\n\t\t{:?}", pair.first_lock_type_name, pair.first_lock_span);
                println!("\tSecondLock: {:?}\n\t\t{:?}", pair.second_lock_type_name, pair.second_lock_span);
                println!("\tCallchains: {:?}\n}}", chains);
            }
        }
    }
}