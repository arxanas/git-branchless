//! Reference implementations of basic search strategies as defined in [`search`].
//! See [`BasicStrategyKind`] for the list.

use std::cmp::{min, Ordering};
use std::fmt::Debug;
use std::hash::Hash;

use indexmap::IndexMap;
use itertools::Itertools;
use tracing::instrument;

use crate::search;
use crate::search::NodeSetExt;

/// Implementation of `Graph` that represents the common case of a directed
/// acyclic graph in source control. You can implement this trait instead of
/// `Graph` (as there is a blanket implementation for `Graph`) and also make use
/// of `BasicStrategy`.
pub trait BasicSourceControlGraph: Debug {
    /// The type of nodes in the graph. This should be cheap to clone.
    type Node: Clone + Debug + Hash + Eq + 'static;

    /// An error type.
    type Error: Debug + std::error::Error + 'static;

    /// The universe of nodes to search.
    fn universe(&self) -> Result<search::NodeSet<Self::Node>, Self::Error>;

    /// Topologically sort the provided nodes such that ancestor nodes come
    /// before descendant nodes.
    fn sort(
        &self,
        nodes: impl IntoIterator<Item = Self::Node>,
    ) -> Result<Vec<Self::Node>, Self::Error> {
        let mut nodes = nodes.into_iter().enumerate().collect::<Vec<_>>();
        let mut sort_err = None;
        nodes.sort_by(|(lhs_idx, lhs), (rhs_idx, rhs)| {
            match (self.is_ancestor(lhs, rhs), self.is_ancestor(rhs, lhs)) {
                (Ok(true), Ok(false)) => Ordering::Less,
                (Ok(false), Ok(true)) => Ordering::Greater,
                (Ok(true), Ok(true)) => Ordering::Equal,
                (Ok(false), Ok(false)) => lhs_idx.cmp(rhs_idx),
                (Err(err), _) | (_, Err(err)) => {
                    sort_err = Some(err);
                    Ordering::Equal
                }
            }
        });
        match sort_err {
            Some(sort_err) => Err(sort_err),
            None => Ok(nodes.into_iter().map(|(_, node)| node).collect()),
        }
    }

    /// Compute the set of nodes that are in `lhs` but not in `rhs`.
    fn difference(
        &self,
        lhs: search::NodeSet<Self::Node>,
        rhs: search::NodeSet<Self::Node>,
    ) -> Result<search::NodeSet<Self::Node>, Self::Error> {
        Ok(lhs
            .into_iter()
            .filter(|node| !rhs.contains(node))
            .cloned()
            .collect())
    }

    /// Return whether or not `node` is an ancestor of `descendant`. A node `X``
    /// is said to be an "ancestor" of node `Y` if one of the following is true:
    ///
    /// - `X == Y`
    /// - `X` is an immediate parent of `Y`.
    /// - `X` is an ancestor of an immediate parent of `Y` (defined
    /// recursively).
    fn is_ancestor(
        &self,
        ancestor: &Self::Node,
        descendant: &Self::Node,
    ) -> Result<bool, Self::Error> {
        Ok(self.ancestors(descendant)?.contains(ancestor))
    }

    /// Get every node `X` in the [`BasicSourceControlGraph::universe`] such
    /// that `X == node` or there exists a child of `X` that is an ancestor of
    /// `node`.
    fn ancestors(&self, node: &Self::Node) -> Result<search::NodeSet<Self::Node>, Self::Error>;

    /// Get the union of `ancestors(node)` for every node in `nodes`.
    #[instrument]
    fn ancestors_all(
        &self,
        nodes: search::NodeSet<Self::Node>,
    ) -> Result<search::NodeSet<Self::Node>, Self::Error> {
        let result = nodes
            .into_iter()
            .map(|node| self.ancestors(node))
            .try_fold(search::NodeSet::default(), |acc, set| Ok(acc.union(set?)))?;
        Ok(result)
    }

