//! Testing utilities.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;

use itertools::Itertools;

use crate::basic_search::BasicSourceControlGraph;
use crate::search::NodeSet;

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

    fn universe(&self) -> Result<NodeSet<Self::Node>, Self::Error> {
        Ok((0..self.max).collect())
    }

    fn children(&self, node: &Self::Node) -> Result<NodeSet<Self::Node>, Infallible> {
        assert!(*node < self.max);
        if *node + 1 == self.max {
            Ok(NodeSet::default())
        } else {
            Ok([*node + 1].into_iter().collect())
        }
    }

    fn parents(&self, node: &Self::Node) -> Result<NodeSet<Self::Node>, Infallible> {
        assert!(*node < self.max);
        if *node == 0 {
            Ok(NodeSet::default())
        } else {
            Ok([*node - 1].into_iter().collect())
        }
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

    fn universe(&self) -> Result<NodeSet<Self::Node>, Self::Error> {
        Ok(self.nodes.keys().copied().collect())
    }

    // @nocommit
    fn sort(
        &self,
        nodes: impl IntoIterator<Item = Self::Node>,
    ) -> Result<Vec<Self::Node>, Self::Error> {
        let mut nodes = nodes.into_iter().collect::<Vec<_>>();
        nodes.sort();
        for (lhs, rhs) in nodes.iter().tuple_windows() {
            assert!(
                !self.is_ancestor(rhs, lhs)?,
                "graph must be defined such that {lhs:?} is not an ancestor of {rhs:?}"
            );
        }
        Ok(nodes)
    }

    fn parents(&self, node: &Self::Node) -> Result<NodeSet<Self::Node>, Self::Error> {
        let mut result = NodeSet::default();
        for (parent, children) in &self.nodes {
            if children.contains(node) {
                result.insert_mut(*parent);
            }
        }
        Ok(result)
    }

    fn children(&self, node: &Self::Node) -> Result<NodeSet<Self::Node>, Self::Error> {
        let children = self.nodes.get(node);
        match children {
            Some(children) => Ok(children.iter().copied().collect()),
            None => Ok(NodeSet::default()),
        }
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
    use proptest::prelude::*;

    let possible_nodes = 'a'..='z';
    assert!(max_num_nodes <= possible_nodes.try_len().unwrap());
    let possible_nodes = possible_nodes.take(max_num_nodes).collect_vec();

    let nodes = prop::collection::hash_set(prop::sample::select(possible_nodes), 0..=max_num_nodes);
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
                prop::sample::subsequence(nodes.into_iter().collect_vec(), 0..=num_nodes),
                0..=num_nodes,
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
            let failure_nodes = prop::sample::subsequence(nodes, 0..=num_nodes);
            (Just(graph), failure_nodes)
        })
}
