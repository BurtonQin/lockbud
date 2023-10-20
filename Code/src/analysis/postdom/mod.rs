#![allow(dead_code)]
extern crate rustc_data_structures;
extern crate rustc_index;
extern crate rustc_middle;

use rustc_data_structures::graph::{
    ControlFlowGraph, DirectedGraph, WithNumNodes, WithPredecessors, WithSuccessors,
};
use rustc_index::{Idx, IndexVec};
use rustc_middle::mir::{BasicBlock, BasicBlocks, Location, TerminatorKind};
use std::borrow::BorrowMut;

#[cfg(test)]
mod tests;

pub fn post_dominates(
    this: Location,
    other: Location,
    post_dominators: &PostDominators<BasicBlock>,
) -> bool {
    if this.block == other.block {
        other.statement_index <= this.statement_index
    } else {
        post_dominators.is_post_dominated_by(other.block, this.block)
    }
}

pub trait WithEndNodes: DirectedGraph {
    fn end_nodes(&self) -> Vec<Self::Node>;
}

impl<'graph, G: WithEndNodes> WithEndNodes for &'graph G {
    fn end_nodes(&self) -> Vec<Self::Node> {
        (**self).end_nodes()
    }
}

impl<'tcx> WithEndNodes for BasicBlocks<'tcx> {
    #[inline]
    fn end_nodes(&self) -> Vec<Self::Node> {
        self.iter_enumerated()
            .filter_map(|(bb, bb_data)| {
                if self.successors(bb).count() == 0 {
                    if bb_data.terminator().kind == TerminatorKind::Return {
                        Some(bb)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect()
    }
}

pub trait EndsControlFlowGraph: ControlFlowGraph + WithEndNodes {
    // convenient trait
}

impl<T> EndsControlFlowGraph for T where T: ControlFlowGraph + WithEndNodes {}

pub fn post_dominators<G: EndsControlFlowGraph>(graph: G) -> PostDominators<G::Node> {
    let end_nodes = graph.end_nodes();
    let rpo = postdom_reverse_post_order(&graph, end_nodes);
    post_dominators_given_rpo(graph, &rpo)
}

pub fn postdom_reverse_post_order<
    G: DirectedGraph + WithPredecessors + WithNumNodes + WithEndNodes,
>(
    graph: &G,
    end_nodes: Vec<G::Node>,
) -> Vec<G::Node> {
    let mut vec = postdom_post_order_from(graph, end_nodes);
    vec.reverse();
    vec
}

pub fn postdom_post_order_from<
    G: DirectedGraph + WithPredecessors + WithNumNodes + WithEndNodes,
>(
    graph: &G,
    end_nodes: Vec<G::Node>,
) -> Vec<G::Node> {
    postdom_post_order_from_to(graph, end_nodes, None)
}

pub fn postdom_post_order_from_to<
    G: DirectedGraph + WithPredecessors + WithNumNodes + WithEndNodes,
>(
    graph: &G,
    end_nodes: Vec<G::Node>,
    start_node: Option<G::Node>,
) -> Vec<G::Node> {
    let mut visited: IndexVec<G::Node, bool> = IndexVec::from_elem_n(false, graph.num_nodes());
    let mut result: Vec<G::Node> = Vec::with_capacity(graph.num_nodes());
    if let Some(start_node) = start_node {
        visited[start_node] = true;
    }
    for end_node in end_nodes {
        postdom_post_order_walk(graph, end_node, &mut result, &mut visited);
    }
    result
}

fn postdom_post_order_walk<G: DirectedGraph + WithPredecessors + WithNumNodes>(
    graph: &G,
    node: G::Node,
    result: &mut Vec<G::Node>,
    visited: &mut IndexVec<G::Node, bool>,
) {
    if visited[node] {
        return;
    }
    visited[node] = true;

    for predecessor in graph.predecessors(node) {
        postdom_post_order_walk(graph, predecessor, result, visited);
    }

    result.push(node);
}

fn post_dominators_given_rpo<G: ControlFlowGraph + BorrowMut<G> + WithEndNodes>(
    mut graph: G,
    rpo: &[G::Node],
) -> PostDominators<G::Node> {
    let end_nodes = graph.borrow().end_nodes();
    let end_idx = rpo.len() - 1;

    // compute the post order index (rank) for each node
    let mut post_order_rank: IndexVec<G::Node, usize> =
        (0..graph.borrow().num_nodes()).map(|_| 0).collect();
    for (index, node) in rpo.iter().rev().cloned().enumerate() {
        post_order_rank[node] = index;
    }
    for node in end_nodes.iter().copied() {
        post_order_rank[node] = end_idx;
    }

    let mut immediate_post_dominators: IndexVec<G::Node, ExtNode<G::Node>> =
        (0..graph.borrow().num_nodes())
            .map(|_| ExtNode::Real(None))
            .collect();

    for node in end_nodes.iter().copied() {
        immediate_post_dominators[node] = ExtNode::Real(Some(node));
    }

    let mut changed = true;
    while changed {
        changed = false;
        for &node in rpo {
            if end_nodes.contains(&node) {
                continue;
            }
            let mut new_ipdom = ExtNode::Real(None);
            for succ in graph.borrow_mut().successors(node) {
                match immediate_post_dominators[succ] {
                    ExtNode::Real(Some(_)) => {
                        new_ipdom = match new_ipdom {
                            ExtNode::Real(Some(new_ipdom)) => intersect(
                                &post_order_rank,
                                &immediate_post_dominators,
                                &end_nodes,
                                new_ipdom,
                                succ,
                            ),
                            ExtNode::Real(None) => ExtNode::Real(Some(succ)),
                            ExtNode::Fake => ExtNode::Fake,
                        };
                    }
                    ExtNode::Real(None) => {
                        // pass
                    }
                    ExtNode::Fake => {
                        new_ipdom = ExtNode::Fake;
                    }
                }
            }

            if new_ipdom != immediate_post_dominators[node] {
                immediate_post_dominators[node] = new_ipdom;
                changed = true;
            }
        }
    }

    PostDominators {
        post_order_rank,
        immediate_post_dominators,
    }
}

fn intersect<Node: Idx>(
    post_order_rank: &IndexVec<Node, usize>,
    immediate_post_dominators: &IndexVec<Node, ExtNode<Node>>,
    end_nodes: &[Node],
    mut node1: Node,
    mut node2: Node,
) -> ExtNode<Node> {
    while node1 != node2 {
        if end_nodes.contains(&node1) && end_nodes.contains(&node2) {
            return ExtNode::Fake;
        }
        while post_order_rank[node1] < post_order_rank[node2] {
            match immediate_post_dominators[node1] {
                ExtNode::Real(Some(n)) => node1 = n,
                ExtNode::Real(None) | ExtNode::Fake => break,
            };
        }

        while post_order_rank[node2] < post_order_rank[node1] {
            match immediate_post_dominators[node2] {
                ExtNode::Real(Some(n)) => node2 = n,
                ExtNode::Real(None) | ExtNode::Fake => break,
            };
        }
    }

    ExtNode::Real(Some(node1))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtNode<N: Idx> {
    Real(Option<N>),
    Fake,
}

impl<N: Idx> ExtNode<N> {
    pub fn is_none(&self) -> bool {
        matches!(self, ExtNode::Real(None))
    }
}

#[derive(Clone, Debug)]
pub struct PostDominators<N: Idx> {
    post_order_rank: IndexVec<N, usize>,
    immediate_post_dominators: IndexVec<N, ExtNode<N>>,
}

impl<Node: Idx> PostDominators<Node> {
    pub fn is_reachable(&self, node: Node) -> bool {
        match self.immediate_post_dominators[node] {
            ExtNode::Real(None) => false,
            ExtNode::Real(Some(_)) => true,
            ExtNode::Fake => true,
        }
    }

    pub fn immediate_post_dominator(&self, node: Node) -> ExtNode<Node> {
        // assert!(self.is_reachable(node), "node {:?} is not reachable", node);
        self.immediate_post_dominators[node]
    }

    pub fn post_dominators(&self, node: Node) -> Iter<'_, Node> {
        // assert!(self.is_reachable(node), "node {:?} is not reachable", node);
        Iter {
            post_dominators: self,
            node: ExtNode::Real(Some(node)),
        }
    }

    pub fn is_post_dominated_by(&self, node: Node, dom: Node) -> bool {
        // FIXME -- could be optimized by using post-order-rank
        self.post_dominators(node).any(|n| n == dom)
    }

    #[allow(clippy::needless_collect)]
    pub fn find_nearest_common_dominator(&self, node1: Node, node2: Node) -> Option<Node> {
        if node1 == node2 {
            return Some(node1);
        }

        let pd1: Vec<_> = self.post_dominators(node1).collect();
        let pd2: Vec<_> = self.post_dominators(node2).collect();
        let mut common = None;
        // post_dominators iter does not implement DoubleEndedIterator, thus cannot directly call `rev()`
        for (n1, n2) in pd1.into_iter().rev().zip(pd2.into_iter().rev()) {
            if n1 != n2 {
                break;
            } else {
                common = Some(n1);
            }
        }
        common
    }
}

pub struct Iter<'dom, Node: Idx> {
    post_dominators: &'dom PostDominators<Node>,
    node: ExtNode<Node>,
}

impl<'dom, Node: Idx> Iterator for Iter<'dom, Node> {
    type Item = Node;

    fn next(&mut self) -> Option<Self::Item> {
        match self.node {
            ExtNode::Real(Some(node)) => {
                match self.post_dominators.immediate_post_dominator(node) {
                    ExtNode::Real(Some(dom)) => {
                        if dom == node {
                            self.node = ExtNode::Real(None); // reached the root
                        } else {
                            self.node = ExtNode::Real(Some(dom));
                        }
                        Some(node)
                    }
                    ExtNode::Real(None) => {
                        // panic!("post_dominators have uncomputed nodes");
                        None
                    }
                    ExtNode::Fake => {
                        self.node = ExtNode::Fake;
                        Some(node)
                    }
                }
            }
            ExtNode::Real(None) => None,
            ExtNode::Fake => None,
        }
    }
}