    /// Filter `nodes` to only include nodes that are not ancestors of any other
    /// node in `nodes`.
    fn ancestor_heads(
        &self,
        nodes: search::NodeSet<Self::Node>,
    ) -> Result<search::NodeSet<Self::Node>, Self::Error> {
        let node_to_ancestors: Vec<(Self::Node, search::NodeSet<Self::Node>)> = nodes
            .iter()
            .map(|node| Ok((node.clone(), self.ancestors(node)?)))
            .try_collect()?;
        let heads: search::NodeSet<Self::Node> = nodes
            .into_iter()
            .filter(|node| {
                node_to_ancestors
                    .iter()
                    .filter_map(|(other_node, ancestors)| {
                        if *node == other_node {
                            None
                        } else {
                            Some(ancestors)
                        }
                    })
                    .all(|ancestors| !ancestors.contains(node))
            })
            .cloned()
            .collect();
        Ok(heads)
    }

    /// Get every node `X` in the [`BasicSourceControlGraph::universe`] such
    /// that `X == node` or there exists a parent of `X` that is a descendant of
    /// `node`.
    fn descendants(&self, node: &Self::Node) -> Result<search::NodeSet<Self::Node>, Self::Error>;

    /// Filter `nodes` to only include nodes that are not descendants of any
    /// other node in `nodes`.
    fn descendant_roots(
        &self,
        nodes: search::NodeSet<Self::Node>,
    ) -> Result<search::NodeSet<Self::Node>, Self::Error> {
        let node_to_descendants: Vec<(Self::Node, search::NodeSet<Self::Node>)> = nodes
            .iter()
            .map(|node| Ok((node.clone(), self.descendants(node)?)))
            .collect::<Result<_, _>>()?;
        let roots: search::NodeSet<Self::Node> = nodes
            .into_iter()
            .filter(|node| {
                node_to_descendants
                    .iter()
                    .filter_map(|(other_node, descendants)| {
                        if *node == other_node {
                            None
                        } else {
                            Some(descendants)
                        }
                    })
                    .all(|descendants| !descendants.contains(node))
            })
            .cloned()
            .collect();
        Ok(roots)
    }

    /// Get the union of `descendants(node)` for every node in `nodes`.
    #[instrument]
    fn descendants_all(
        &self,
        nodes: search::NodeSet<Self::Node>,
    ) -> Result<search::NodeSet<Self::Node>, Self::Error> {
        let descendants = nodes
            .into_iter()
            .map(|node| self.descendants(node))
            .try_fold(search::NodeSet::default(), |acc, set| Ok(acc.union(set?)))?;
        Ok(descendants)
    }
}

impl<T: BasicSourceControlGraph> search::Graph for T {
    type Node = <Self as BasicSourceControlGraph>::Node;
    type Error = <Self as BasicSourceControlGraph>::Error;

    fn is_ancestor(
        &self,
        ancestor: &Self::Node,
        descendant: &Self::Node,
    ) -> Result<bool, Self::Error> {
        self.is_ancestor(ancestor, descendant)
    }

    fn add_success_bound(
        &self,
        nodes: search::NodeSet<Self::Node>,
        node: &Self::Node,
    ) -> Result<search::NodeSet<Self::Node>, Self::Error> {
        for success_node in nodes.iter() {
            if self.is_ancestor(node, success_node)? {
                return Ok(nodes);
            }
        }
        self.ancestor_heads(nodes.insert(node.clone()))
    }

