//! Basic search strategies; see `BasicStrategyKind`.

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::hash::Hash;

use indexmap::IndexMap;
use tracing::instrument;

use crate::search;

/// Implementation of `Graph` that represents the common case of a directed
/// acyclic graph in source control. You can implement this trait instead of
/// `Graph` (as there is a blanket implementation for `Graph`) and also make use
/// of `BasicStrategy`.
pub trait BasicSourceControlGraph: Debug {
    /// The type of nodes in the graph. This should be cheap to clone.
    type Node: Clone + Debug + Hash + Eq + 'static;

    /// An error type.
    type Error: Debug + std::error::Error + 'static;

    /// Get every node `X` in the graph such that `X == node` or there exists a
    /// child of `X` that is an ancestor of `node`.
    fn ancestors(&self, node: Self::Node) -> Result<HashSet<Self::Node>, Self::Error>;

    /// Get the union of `ancestors(node)` for every node in `nodes`.
    #[instrument]
    fn ancestors_all(
        &self,
        nodes: HashSet<Self::Node>,
    ) -> Result<HashSet<Self::Node>, Self::Error> {
        let mut ancestors = HashSet::new();
        for node in nodes {
            ancestors.extend(self.ancestors(node)?);
        }
        Ok(ancestors)
    }

    /// Filter `nodes` to only include nodes that are not ancestors of any other
    /// node in `nodes`.
    fn ancestor_heads(
        &self,
        nodes: HashSet<Self::Node>,
    ) -> Result<HashSet<Self::Node>, Self::Error> {
        let node_to_ancestors: HashMap<Self::Node, HashSet<Self::Node>> = nodes
            .iter()
            .map(|node| Ok((node.clone(), self.ancestors(node.clone())?)))
            .collect::<Result<_, _>>()?;
        let heads: HashSet<Self::Node> = nodes
            .into_iter()
            .filter(|node| {
                node_to_ancestors
                    .iter()
                    .filter_map(|(other_node, ancestors)| {
                        if node == other_node {
                            None
                        } else {
                            Some(ancestors)
                        }
                    })
                    .all(|ancestors| !ancestors.contains(node))
            })
            .collect();
        Ok(heads)
    }

    /// Get every node `X` in the graph such that `X == node` or there exists a
    /// parent of `X` that is a descendant of `node`.
    fn descendants(&self, node: Self::Node) -> Result<HashSet<Self::Node>, Self::Error>;

    /// Filter `nodes` to only include nodes that are not descendants of any
    /// other node in `nodes`.
    fn descendant_roots(
        &self,
        nodes: HashSet<Self::Node>,
    ) -> Result<HashSet<Self::Node>, Self::Error> {
        let node_to_descendants: HashMap<Self::Node, HashSet<Self::Node>> = nodes
            .iter()
            .map(|node| Ok((node.clone(), self.descendants(node.clone())?)))
            .collect::<Result<_, _>>()?;
        let roots: HashSet<Self::Node> = nodes
            .into_iter()
            .filter(|node| {
                node_to_descendants
                    .iter()
                    .filter_map(|(other_node, descendants)| {
                        if node == other_node {
                            None
                        } else {
                            Some(descendants)
                        }
                    })
                    .all(|descendants| !descendants.contains(node))
            })
            .collect();
        Ok(roots)
    }

    /// Get the union of `descendants(node)` for every node in `nodes`.
    #[instrument]
    fn descendants_all(
        &self,
        nodes: HashSet<Self::Node>,
    ) -> Result<HashSet<Self::Node>, Self::Error> {
        let mut descendants = HashSet::new();
        for node in nodes {
            descendants.extend(self.descendants(node)?);
        }
        Ok(descendants)
    }
}

impl<T: BasicSourceControlGraph> search::Graph for T {
    type Node = <Self as BasicSourceControlGraph>::Node;
    type Error = <Self as BasicSourceControlGraph>::Error;

