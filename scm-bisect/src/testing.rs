//! Testing utilities.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;

use crate::basic_search::BasicSourceControlGraph;

/// Testing graph representing a "stick" of nodes, represented as increasing
/// integers. The node `n` is the immediate parent of `n + 1`. Hence each node
/// has exactly one parent and one child node, except for the top node (which
/// has no parent) and bottom node (which has no child).
#[derive(Clone, Debug)]
pub struct UsizeGraph {
    /// The maximum node value for this graph. Valid nodes are in `0..max` (a
    /// half-open [`std::ops::Range`] from `0` to `max - 1`).
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

/// General-purpose testing directed acyclic graph with arbitrary parent-child
/// relationships.  Nodes are represented with `char`, and edges are represented
/// with `char -> char`.
#[derive(Clone, Debug)]
pub struct TestGraph {
    /// Mapping from parent to children that defines the graph.
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

/// Select an arbitrary [`BasicStrategyKind`].
#[cfg(test)]
pub fn arb_strategy(
) -> impl proptest::strategy::Strategy<Value = crate::basic_search::BasicStrategyKind> {
    use crate::basic_search::BasicStrategyKind;
    use proptest::prelude::*;

    prop_oneof![
        Just(BasicStrategyKind::Linear),
        Just(BasicStrategyKind::LinearReverse),
        Just(BasicStrategyKind::Binary),
    ]
}

/// Create an arbitrary [`TestGraph`] and an arbitrary set of failing nodes.
#[cfg(test)]
pub fn arb_test_graph_and_nodes(
    max_num_nodes: usize,
) -> impl proptest::strategy::Strategy<Value = (TestGraph, Vec<char>)> {
    use itertools::Itertools;
    use proptest::prelude::*;

    let possible_nodes = 'a'..='z';
    assert!(max_num_nodes <= possible_nodes.try_len().unwrap());
    let possible_nodes = possible_nodes.take(max_num_nodes).collect_vec();

    let nodes = prop::collection::hash_set(prop::sample::select(possible_nodes), 1..=max_num_nodes);
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