    fn add_failure_bound(
        &self,
        nodes: search::NodeSet<Self::Node>,
        node: &Self::Node,
    ) -> Result<search::NodeSet<Self::Node>, Self::Error> {
        for failure_node in nodes.iter() {
            if self.is_ancestor(failure_node, node)? {
                return Ok(nodes);
            }
        }
        self.descendant_roots(nodes.insert(node.clone()))
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
    /// <https://git-scm.com/docs/git-bisect-lk2009#_bisection_algorithm_discussed>
    /// discusses a metric to find the best partition for the subgraph which
    /// remains to be tested.
    ///
    /// See also `git-bisect`'s skip algorithm:
    /// <https://git-scm.com/docs/git-bisect-lk2009#_skip_algorithm>. We do
    /// *not* use the same skip algorithm, and instead uses a deterministic
    /// approach. In order to solve the following problem:
    ///
    /// > "sometimes the best bisection points all happened to be in an area
    /// where all the commits are untestable. And in this case the user was
    /// asked to test many untestable commits, which could be very inefficient."
    ///
    /// We instead speculate on the hypothetical success and failure cases, and
    /// interleave the resulting search nodes.
    ///
    /// Resources:
    ///
    /// - <https://git-scm.com/docs/git-bisect-lk2009#_bisection_algorithm_discussed>
    /// - <https://byorgey.wordpress.com/2023/01/01/competitive-programming-in-haskell-better-binary-search/>
    /// - <https://julesjacobs.com/notes/binarysearch/binarysearch.pdf>
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

    fn midpoints(
        &self,
        graph: &G,
        bounds: &search::Bounds<G::Node>,
        statuses: &IndexMap<G::Node, search::Status>,
    ) -> Result<Vec<G::Node>, G::Error> {
        let search::Bounds {
            success: success_bounds,
            failure: failure_bounds,
        } = bounds;
        let nodes_to_search = {
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
        match self.strategy {
            BasicStrategyKind::Linear => {
                let remaining_nodes = graph.difference(
                    graph.universe()?,
                    graph.ancestors_all(success_bounds.clone())?,
                )?;
                let roots = graph.descendant_roots(remaining_nodes)?;
                let roots = graph.sort(roots.into_iter().cloned())?;
                Ok(roots)
            }
            BasicStrategyKind::LinearReverse => {
                let remaining_nodes = graph.difference(
                    graph.universe()?,
                    graph.descendants_all(failure_bounds.clone())?,
                )?;
                let heads = graph.ancestor_heads(remaining_nodes)?;
                let heads = graph.sort(heads.into_iter().cloned())?;
                Ok(heads)
            }
            BasicStrategyKind::Binary => {
                let search_degrees: Vec<(G::Node, usize)> = nodes_to_search
                    .into_iter()
                    .map(|node| {
                        let degree = search_degree(graph, bounds, &node)?;
                        Ok((node, degree))
                    })
                    .try_collect()?;
                let max_degree = search_degrees
                    .iter()
                    .map(|(_, degree)| degree)
                    .copied()
                    .max();
                match max_degree {
                    Some(max_degree) => {
                        let max_degree_nodes = search_degrees
                            .into_iter()
                            .filter(|(_, degree)| *degree == max_degree)
                            .map(|(node, _)| node)
                            .collect_vec();
                        let max_degree_nodes = graph.sort(max_degree_nodes)?;
                        Ok(max_degree_nodes)
                    }
                    None => Ok(Default::default()),
                }
            }
        }
    }
}

/// Calculate the "search degree" of a node for binary search. The "search
/// degree" of a node is the minimum number of nodes that could be excluded from
/// the next step of the binary search by testing that node.
///
/// FIXME: Performs a call to
/// [`BasicSourceControlGraph::ancestors`]/[`BasicSourceControlGraph::descendants`]
/// for each node, resulting in O(n^2) complexity when called on each node in
/// the search range. This could be improved by walking the whole graph and
/// keeping track of degree rather than recomputing common information for each
/// node.
///
/// FIXME: Does not take into account nodes that returned
/// [`search::Status::Indeterminate`].
fn search_degree<G: BasicSourceControlGraph>(
    graph: &G,
    bounds: &search::Bounds<G::Node>,
    node: &G::Node,
) -> Result<usize, G::Error> {
    let ancestors = graph.ancestors(node)?;
    let success_ancestors = graph.ancestors_all(bounds.success.clone())?;
    let remaining_ancestors = graph.difference(ancestors, success_ancestors)?;

    let descendants = graph.descendants(node)?;
    let failure_descendants = graph.descendants_all(bounds.failure.clone())?;
    let remaining_descendants = graph.difference(descendants, failure_descendants)?;

    let degree = min(remaining_ancestors.size(), remaining_descendants.size());
    Ok(degree)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::search::Bounds;
    use crate::search::EagerSolution;
    use crate::search::NodeSet;
    use crate::search::NodeSetExt;
    use crate::search::Search;
    use crate::search::Status;
    use crate::testing::arb_strategy;
    use crate::testing::arb_test_graph_and_nodes;
    use crate::testing::TestGraph;
    use crate::testing::UsizeGraph;

    use itertools::Itertools;
    use maplit::hashmap;

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
        let mut search = Search::new_with_nodes(graph.clone(), nodes.clone());

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
                    success: [2].into_iter().collect(),
                    failure: [].into_iter().collect(),
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
                    success: [2].into_iter().collect(),
                    failure: [].into_iter().collect(),
                },
                next_to_search: vec![4, 5, 3, 6],
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
                    success: [2].into_iter().collect(),
                    failure: [5].into_iter().collect(),
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
                    success: [2].into_iter().collect(),
                    failure: [5].into_iter().collect(),
                },
                next_to_search: vec![3, 4],
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
                    success: [2].into_iter().collect(),
                    failure: [5].into_iter().collect(),
                },
                next_to_search: vec![4],
            }
        );
    }

    #[test]
    fn test_search_inconsistent_notify() {
        let graph = UsizeGraph { max: 7 };
        let nodes = 0..graph.max;
        let mut search = Search::new_with_nodes(graph, nodes);

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

    #[test]
    fn test_search_dag() {
        let graph = TestGraph {
            // a -> b -> e -> f -> g
            // c -> d ->   -> h
            nodes: hashmap! {
                'a' =>  ['b'].into_iter().collect(),
                'b' =>  ['e'].into_iter().collect(),
                'c' =>  ['d'].into_iter().collect(),
                'd' =>  ['e'].into_iter().collect(),
                'e' =>  ['f', 'h'].into_iter().collect(),
                'f' =>  ['g'].into_iter().collect(),
                'g' =>  [].into_iter().collect(),
                'h' =>  [].into_iter().collect(),
            },
        };
        let linear_strategy = BasicStrategy {
            strategy: BasicStrategyKind::Linear,
        };
        assert_eq!(
            graph.descendants(&'e'),
            Ok(['e', 'f', 'g', 'h'].into_iter().collect())
        );
        assert_eq!(
            graph.ancestors(&'e'),
            Ok(['a', 'b', 'c', 'd', 'e'].into_iter().collect())
        );

        let mut search = Search::new_with_nodes(graph, 'a'..='h');
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
                    success: ['b'].into_iter().collect(),
                    failure: ['g'].into_iter().collect(),
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
                    success: ['e'].into_iter().collect(),
                    failure: ['g'].into_iter().collect(),
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
                    success: ['f'].into_iter().collect(),
                    failure: ['g'].into_iter().collect(),
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
                    success: ['f', 'h'].into_iter().collect(),
                    failure: ['g'].into_iter().collect(),
                },
                next_to_search: vec![],
            }
        );
    }

    proptest::proptest! {
        #[test]
        fn test_search_dag_proptest(strategy in arb_strategy(), (graph, failure_nodes) in arb_test_graph_and_nodes(8)) {
            let nodes = graph.nodes.keys().sorted().copied().collect::<Vec<_>>();
            let strategy = BasicStrategy {
                strategy,
            };
            let mut search = Search::new_with_nodes(graph.clone(), nodes);
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

            let nodes = graph.nodes.keys().copied().collect::<NodeSet<_>>();
            assert!(solution.bounds.success.is_subset(&nodes));
            assert!(solution.bounds.failure.is_subset(&nodes));
            assert!(solution.bounds.success.is_disjoint(&solution.bounds.failure));
            let all_success_nodes = graph.ancestors_all(solution.bounds.success.clone()).unwrap();
            let all_failure_nodes = graph.descendants_all(solution.bounds.failure).unwrap();
            assert!(all_success_nodes.is_disjoint(&all_failure_nodes));
            assert!(
                all_success_nodes.clone().union(all_failure_nodes.clone()) == nodes,
                "all_success_nodes: {all_success_nodes:?}, all_failure_nodes: {all_failure_nodes:?}, nodes: {nodes:?}",
            );
        }
    }
}
