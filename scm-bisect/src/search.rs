//! A search algorithm for directed acyclic graphs to find the nodes which
//! "flip" from passing to failing a predicate.

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::hash::Hash;

use indexmap::IndexMap;
use itertools::{EitherOrBoth, Itertools};
use tracing::instrument;

/// The set of nodes compromising a directed acyclic graph to be searched.
pub trait Graph: Debug {
    /// The type of nodes in the graph. This should be cheap to clone.
    type Node: Clone + Debug + Hash + Eq;

    /// An error type.
    type Error: std::error::Error;

    /// Return whether or not `node` is an ancestor of `descendant`.
    fn is_ancestor(
        &self,
        ancestor: Self::Node,
        descendant: Self::Node,
    ) -> Result<bool, Self::Error>;

    /// Filter `nodes` to only include nodes that are not ancestors of any other
    /// node in `nodes`. This is not strictly necessary, but it makes the
    /// intermediate results more sensible.
    #[instrument]
    fn simplify_success_bounds(
        &self,
        nodes: HashSet<Self::Node>,
    ) -> Result<HashSet<Self::Node>, Self::Error> {
        Ok(nodes)
    }

    /// Filter `nodes` to only include nodes that are not descendants of any
    /// other node in `nodes`. This is not strictly necessary, but it makes the
    /// intermediate results more sensible.
    #[instrument]
    fn simplify_failure_bounds(
        &self,
        nodes: HashSet<Self::Node>,
    ) -> Result<HashSet<Self::Node>, Self::Error> {
        Ok(nodes)
    }
}

/// The possible statuses of a node in the search.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Status {
    /// The node has not been tested yet. This is the starting state for each node in a search.
    Untested,

    /// The node has been tested and satisfies some caller-defined predicate.
    /// For the rest of the search, it's assumed that all ancestor nodes of this
    /// node also satisfy the predicate.
    Success,

    /// The node has been tested and does not satisfy some caller-defined
    /// predicate. For the rest of the search, it's assumed that all descendant
    /// nodes of this node also do not satisfy the predicate.
    Failure,

    /// The node has been tested, but it is not known whether it satisfies some caller-defined
    /// predicate. It will be skipped in future searches.
    Indeterminate,
}

/// The upper and lower bounds of the search.
#[derive(Debug, Eq, PartialEq)]
pub struct Bounds<Node: Debug + Eq + Hash> {
    /// The upper bounds of the search. The ancestors of this set have (or are
    /// assumed to have) `Status::Success`.
    pub success: HashSet<Node>,

    /// The lower bounds of the search. The ancestors of this set have (or are
    /// assumed to have) `Status::Failure`.
    pub failure: HashSet<Node>,
}

impl<Node: Debug + Eq + Hash> Default for Bounds<Node> {
    fn default() -> Self {
        Bounds {
            success: Default::default(),
            failure: Default::default(),
        }
    }
}

/// A search strategy to select the next node to search in the graph.
pub trait Strategy<G: Graph>: Debug {
    /// An error type.
    type Error: std::error::Error;

    /// Return "midpoints" for the search. Such a midpoint lies between the
    /// success bounds and failure bounds, for some meaning of "lie between",
    /// which depends on the strategy details.
    ///
    /// For example, linear search would return the node(s) immediately "after"
    /// the node(s) in `success_bounds`, while binary search would return the
    /// node(s) in the middle of `success_bounds` and `failure_bounds`.`
    fn midpoints(
        &self,
        graph: &G,
        success_bounds: &HashSet<G::Node>,
        failure_bounds: &HashSet<G::Node>,
        statuses: &IndexMap<G::Node, Status>,
    ) -> Result<Box<dyn Iterator<Item = G::Node>>, Self::Error>;
}

/// The results of the search so far. The search is complete if `next_to_search` is empty.
pub struct LazySolution<'a, Node: Debug + Eq + Hash + 'a> {
    /// The bounds of the search so far.
    pub bounds: Bounds<Node>,

    /// The next nodes to search in a suggested order. Normally, you would only
    /// consume the first node in this iterator and then call `Search::notify`
    /// with the result. However, if you want to parallelize or speculate on
    /// further nodes, you can consume more nodes from this iterator.
    ///
    /// This will be empty when the bounds are as tight as possible, i.e. the
    /// search is complete.
    pub next_to_search: Box<dyn Iterator<Item = Node> + 'a>,
}

impl<'a, Node: Debug + Eq + Hash + 'a> LazySolution<'a, Node> {
    /// Convenience function to call `EagerSolution::from` on this `LazySolution`.
    pub fn into_eager(self) -> EagerSolution<Node> {
        EagerSolution::from(self)
    }
}

/// A `LazySolution` with a `Vec<Node>` for `next_to_search`. This is primarily
/// for debugging.
#[derive(Debug, Eq, PartialEq)]
pub struct EagerSolution<Node: Debug + Hash + Eq> {
    pub(crate) bounds: Bounds<Node>,
    pub(crate) next_to_search: Vec<Node>,
}

