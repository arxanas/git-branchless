//! Testing utilities.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;

use itertools::Itertools;
use proptest::prelude::Strategy as ProptestStrategy;
use proptest::prelude::*;

use crate::basic_search::{BasicSourceControlGraph, BasicStrategyKind};

#[derive(Clone, Debug)]
pub struct UsizeGraph {
    pub max: usize,
}

impl BasicSourceControlGraph for UsizeGraph {
    type Node = usize;
    type Error = Infallible;

    fn ancestors(&self, node: Self::Node) -> Result<HashSet<Self::Node>, Infallible> {
        assert!(node < self.max);
        Ok((0..=node).collect())
    }

    fn descendants(&self, node: Self::Node) -> Result<HashSet<Self::Node>, Infallible> {
        assert!(node < self.max);
        Ok((node..self.max).collect())
    }
}

#[derive(Clone, Debug)]
pub struct TestGraph {
    pub nodes: HashMap<char, HashSet<char>>,
}

impl BasicSourceControlGraph for TestGraph {
    type Node = char;
    type Error = Infallible;

    fn ancestors(&self, node: Self::Node) -> Result<HashSet<Self::Node>, Infallible> {
        let mut result = HashSet::new();
        result.insert(node);
        let parents: HashSet<char> = self
            .nodes
            .iter()
            .filter_map(|(k, v)| if v.contains(&node) { Some(*k) } else { None })
            .collect();
        result.extend(self.ancestors_all(parents)?);
        Ok(result)
    }

    fn descendants(&self, node: Self::Node) -> Result<HashSet<Self::Node>, Infallible> {
        let mut result = HashSet::new();
        result.insert(node);
        let children: HashSet<char> = self.nodes[&node].clone();
        result.extend(self.descendants_all(children)?);
        Ok(result)
    }
}

pub fn arb_strategy() -> impl ProptestStrategy<Value = BasicStrategyKind> {
    prop_oneof![
        Just(BasicStrategyKind::Linear),
        Just(BasicStrategyKind::LinearReverse),
        Just(BasicStrategyKind::Binary),
    ]
}

pub fn arb_test_graph_and_nodes() -> impl ProptestStrategy<Value = (TestGraph, Vec<char>)> {
    let nodes = prop::collection::hash_set(
        prop::sample::select(vec!['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h']),
        1..=8,
    );
    nodes
        .prop_flat_map(|nodes| {
            let num_nodes = nodes.len();
            let nodes_kv = nodes
                .iter()
                .copied()
                .map(|node| (node, HashSet::new()))
                .collect();
            let graph = TestGraph { nodes: nodes_kv };
            let lineages = prop::collection::vec(
                prop::sample::subsequence(nodes.into_iter().collect_vec(), 0..num_nodes),
                0..num_nodes,
            );
            (Just(graph), lineages)
        })
        .prop_map(|(mut graph, lineages)| {
            for lineage in lineages {
                for (parent, child) in lineage.into_iter().tuple_windows() {
                    graph.nodes.get_mut(&parent).unwrap().insert(child);
                }
            }
            graph
        })
        .prop_flat_map(|graph| {
            let nodes = graph.nodes.keys().copied().collect::<Vec<_>>();
            let num_nodes = nodes.len();
            let failure_nodes = prop::sample::subsequence(nodes, 0..num_nodes);
            (Just(graph), failure_nodes)
        })
}
