use crate::analysis::postdom::WithEndNodes;
use rustc_data_structures::fx::FxHashMap;
use rustc_data_structures::graph::{
    DirectedGraph, GraphPredecessors, GraphSuccessors, WithNumNodes, WithPredecessors,
    WithStartNode, WithSuccessors,
};
use std::cmp::max;
use std::iter;
use std::slice;

use super::*;

pub struct TestGraph {
    num_nodes: usize,
    start_node: usize,
    successors: FxHashMap<usize, Vec<usize>>,
    predecessors: FxHashMap<usize, Vec<usize>>,
}

impl TestGraph {
    pub fn new(start_node: usize, edges: &[(usize, usize)]) -> Self {
        let mut graph = TestGraph {
            num_nodes: start_node + 1,
            start_node,
            successors: FxHashMap::default(),
            predecessors: FxHashMap::default(),
        };
        for &(source, target) in edges {
            graph.num_nodes = max(graph.num_nodes, source + 1);
            graph.num_nodes = max(graph.num_nodes, target + 1);
            graph.successors.entry(source).or_default().push(target);
            graph.predecessors.entry(target).or_default().push(source);
        }
        for node in 0..graph.num_nodes {
            graph.successors.entry(node).or_default();
            graph.predecessors.entry(node).or_default();
        }
        graph
    }
}

impl DirectedGraph for TestGraph {
    type Node = usize;
}

impl WithStartNode for TestGraph {
    fn start_node(&self) -> usize {
        self.start_node
    }
}

impl WithEndNodes for TestGraph {
    fn end_nodes(&self) -> Vec<usize> {
        let mut result = vec![];
        for node in self.depth_first_search(self.start_node()) {
            if self.successors(node).count() == 0 {
                result.push(node);
            }
        }
        result.reverse();
        result
    }
}

impl WithNumNodes for TestGraph {
    fn num_nodes(&self) -> usize {
        self.num_nodes
    }
}

impl WithPredecessors for TestGraph {
    fn predecessors(&self, node: usize) -> <Self as GraphPredecessors<'_>>::Iter {
        self.predecessors[&node].iter().cloned()
    }
}

impl WithSuccessors for TestGraph {
    fn successors(&self, node: usize) -> <Self as GraphSuccessors<'_>>::Iter {
        self.successors[&node].iter().cloned()
    }
}

impl<'graph> GraphPredecessors<'graph> for TestGraph {
    type Item = usize;
    type Iter = iter::Cloned<slice::Iter<'graph, usize>>;
}

impl<'graph> GraphSuccessors<'graph> for TestGraph {
    type Item = usize;
    type Iter = iter::Cloned<slice::Iter<'graph, usize>>;
}

#[test]
fn diamond_parents() {
    let graph = TestGraph::new(0, &[(0, 1), (0, 2), (1, 3), (2, 3)]);
    let control_deps = control_deps(graph);
    assert_eq!(
        format!("{:?}", control_deps.parents),
        "[{0}, {0}, {0}, {0}]"
    );
}

#[test]
fn multi_ends_parents() {
    let graph = TestGraph::new(0, &[(0, 1), (0, 2), (1, 3), (2, 4), (2, 5)]);
    let control_deps = control_deps(graph);
    assert_eq!(
        format!("{:?}", control_deps.parents),
        "[{0}, {0}, {0}, {0}, {2}, {2}]"
    );
}

#[test]
fn multi_ends_influences() {
    let graph = TestGraph::new(
        0,
        &[
            (0, 1),
            (0, 2),
            (1, 3),
            (1, 4),
            (2, 4),
            (2, 5),
            (3, 6),
            (4, 6),
        ],
    );
    let control_deps = control_deps(graph);
    assert_eq!(
        format!("{:?}", control_deps.parents),
        "[{0}, {0}, {0}, {1}, {1, 2}, {2}, {0, 2}]"
    );
    let true_pairs = [(1, 3), (1, 4), (2, 4), (2, 5), (2, 6)];
    for i in 0..7 {
        for j in 0..7 {
            if i == 0 || i == j || true_pairs.contains(&(i, j)) {
                assert!(control_deps.influences(i, j));
            } else {
                assert!(!control_deps.influences(i, j));
            }
        }
    }
}

#[test]
fn multi_ends_influences2() {
    let graph = TestGraph::new(
        0,
        &[
            (0, 1),
            (1, 2),
            (2, 3),
            (3, 4),
            (3, 5),
            (4, 6),
            (5, 6),
            (6, 10),
            (6, 7),
            (7, 8),
            (8, 10),
            (8, 9),
            (9, 10),
            (10, 11),
            (11, 12),
            (12, 13),
        ],
    );
    let control_deps = control_deps(graph);
    println!("control_deps = {control_deps:?}");
    println!("{}", control_deps.influences(3, 3));
    println!("{}", control_deps.influences(3, 4));
    println!("{}", control_deps.influences(3, 5));
    println!("{}", control_deps.influences(3, 6));
    println!("{}", control_deps.influences(3, 7));
    println!("{}", control_deps.influences(5, 6));
    println!("{}", control_deps.influences(5, 7));
    // assert_eq!(
    //     format!("{:?}", control_deps.parents),
    //     "[{0}, {0}, {0}, {1}, {1, 2}, {2}, {0, 2}]"
    // );
    // let true_pairs = [(1, 3), (1, 4), (2, 4), (2, 5), (2, 6)];
    // for i in 0..7 {
    //     for j in 0..7 {
    //         if i == 0 || i == j || true_pairs.contains(&(i, j)) {
    //             assert!(control_deps.influences(i, j));
    //         } else {
    //             assert!(!control_deps.influences(i, j));
    //         }
    //     }
    // }
}