    fn is_ancestor(
        &self,
        ancestor: Self::Node,
        descendant: Self::Node,
    ) -> Result<bool, Self::Error> {
        let ancestors = self.ancestors(descendant)?;
        Ok(ancestors.contains(&ancestor))
    }

    fn simplify_success_bounds(
        &self,
        nodes: HashSet<Self::Node>,
    ) -> Result<HashSet<Self::Node>, Self::Error> {
        self.ancestor_heads(nodes)
    }

    fn simplify_failure_bounds(
        &self,
        nodes: HashSet<Self::Node>,
    ) -> Result<HashSet<Self::Node>, Self::Error> {
        self.descendant_roots(nodes)
    }
}

/// The possible strategies for searching the graph.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BasicStrategyKind {
    /// Search the nodes in the order that they were provided.
    Linear,

    /// Search the nodes in the reverse order that they were provided.
    LinearReverse,

    /// Conduct a binary search on the nodes by partitioning the nodes into two
    /// groups of approximately equal size.
    ///
    /// TODO: Partitioning into groups of approximately equal size isn't
    /// actually optimal for the DAG case. Really, we want to maximize the
    /// information that we gain from each test. The `git bisect` algorithm at
    /// https://git-scm.com/docs/git-bisect-lk2009#_bisection_algorithm_discussed
    /// discusses a metric to find the best partition for the subgraph which
    /// remains to be tested.
    ///
    /// See also `git-bisect`'s skip algorithm:
    /// https://git-scm.com/docs/git-bisect-lk2009#_skip_algorithm. This does
    /// *not* use the same skip algorithm, and instead uses a deterministic
    /// approach. In order to solve the following problem:
    ///
    /// > sometimes the best bisection points all happened to be in an area
    /// where all the commits are untestable. And in this case the user was
    /// asked to test many untestable commits, which could be very inefficient.
    ///
    /// We instead consider the hypothetical case that the node is a success,
    /// and yield further nodes as if it were a success, and then interleave
    /// those nodes with the hypothetical failure case.
    ///
    /// Resources:
    ///
    /// - https://git-scm.com/docs/git-bisect-lk2009#_bisection_algorithm_discussed
    /// - https://byorgey.wordpress.com/2023/01/01/competitive-programming-in-haskell-better-binary-search/
    /// - https://julesjacobs.com/notes/binarysearch/binarysearch.pdf
    Binary,
}

/// A set of basic search strategies defined by `BasicStrategyKind`.
#[derive(Clone, Debug)]
pub struct BasicStrategy {
    strategy: BasicStrategyKind,
}

impl BasicStrategy {
    /// Constructor.
    pub fn new(strategy: BasicStrategyKind) -> Self {
        Self { strategy }
    }
}

impl<G: BasicSourceControlGraph> search::Strategy<G> for BasicStrategy {
    type Error = G::Error;