impl<Node: Debug + Hash + Eq> From<LazySolution<'_, Node>> for EagerSolution<Node> {
    fn from(solution: LazySolution<Node>) -> Self {
        let LazySolution {
            bounds,
            next_to_search,
        } = solution;
        Self {
            bounds,
            next_to_search: next_to_search.collect(),
        }
    }
}

#[allow(missing_docs)]
#[derive(Debug, thiserror::Error)]
pub enum SearchError<TGraphError, TStrategyError> {
    #[error(transparent)]
    Graph(TGraphError),

    #[error(transparent)]
    Strategy(TStrategyError),
}

/// The error type for the search.
#[allow(missing_docs)]
#[derive(Debug, thiserror::Error)]
pub enum NotifyError<TNode, TGraphError> {
    #[error("inconsistent state transition: {ancestor_node:?} ({ancestor_status:?}) was marked as an ancestor of {descendant_node:?} ({descendant_status:?}")]
    InconsistentStateTransition {
        ancestor_node: TNode,
        ancestor_status: Status,
        descendant_node: TNode,
        descendant_status: Status,
    },

    #[error("illegal state transition for {node:?}: {from:?} -> {to:?}")]
    IllegalStateTransition {
        node: TNode,
        from: Status,
        to: Status,
    },

    #[error(transparent)]
    Graph(TGraphError),
}

/// The search algorithm.
#[derive(Clone, Debug)]
pub struct Search<G: Graph> {
    graph: G,
    nodes: IndexMap<G::Node, Status>,
}

impl<G: Graph> Search<G> {
    /// Construct a new search. The provided `graph` represents the universe of
    /// all nodes, and `nodes` represents a subset of that universe to search
    /// in. Only elements from `nodes` will be returned by `success_bounds` and
    /// `failure_bounds`.
    ///
    /// For example, `graph` might correspond to the entire source control
    /// directed acyclic graph, and `nodes` might correspond to a recent range
    /// of commits where the first one is passing and the last one is failing.
    pub fn new(graph: G, search_nodes: impl IntoIterator<Item = G::Node>) -> Self {
        let nodes = search_nodes
            .into_iter()
            .map(|node| (node, Status::Untested))
            .collect();
        Self { graph, nodes }
    }

    /// Get the currently known bounds on the success nodes.
    ///
    /// FIXME: O(n) complexity.
    #[instrument]
    pub fn success_bounds(&self) -> Result<HashSet<G::Node>, G::Error> {
        let success_nodes = self
            .nodes
            .iter()
            .filter_map(|(node, status)| match status {
                Status::Success => Some(node.clone()),
                Status::Untested | Status::Failure | Status::Indeterminate => None,
            })
            .collect::<HashSet<_>>();
        let success_bounds = self.graph.simplify_success_bounds(success_nodes)?;
        Ok(success_bounds)
    }

    /// Get the currently known bounds on the failure nodes.
    ///
    /// FIXME: O(n) complexity.
    #[instrument]
    pub fn failure_bounds(&self) -> Result<HashSet<G::Node>, G::Error> {
        let failure_nodes = self
            .nodes
            .iter()
            .filter_map(|(node, status)| match status {
                Status::Failure => Some(node.clone()),
                Status::Untested | Status::Success | Status::Indeterminate => None,
            })
            .collect::<HashSet<_>>();
        let failure_bounds = self.graph.simplify_failure_bounds(failure_nodes)?;
        Ok(failure_bounds)
    }

    /// Summarize the current search progress and suggest the next node(s) to
    /// search. The caller is responsible for calling `notify` with the result.
    #[instrument]
    #[allow(clippy::type_complexity)]
    pub fn search<S: Strategy<G>>(
        &self,
        strategy: &S,
    ) -> Result<LazySolution<G::Node>, SearchError<G::Error, S::Error>> {
        let success_bounds = self.success_bounds().map_err(SearchError::Graph)?;
        let failure_bounds = self.failure_bounds().map_err(SearchError::Graph)?;
        let next_to_search = strategy
            .midpoints(&self.graph, &success_bounds, &failure_bounds, &self.nodes)
            .map_err(SearchError::Strategy)?;
        Ok(LazySolution {
            bounds: Bounds {
                success: success_bounds,
                failure: failure_bounds,
            },
            next_to_search,
        })
    }

    /// Update the search state with the result of a search.
    #[instrument]
    pub fn notify(
        &mut self,
        node: G::Node,
        status: Status,
    ) -> Result<(), NotifyError<G::Node, G::Error>> {
        match self.nodes.get(&node) {
            Some(existing_status @ (Status::Success | Status::Failure))
                if existing_status != &status =>
            {
                return Err(NotifyError::IllegalStateTransition {
                    node,
                    from: *existing_status,
                    to: status,
                })
            }
            _ => {}
        }

        match status {
            Status::Untested | Status::Indeterminate => {}

            Status::Success => {
                for failure_node in self.failure_bounds().map_err(NotifyError::Graph)? {
                    if self
                        .graph
                        .is_ancestor(failure_node.clone(), node.clone())
                        .map_err(NotifyError::Graph)?
                    {
                        return Err(NotifyError::InconsistentStateTransition {
                            ancestor_node: failure_node,
                            ancestor_status: Status::Failure,
                            descendant_node: node,
                            descendant_status: Status::Success,
                        });
                    }
                }
            }

            Status::Failure => {
                for success_node in self.success_bounds().map_err(NotifyError::Graph)? {
                    if self
                        .graph
                        .is_ancestor(node.clone(), success_node.clone())
                        .map_err(NotifyError::Graph)?
                    {
                        return Err(NotifyError::InconsistentStateTransition {
                            ancestor_node: node,
                            ancestor_status: Status::Failure,
                            descendant_node: success_node,
                            descendant_status: Status::Success,
                        });
                    }
                }
            }
        }

        self.nodes.insert(node, status);
        Ok(())
    }
}

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

