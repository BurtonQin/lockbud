use rustc_middle::mir::TerminatorKind;
use rustc_middle::mir::Body;
use rustc_data_structures::graph::{DirectedGraph, ControlFlowGraph};
use rustc_index::vec::{Idx, IndexVec};
use std::borrow::BorrowMut;

pub fn is_exit_term(term_kind: TerminatorKind) -> bool {
    match term_kind {
        TerminatorKind::Resume | TerminatorKind::Abort | TerminatorKind::Return | TerminatorKind::Unreachable => true,
        _ => false,
    }
}

pub fn post_post_order_from<G: DirectedGraph + WithPredecessors + WithNumNodes>(
    graph: &G,
    exit_nodes: Vec<G::Node>,
) -> Vec<G::Node> {
    post_post_order_from_to(graph, exit_nodes, None)
}

pub fn post_post_order_from_to<G: DirectedGraph + WithPredecessors + WithNumNodes>(
    graph: &G,
    exit_nodes: Vec<G::Node>,
    end_node: Option<G::Node>,
) -> Vec<G::Node> {
    let mut visited: IndexVec<G::Node, bool> = IndexVec::from_elem_n(false, graph.num_nodes());
    let mut result: Vec<G::Node> = Vec::with_capacity(graph.num_nodes());
    if let Some(end_node) = end_node {
        visited[end_node] = true;
    }
    for exit_node in exits_nodes {
        post_post_order_walk(graph, exit_node, &mut result, &mut visited);
    }
    result
}

fn post_post_order_walk<G: DirectedGraph + WithPredecessors + WithNumNodes>(
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
        post_post_order_walk(graph, predecessor, result, visited);
    }

    result.push(node);
}

pub fn post_reverse_post_order<G: DirectedGraph + WithPredecessors + WithNumNodes>(
    graph: &G,
    start_node: G::Node,
) -> Vec<G::Node> {
    let mut vec = post_post_order_from(graph, start_node);
    vec.reverse();
    vec
}
pub trait WithExitNodes: DirectedGraph {
    fn virtual_exit_node(&self) -> Self::Node;
    fn exit_nodes(&self) -> Vec<Self::Node>;
}

impl<'tcx> WithExitNodes for Body<'tcx> {
    fn virtual_exit_node(&self) -> Self::Node {
        self.basic_blocks().len() + 1
    }

    fn exit_nodes(&self) -> Vec<Self::Node> {
        let mut result = Vec::new();
        for (bb, bb_data) in self.basic_blocks().iter_enumerated() {
            if is_exit_term(bb_data.terminator().kind) {
                result.push(bb);
            }
        }
        result
    }
}

// virtual exit node post-dom all exit nodes

#[derive(Clone, Debug)]
pub struct PostDominators<N: Idx> {
    post_order_rank: IndexVec<N, usize>,
    immediate_post_dominators: IndexVec<N, Option<N>>,
}

impl<Node: Idx> PostDominators<Node> {
    pub fn is_post_reachable(&self, node: Node) -> bool {
        self.immediate_post_dominators[node].is_some()
    }

    pub fn immediate_post_dominator(&self, node: Node) -> Node {
        assert!(self.is_post_reachable(node), "node {:?} is not reachable", node);
        self.immediate_dominators[node].unwrap()
    }

    pub fn post_dominators(&self, node: Node) -> Iter<'_, Node> {
        assert!(self.is_post_reachable(node), "node {:?} is not reachable", node);
        Iter { dominators: self, node: Some(node) }
    }

    pub fn is_post_dominated_by(&self, node: Node, dom: Node) -> bool {
        // FIXME -- could be optimized by using post-order-rank
        self.post_dominators(node).any(|n| n == dom)
    }
}
pub trait PostControlFlowGraph: ControlFlowGraph + WithExitNodes {
    // fn exit_nodes() -> 
}

pub fn post_dominators<G: PostControlFlowGraph>(graph: G) -> Dominators<G::Node> {
    // 1. get virtual exit
    // 2. virtual exit's preds = exit_nodes
    // 3. virtual exit immediate_post_dom exit_nodes
    let exit_node = graph.virtual_exit_node();
    let rpo = post_reverse_post_order(&graph, exit_node);
    post_dominators_given_rpo(graph, &rpo)
}

fn post_dominators_given_rpo<G: PostControlFlowGraph + BorrowMut<G>>(
    mut graph: G,
    rpo: &[G::Node],
) -> PostDominators<G::Node> {
    let exit_nodes = graph.borrow().exit_nodes();

    // compute the post order index (rank) for each node
    let mut post_post_order_rank: IndexVec<G::Node, usize> =
        (0..graph.borrow().num_nodes()).map(|_| 0).collect();
    for (index, node) in rpo.iter().rev().cloned().enumerate() {
        post_post_order_rank[node] = index;
    }

    let mut post_immediate_dominators: IndexVec<G::Node, Option<G::Node>> =
        (0..graph.borrow().num_nodes()).map(|_| None).collect();
    for exit_node in exit_nodes {
        post_immediate_dominators[exit_node] = Some(exit_node);
    }

    let mut changed = true;
    while changed {
        changed = false;

        for &node in &rpo[1..] {
            let mut new_idom = None;
            for pred in graph.borrow_mut().successors(node) {
                if immediate_dominators[pred].is_some() {
                    // (*) dominators for `pred` have been calculated
                    new_idom = Some(if let Some(new_idom) = new_idom {
                        intersect(&post_order_rank, &immediate_dominators, new_idom, pred)
                    } else {
                        pred
                    });
                }
            }

            if new_idom != immediate_dominators[node] {
                immediate_dominators[node] = new_idom;
                changed = true;
            }
        }
    }

    Dominators { post_order_rank, immediate_dominators }
}

fn intersect<Node: Idx>(
    post_post_order_rank: &IndexVec<Node, usize>,
    post_immediate_dominators: &IndexVec<Node, Option<Node>>,
    mut node1: Node,
    mut node2: Node,
) -> Node {
    while node1 != node2 {
        while post_post_order_rank[node1] < post_post_order_rank[node2] {
            node1 = post_immediate_dominators[node1].unwrap();
        }

        while post_post_order_rank[node2] < post_post_order_rank[node1] {
            node2 = post_immediate_dominators[node2].unwrap();
        }
    }

    node1
}
pub struct Iter<'dom, Node: Idx> {
    post_dominators: &'dom PostDominators<Node>,
    node: Option<Node>,
}

impl<'dom, Node: Idx> Iterator for Iter<'dom, Node> {
    type Item = Node;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(node) = self.node {
            let post_dom = self.post_dominators.immediate_post_dominator(node);
            if post_dom == node {
                self.node = None; // reached the root
            } else {
                self.node = Some(post_dom);
            }
            Some(node)
        } else {
            None
        }
    }
}