    fn midpoint(
        &self,
        graph: &G,
        success_bounds: &HashSet<G::Node>,
        failure_bounds: &HashSet<G::Node>,
        statuses: &IndexMap<G::Node, search::Status>,
    ) -> Result<Option<G::Node>, G::Error> {
        let mut nodes_to_search = {
            let implied_success_nodes = graph.ancestors_all(success_bounds.clone())?;
            let implied_failure_nodes = graph.descendants_all(failure_bounds.clone())?;
            statuses
                .iter()
                .filter_map(|(node, status)| match status {
                    search::Status::Untested => Some(node.clone()),
                    search::Status::Success
                    | search::Status::Failure
                    | search::Status::Indeterminate => None,
                })
                .filter(|node| {
                    !implied_success_nodes.contains(node) && !implied_failure_nodes.contains(node)
                })
                .collect::<Vec<_>>()
        };
        let next_to_search: Option<G::Node> = match self.strategy {
            BasicStrategyKind::Linear => nodes_to_search.into_iter().next(),
            BasicStrategyKind::LinearReverse => nodes_to_search.into_iter().next_back(),
            BasicStrategyKind::Binary => {
                let middle_index = nodes_to_search.len() / 2;
                if middle_index < nodes_to_search.len() {
                    Some(nodes_to_search.swap_remove(middle_index))
                } else {
                    None
                }
            }
        };
        Ok(next_to_search)
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use itertools::Itertools;
    use maplit::{hashmap, hashset};
    use proptest::prelude::Strategy as ProptestStrategy;
    use proptest::prelude::*;
    use proptest::proptest;

    use crate::search::Bounds;
    use crate::search::EagerSolution;
    use crate::search::Search;
    use crate::search::Status;

    use super::BasicStrategyKind;
    use super::*;

    #[derive(Clone, Debug)]
    struct UsizeGraph {
        max: usize,
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

    #[test]
    fn test_search_stick() {
        let graph = UsizeGraph { max: 7 };
        let nodes = 0..graph.max;
        let linear_strategy = BasicStrategy {
            strategy: BasicStrategyKind::Linear,
        };
        let linear_reverse_strategy = BasicStrategy {
            strategy: BasicStrategyKind::LinearReverse,
        };
        let binary_strategy = BasicStrategy {
            strategy: BasicStrategyKind::Binary,
        };
        let mut search = Search::new(graph.clone(), nodes.clone());
        // let mut linear_search = Search::new(graph.clone(), linear_strategy, nodes.clone());
        // let mut linear_reverse_search =
        //     Search::new(graph.clone(), linear_reverse_strategy, nodes.clone());
        // let mut binary_search = Search::new(graph.clone(), binary_strategy, nodes.clone());

        assert_eq!(
            search
                .search(&linear_strategy)
                .unwrap()
                .into_eager()
                .unwrap(),
            EagerSolution {
                bounds: Default::default(),
                next_to_search: vec![0, 1, 2, 3, 4, 5, 6],
            }
        );
        assert_eq!(
            search
                .search(&linear_reverse_strategy)
                .unwrap()
                .into_eager()
                .unwrap(),
            EagerSolution {
                bounds: Default::default(),
                next_to_search: vec![6, 5, 4, 3, 2, 1, 0],
            }
        );
        assert_eq!(
            search
                .search(&binary_strategy)
                .unwrap()
                .into_eager()
                .unwrap(),
            EagerSolution {
                bounds: Default::default(),
                // Breadth-first search:
                // 0 1 2 3 4 5 6
                //       ^
                //   ^
                //           ^
                // ^
                //     ^
                //         ^
                //             ^
                next_to_search: vec![3, 1, 5, 0, 2, 4, 6],
            }
        );

        search.notify(2, Status::Success).unwrap();
        assert_eq!(
            search
                .search(&linear_strategy)
                .unwrap()
                .into_eager()
                .unwrap(),
            EagerSolution {
                bounds: Bounds {
                    success: hashset! {2},
                    failure: hashset! {},
                },
                next_to_search: vec![3, 4, 5, 6],
            }
        );
        assert_eq!(
            search
                .search(&binary_strategy)
                .unwrap()
                .into_eager()
                .unwrap(),
            EagerSolution {
                bounds: Bounds {
                    success: hashset! {2},
                    failure: hashset! {},
                },
                next_to_search: vec![5, 4, 6, 3],
            }
        );

        search.notify(5, Status::Failure).unwrap();
        assert_eq!(
            search
                .search(&linear_strategy)
                .unwrap()
                .into_eager()
                .unwrap(),
            EagerSolution {
                bounds: Bounds {
                    success: hashset! {2},
                    failure: hashset! {5},
                },
                next_to_search: vec![3, 4],
            }
        );
        assert_eq!(
            search
                .search(&binary_strategy)
                .unwrap()
                .into_eager()
                .unwrap(),
            EagerSolution {
                bounds: Bounds {
                    success: hashset! {2},
                    failure: hashset! {5},
                },
                next_to_search: vec![4, 3],
            }
        );

        search.notify(3, Status::Indeterminate).unwrap();
        assert_eq!(
            search
                .search(&binary_strategy)
                .unwrap()
                .into_eager()
                .unwrap(),
            EagerSolution {
                bounds: Bounds {
                    success: hashset! {2},
                    failure: hashset! {5},
                },
                next_to_search: vec![4],
            }
        );
    }

    #[test]
    fn test_search_inconsistent_notify() {
        let graph = UsizeGraph { max: 7 };
        let nodes = 0..graph.max;
        let mut search = Search::new(graph, nodes);

        search.notify(4, Status::Success).unwrap();
        insta::assert_debug_snapshot!(search.notify(3, Status::Failure), @r###"
        Err(
            InconsistentStateTransition {
                ancestor_node: 3,
                ancestor_status: Failure,
                descendant_node: 4,
                descendant_status: Success,
            },
        )
        "###);

        insta::assert_debug_snapshot!(search.notify(4, Status::Indeterminate), @r###"
        Err(
            IllegalStateTransition {
                node: 4,
                from: Success,
                to: Indeterminate,
            },
        )
        "###);

        search.notify(5, Status::Failure).unwrap();
        insta::assert_debug_snapshot!(search.notify(6, Status::Success), @r###"
        Err(
            InconsistentStateTransition {
                ancestor_node: 5,
                ancestor_status: Failure,
                descendant_node: 6,
                descendant_status: Success,
            },
        )
        "###);
    }

    #[derive(Clone, Debug)]
    struct TestGraph {
        nodes: HashMap<char, HashSet<char>>,
    }

    impl BasicSourceControlGraph for TestGraph {
        type Node = char;
        type Error = Infallible;

        fn ancestors(&self, node: Self::Node) -> Result<HashSet<Self::Node>, Infallible> {
            let mut result = hashset! {node};
            let parents: HashSet<char> = self
                .nodes
                .iter()
                .filter_map(|(k, v)| if v.contains(&node) { Some(*k) } else { None })
                .collect();
            result.extend(self.ancestors_all(parents)?);
            Ok(result)
        }

        fn descendants(&self, node: Self::Node) -> Result<HashSet<Self::Node>, Infallible> {
            let mut result = hashset! {node};
            let children: HashSet<char> = self.nodes[&node].clone();
            result.extend(self.descendants_all(children)?);
            Ok(result)
        }
    }

    #[test]
    fn test_search_dag() {
        let graph = TestGraph {
            // a -> b -> e -> f -> g
            // c -> d ->   -> h
            nodes: hashmap! {
                'a' => hashset! {'b'},
                'b' => hashset! {'e'},
                'c' => hashset! {'d'},
                'd' => hashset! {'e'},
                'e' => hashset! {'f', 'h'},
                'f' => hashset! {'g'},
                'g' => hashset! {},
                'h' => hashset! {},
            },
        };
        let linear_strategy = BasicStrategy {
            strategy: BasicStrategyKind::Linear,
        };
        assert_eq!(graph.descendants('e'), Ok(hashset! {'e', 'f', 'g', 'h'}));
        assert_eq!(graph.ancestors('e'), Ok(hashset! {'a', 'b', 'c', 'd', 'e'}));

        let mut search = Search::new(graph, 'a'..='h');
        assert_eq!(
            search
                .search(&linear_strategy)
                .unwrap()
                .into_eager()
                .unwrap(),
            EagerSolution {
                bounds: Default::default(),
                // Breadth-first search: we start from the roots of the graph in
                // parallel and proceed to the heads of the graph.
                next_to_search: vec!['a', 'c', 'b', 'd', 'e', 'f', 'h', 'g'],
            }
        );

        search.notify('b', Status::Success).unwrap();
        search.notify('g', Status::Failure).unwrap();
        assert_eq!(
            search
                .search(&linear_strategy)
                .unwrap()
                .into_eager()
                .unwrap(),
            EagerSolution {
                bounds: Bounds {
                    success: hashset! {'b'},
                    failure: hashset! {'g'},
                },
                next_to_search: vec!['c', 'd', 'e', 'f', 'h'],
            }
        );

        search.notify('e', Status::Success).unwrap();
        assert_eq!(
            search
                .search(&linear_strategy)
                .unwrap()
                .into_eager()
                .unwrap(),
            EagerSolution {
                bounds: Bounds {
                    success: hashset! {'e'},
                    failure: hashset! {'g'},
                },
                next_to_search: vec!['f', 'h'],
            }
        );

        search.notify('f', Status::Success).unwrap();
        assert_eq!(
            search
                .search(&linear_strategy)
                .unwrap()
                .into_eager()
                .unwrap(),
            EagerSolution {
                bounds: Bounds {
                    success: hashset! {'f'},
                    failure: hashset! {'g'},
                },
                next_to_search: vec!['h'],
            }
        );

        search.notify('h', Status::Success).unwrap();
        assert_eq!(
            search
                .search(&linear_strategy)
                .unwrap()
                .into_eager()
                .unwrap(),
            EagerSolution {
                bounds: Bounds {
                    success: hashset! {'f', 'h'},
                    failure: hashset! {'g'},
                },
                next_to_search: vec![],
            }
        );
    }

    fn arb_strategy() -> impl ProptestStrategy<Value = BasicStrategyKind> {
        prop_oneof![
            Just(BasicStrategyKind::Linear),
            Just(BasicStrategyKind::LinearReverse),
            Just(BasicStrategyKind::Binary),
        ]
    }

    fn arb_test_graph_and_nodes() -> impl ProptestStrategy<Value = (TestGraph, Vec<char>)> {
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

    proptest! {
        #[test]
        fn test_search_dag_proptest(strategy in arb_strategy(), (graph, failure_nodes) in arb_test_graph_and_nodes()) {
            let nodes = graph.nodes.keys().sorted().copied().collect::<Vec<_>>();
            let strategy = BasicStrategy {
                strategy,
            };
            let mut search = Search::new(graph.clone(), nodes);
            let failure_nodes = graph.descendants_all(failure_nodes.into_iter().collect()).unwrap();

            let solution = loop {
                let solution = search.search(&strategy).unwrap().into_eager().unwrap();
                let Bounds { success, failure } = &solution.bounds;
                for success_node in success {
                    assert!(!failure_nodes.contains(success_node))
                }
                for failure_node in failure {
                    assert!(failure_nodes.contains(failure_node));
                }
                match solution.next_to_search.first() {
                    Some(node) => {
                        search.notify(*node, if failure_nodes.contains(node) {
                            Status::Failure
                        } else {
                            Status::Success
                        }).unwrap();
                    }
                    None => break solution,
                }
            };

            let nodes = graph.nodes.keys().copied().collect::<HashSet<_>>();
            assert!(solution.bounds.success.is_subset(&nodes));
            assert!(solution.bounds.failure.is_subset(&nodes));
            assert!(solution.bounds.success.is_disjoint(&solution.bounds.failure));
            let all_success_nodes = graph.ancestors_all(solution.bounds.success.clone()).unwrap();
            let all_failure_nodes = graph.descendants_all(solution.bounds.failure).unwrap();
            assert!(all_success_nodes.is_disjoint(&all_failure_nodes));
            assert!(
                all_success_nodes.union(&all_failure_nodes).copied().collect::<HashSet<_>>() == nodes,
                "all_success_nodes: {all_success_nodes:?}, all_failure_nodes: {all_failure_nodes:?}, nodes: {nodes:?}",
            );
        }
    }
}