impl<T: BasicSourceControlGraph> Graph for T {
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

impl<G: BasicSourceControlGraph> Strategy<G> for BasicStrategy {
    type Error = G::Error;

    fn midpoints(
        &self,
        graph: &G,
        success_bounds: &HashSet<G::Node>,
        failure_bounds: &HashSet<G::Node>,
        statuses: &IndexMap<G::Node, Status>,
    ) -> Result<Box<dyn Iterator<Item = G::Node>>, Self::Error> {
        let nodes_to_search = {
            let implied_success_nodes = graph.ancestors_all(success_bounds.clone())?;
            let implied_failure_nodes = graph.descendants_all(failure_bounds.clone())?;
            statuses
                .iter()
                .filter_map(|(node, status)| match status {
                    Status::Untested => Some(node.clone()),
                    Status::Success | Status::Failure | Status::Indeterminate => None,
                })
                .filter(|node| {
                    !implied_success_nodes.contains(node) && !implied_failure_nodes.contains(node)
                })
                .collect::<Vec<_>>()
        };
        let next_to_search: Box<dyn Iterator<Item = G::Node>> = match self.strategy {
            BasicStrategyKind::Linear => Box::new(nodes_to_search.into_iter()),
            BasicStrategyKind::LinearReverse => Box::new(nodes_to_search.into_iter().rev()),
            BasicStrategyKind::Binary => Box::new(make_binary_search_iter(&nodes_to_search)),
        };
        Ok(next_to_search)
    }
}

fn make_binary_search_iter<T: Clone>(nodes: &[T]) -> impl Iterator<Item = T> {
    // FIXME: O(n^2) complexity.
    let mut result = vec![];
    let middle_index = nodes.len() / 2;
    if let Some(middle_node) = nodes.get(middle_index) {
        result.push(middle_node.clone());
        let left = make_binary_search_iter(&nodes[0..middle_index]);
        let right = make_binary_search_iter(&nodes[middle_index + 1..]);
        for item in left.zip_longest(right) {
            match item {
                EitherOrBoth::Both(lhs, rhs) => {
                    result.push(lhs);
                    result.push(rhs);
                }
                EitherOrBoth::Left(item) | EitherOrBoth::Right(item) => {
                    result.push(item);
                }
            }
        }
    }
    result.into_iter()
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
            search.search(&linear_strategy).unwrap().into_eager(),
            EagerSolution {
                bounds: Default::default(),
                next_to_search: vec![0, 1, 2, 3, 4, 5, 6],
            }
        );
        assert_eq!(
            search
                .search(&linear_reverse_strategy)
                .unwrap()
                .into_eager(),
            EagerSolution {
                bounds: Default::default(),
                next_to_search: vec![6, 5, 4, 3, 2, 1, 0],
            }
        );
        assert_eq!(
            search.search(&binary_strategy).unwrap().into_eager(),
            EagerSolution {
                bounds: Default::default(),
                next_to_search: vec![3, 1, 5, 0, 4, 2, 6],
            }
        );

        search.notify(2, Status::Success).unwrap();
        assert_eq!(
            search.search(&linear_strategy).unwrap().into_eager(),
            EagerSolution {
                bounds: Bounds {
                    success: hashset! {2},
                    failure: hashset! {},
                },
                next_to_search: vec![3, 4, 5, 6],
            }
        );
        assert_eq!(
            search.search(&binary_strategy).unwrap().into_eager(),
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
            search.search(&linear_strategy).unwrap().into_eager(),
            EagerSolution {
                bounds: Bounds {
                    success: hashset! {2},
                    failure: hashset! {5},
                },
                next_to_search: vec![3, 4],
            }
        );
        assert_eq!(
            search.search(&binary_strategy).unwrap().into_eager(),
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
            search.search(&binary_strategy).unwrap().into_eager(),
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
            search.search(&linear_strategy).unwrap().into_eager(),
            EagerSolution {
                bounds: Default::default(),
                next_to_search: vec!['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'],
            }
        );

        search.notify('b', Status::Success).unwrap();
        search.notify('g', Status::Failure).unwrap();
        assert_eq!(
            search.search(&linear_strategy).unwrap().into_eager(),
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
            search.search(&linear_strategy).unwrap().into_eager(),
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
            search.search(&linear_strategy).unwrap().into_eager(),
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
            search.search(&linear_strategy).unwrap().into_eager(),
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
                let solution = search.search(&strategy).unwrap().into_eager();
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
