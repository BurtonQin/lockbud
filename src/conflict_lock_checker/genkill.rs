extern crate rustc_hir;
extern crate rustc_middle;

use super::config::RUN_LIMIT;
use super::lock::ConflictLockInfo;
use super::lock::{LockGuardId, LockGuardInfo};
use rustc_hir::def_id::LocalDefId;
use rustc_middle::mir::{BasicBlock, Body, START_BLOCK};
use std::collections::HashMap;
use std::collections::HashSet;
pub struct GenKill<'a> {
    gen: HashMap<BasicBlock, HashSet<LockGuardId>>,
    kill: HashMap<BasicBlock, HashSet<LockGuardId>>,
    before: HashMap<BasicBlock, HashSet<LockGuardId>>,
    after: HashMap<BasicBlock, HashSet<LockGuardId>>,
    worklist: Vec<BasicBlock>,
    crate_lockguards: &'a HashMap<LockGuardId, LockGuardInfo>,
}

impl<'a> GenKill<'a> {
    pub fn new(
        fn_id: LocalDefId,
        body: &Body,
        crate_lockguards: &'a HashMap<LockGuardId, LockGuardInfo>,
        context: &HashSet<LockGuardId>,
    ) -> GenKill<'a> {
        let mut gen: HashMap<BasicBlock, HashSet<LockGuardId>> = HashMap::new();
        let mut kill: HashMap<BasicBlock, HashSet<LockGuardId>> = HashMap::new();
        let mut before: HashMap<BasicBlock, HashSet<LockGuardId>> = HashMap::new();
        let mut after: HashMap<BasicBlock, HashSet<LockGuardId>> = HashMap::new();
        let mut worklist = Vec::new();
        for (id, lockguard) in crate_lockguards.iter() {
            if id.fn_id != fn_id {
                continue;
            }
            for bb in lockguard.gen_bbs.iter() {
                gen.entry(*bb).or_insert_with(HashSet::new).insert(*id);
            }
            for bb in lockguard.kill_bbs.iter() {
                kill.entry(*bb).or_insert_with(HashSet::new).insert(*id);
            }
        }

        for (bb, _) in body.basic_blocks().iter_enumerated() {
            before.insert(bb, HashSet::new());
            after.insert(bb, HashSet::new());
            worklist.push(bb);
        }
        if let Some(v) = before.get_mut(&START_BLOCK) {
            *v = context.clone();
        }
        Self {
            gen,
            kill,
            before,
            after,
            worklist,
            crate_lockguards,
        }
    }
    pub fn analyze(&mut self, body: &Body) -> Vec<ConflictLockInfo> {
        let mut conflict_lock_bugs: Vec<ConflictLockInfo> = Vec::new();
        let mut count: u32 = 0;
        while !self.worklist.is_empty() && count <= RUN_LIMIT {
            count += 1;
            let cur = self.worklist.pop().unwrap();
            let mut new_before: HashSet<LockGuardId> = HashSet::new();
            // copy after[prev] to new_before
            let prevs = &body.predecessors()[cur];
            if !prevs.is_empty() {
                for prev in prevs {
                    new_before.extend(self.after[prev].clone().into_iter());
                    self.before
                        .get_mut(&cur)
                        .unwrap()
                        .extend(new_before.clone().into_iter());
                }
            } else {
                new_before.extend(self.before[&cur].clone().into_iter());
            }
            // first kill, then gen
            if let Some(lockguards) = self.kill.get(&cur) {
                self.kill_kill_set(&mut new_before, lockguards);
            }
            if let Some(lockguards) = self.gen.get(&cur) {
                let conflict_locks = self.union_gen_set(&mut new_before, lockguards);
                conflict_lock_bugs.extend(conflict_locks.into_iter());
            }
            if !self.compare_lockguards(&new_before, &self.after[&cur]) {
                self.after.insert(cur, new_before);
                self.worklist.extend(
                    body.basic_blocks()[cur]
                        .terminator()
                        .successors()
                        .clone()
                        .into_iter(),
                );
            }
        }
        conflict_lock_bugs
    }

    pub fn get_live_lockguards(&self, bb: &BasicBlock) -> Option<&HashSet<LockGuardId>> {
        if let Some(context) = self.before.get(bb) {
            if !context.is_empty() {
                return Some(context);
            } else {
                return None;
            }
        }
        None
    }
    fn union_gen_set(
        &self,
        new_before: &mut HashSet<LockGuardId>,
        lockguards: &HashSet<LockGuardId>,
    ) -> Vec<ConflictLockInfo> {
        let mut conflict_locks: Vec<ConflictLockInfo> = Vec::new();
        for first in new_before.iter() {
            for second in lockguards.iter() {
                if self.crate_lockguards.get(first).unwrap()
                    != self.crate_lockguards.get(second).unwrap()
                {
                    conflict_locks.push(ConflictLockInfo {
                        first: *first,
                        second: *second,
                    });
                }
            }
        }
        new_before.extend(lockguards.clone().into_iter());
        conflict_locks
    }

    fn kill_kill_set(
        &self,
        new_before: &mut HashSet<LockGuardId>,
        lockguards: &HashSet<LockGuardId>,
    ) {
        new_before.retain(move |b| {
            let b = self.crate_lockguards.get(b).unwrap();
            if lockguards
                .iter()
                .map(move |k| self.crate_lockguards.get(k).unwrap())
                .find(|e| **e == *b)
                .is_some()
            {
                return false;
            }

            true
        });
    }

    fn compare_lockguards(&self, lhs: &HashSet<LockGuardId>, rhs: &HashSet<LockGuardId>) -> bool {
        if lhs.len() != rhs.len() {
            return false;
        }
        let rhs_info = rhs
            .iter()
            .map(|r| self.crate_lockguards.get(r).unwrap())
            .collect::<Vec<_>>();
        lhs.iter()
            .map(move |l| self.crate_lockguards.get(l).unwrap())
            .all(move |li| rhs_info.contains(&li))
    }
}
