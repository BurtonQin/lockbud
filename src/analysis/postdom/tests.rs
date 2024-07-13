use rustc_data_structures::fx::FxHashMap;
use std::cmp::max;

use super::*;

use rustc_data_structures::graph::{Predecessors, StartNode, Successors};

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

    fn num_nodes(&self) -> usize {
        self.num_nodes
    }
}

impl StartNode for TestGraph {
    fn start_node(&self) -> Self::Node {
        self.start_node
    }
}

impl WithEndNodes for TestGraph {
    fn end_nodes(&self) -> Vec<usize> {
        let mut result = vec![];
        for node in rustc_data_structures::graph::depth_first_search(self, self.start_node()) {
            if self.successors(node).count() == 0 {
                result.push(node);
            }
        }
        result.reverse();
        result
    }
}

impl Predecessors for TestGraph {
    fn predecessors(&self, node: Self::Node) -> impl Iterator<Item = Self::Node> {
        self.predecessors[&node].iter().cloned()
    }
}

impl Successors for TestGraph {
    fn successors(&self, node: Self::Node) -> impl Iterator<Item = Self::Node> {
        self.successors[&node].iter().cloned()
    }
}

#[test]
fn diamond_post_order() {
    let graph = TestGraph::new(0, &[(0, 1), (0, 2), (1, 3), (2, 3)]);

    let result = postdom_post_order_from(&graph, vec![3usize]);
    assert_eq!(result, vec![0, 1, 2, 3]);
    let pdt = post_dominators(graph);
    assert_eq!(pdt.find_nearest_common_dominator(1, 3), Some(3));
    assert_eq!(pdt.find_nearest_common_dominator(3, 1), Some(3));
    assert_eq!(pdt.find_nearest_common_dominator(1, 2), Some(3));
    assert_eq!(pdt.find_nearest_common_dominator(2, 1), Some(3));
    assert_eq!(pdt.find_nearest_common_dominator(0, 1), Some(3));
    assert_eq!(pdt.find_nearest_common_dominator(1, 0), Some(3));
    assert_eq!(pdt.find_nearest_common_dominator(0, 2), Some(3));
    assert_eq!(pdt.find_nearest_common_dominator(2, 0), Some(3));
    assert_eq!(pdt.find_nearest_common_dominator(0, 0), Some(0));
    assert_eq!(pdt.find_nearest_common_dominator(0, 3), Some(3));
    assert_eq!(pdt.find_nearest_common_dominator(3, 0), Some(3));
    assert_eq!(pdt.find_nearest_common_dominator(1, 1), Some(1));
    assert_eq!(pdt.find_nearest_common_dominator(2, 2), Some(2));
    assert_eq!(pdt.find_nearest_common_dominator(3, 3), Some(3));
}

#[test]
fn multi_ends_post_order() {
    let graph = TestGraph::new(0, &[(0, 1), (0, 2), (1, 3), (2, 4), (2, 5)]);

    let result = postdom_post_order_from(&graph, vec![3, 4, 5]);
    assert_eq!(result, vec![0, 1, 3, 2, 4, 5]);
    let pdt = post_dominators(graph);
    assert_eq!(pdt.find_nearest_common_dominator(0, 0), Some(0));
    assert_eq!(pdt.find_nearest_common_dominator(0, 1), None);
    assert_eq!(pdt.find_nearest_common_dominator(0, 3), None);
    assert_eq!(pdt.find_nearest_common_dominator(1, 2), None);
    assert_eq!(pdt.find_nearest_common_dominator(1, 3), Some(3));
    assert_eq!(pdt.find_nearest_common_dominator(2, 4), None);
    assert_eq!(pdt.find_nearest_common_dominator(2, 5), None);
    assert_eq!(pdt.find_nearest_common_dominator(1, 4), None);
    assert_eq!(pdt.find_nearest_common_dominator(2, 3), None);
    assert_eq!(pdt.find_nearest_common_dominator(3, 4), None);
    assert_eq!(pdt.find_nearest_common_dominator(4, 5), None);
}

#[test]
fn multi_ends_postdom() {
    let graph = TestGraph::new(0, &[(0, 1), (0, 2), (1, 3), (2, 4), (2, 5), (3, 6), (4, 6)]);
    let pdt = post_dominators(graph);
    for n in 0..7usize {
        println!("node: {:?}", n);
        for pd in pdt.post_dominators(n) {
            println!("pdt: {:?}", pd);
        }
    }
    assert_eq!(pdt.find_nearest_common_dominator(3, 6), Some(6));
    assert_eq!(pdt.find_nearest_common_dominator(1, 6), Some(6));
    assert_eq!(pdt.find_nearest_common_dominator(4, 6), Some(6));
    assert_eq!(pdt.find_nearest_common_dominator(1, 3), Some(3));
    assert_eq!(pdt.find_nearest_common_dominator(2, 6), None);
    assert_eq!(pdt.find_nearest_common_dominator(2, 4), None);
}
