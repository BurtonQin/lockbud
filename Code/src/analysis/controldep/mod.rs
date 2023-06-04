extern crate rustc_data_structures;
extern crate rustc_index;
extern crate rustc_middle;

use std::collections::VecDeque;

use rustc_data_structures::fx::FxHashSet;
use rustc_index::vec::{Idx, IndexVec};
use rustc_middle::mir::{BasicBlock, Location};

use crate::analysis::postdom::{post_dominators, EndsControlFlowGraph};

#[cfg(test)]
mod tests;

pub fn influences(this: Location, other: Location, control_deps: &ControlDeps<BasicBlock>) -> bool {
    if this.block == other.block {
        return false;
    }
    control_deps.influences(this.block, other.block)
}

#[derive(Clone, Debug)]
pub struct ControlDeps<N: Idx> {
    parents: IndexVec<N, FxHashSet<N>>,
}

impl<Node: Idx> ControlDeps<Node> {
    pub fn influences(&self, influencer: Node, influencee: Node) -> bool {
        let mut worklist = VecDeque::from_iter([influencee]);
        let mut visited = FxHashSet::from_iter([influencee]);
        while let Some(n) = worklist.pop_front() {
            if n == influencer {
                return true;
            }
            for p in &self.parents[n] {
                if visited.insert(*p) {
                    worklist.push_front(*p)
                }
            }
        }
        false
    }
}

pub fn control_deps<G: EndsControlFlowGraph>(graph: G) -> ControlDeps<G::Node> {
    let mut parents = IndexVec::from_elem_n(FxHashSet::default(), graph.num_nodes());
    let pdt = post_dominators(&graph);
    let nodes = IndexVec::from_elem_n((), graph.num_nodes());
    for (a, _) in nodes.iter_enumerated() {
        for b in graph.successors(a) {
            if a != b && pdt.is_post_dominated_by(a, b) {
                continue;
            }
            if let Some(l) = pdt.find_nearest_common_dominator(a, b) {
                if a == l {
                    parents[a].insert(a);
                }
                for c in pdt.post_dominators(b) {
                    if c == l {
                        break;
                    } else {
                        parents[c].insert(a);
                    }
                }
            } else {
                // (fake) end node
                for c in pdt.post_dominators(b) {
                    parents[c].insert(a);
                }
            }
        }
    }
    let root = graph.start_node();
    for c in pdt.post_dominators(root) {
        parents[c].insert(root);
    }

    ControlDeps { parents }
}
