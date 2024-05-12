//! A search algorithm for directed acyclic graphs to find the nodes which
//! "flip" from passing to failing a predicate.

use std::collections::{HashSet, VecDeque};
use std::fmt::Debug;
use std::hash::Hash;

use indexmap::IndexMap;
use tracing::{debug, instrument};

/// The set of nodes compromising a directed acyclic graph to be searched.
pub trait Graph: Debug {
    /// The type of nodes in the graph. This should be cheap to clone.
    type Node: Clone + Debug + Hash + Eq;

    /// An error type.
    type Error: std::error::Error;

    /// Return whether or not `node` is an ancestor of `descendant`. A node `X``
    /// is said to be an "ancestor" of node `Y` if one of the following is true:
    ///
    /// - `X == Y`
    /// - `X` is an immediate parent of `Y`.
    /// - `X` is an ancestor of an immediate parent of `Y` (defined
    /// recursively).
    fn is_ancestor(
        &self,
        ancestor: Self::Node,
        descendant: Self::Node,
    ) -> Result<bool, Self::Error>;

    /// Filter `nodes` to only include nodes that are not ancestors of any other
    /// node in `nodes`. This is not strictly necessary, but it improves
    /// performance as some operations are linear in the size of the success
    /// bounds, and it can make the intermediate results more sensible.
    ///
    /// This operation is called `heads` in e.g. Mercurial.
    #[instrument]
    fn simplify_success_bounds(
        &self,
        nodes: HashSet<Self::Node>,
    ) -> Result<HashSet<Self::Node>, Self::Error> {
        Ok(nodes)
    }

    /// Filter `nodes` to only include nodes that are not descendants of any
    /// other node in `nodes`. This is not strictly necessary, but it improves
    /// performance as some operations are linear in the size of the failure
    /// bounds, and it can make the intermediate results more sensible.
    ///
    /// This operation is called `roots` in e.g. Mercurial.
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

    /// Return a **midpoint** for the search. The returned value becomes a
    /// potential next node to test.
    ///
    /// A midpoint "lies between" the success bounds and failure bounds, for
    /// some meaning of "lie between". The definition of "midpoint" is the key
    /// characteristic of the search strategy.
    ///
    /// - Example: Linear search would try to return a child node of a node in
    /// `success_bounds`.
    /// - Example: Binary search would try to return a node in the "middle" of
    /// `success_bounds` and `failure_bounds`, such that the returned value has
    /// a roughly equal number of untested ancestor nodes vs untested descendant
    /// nodes.
    ///
    /// ## Parameters
    ///
    /// - `graph`: The graph to search in.
    /// - `bounds`: The current bounds of the search. The returned midpoint
    /// should lie between the bounds.
    /// - `statuses`: The results of all nodes that have been tested
    /// ([`Search::notify`]) so far.
    ///   - Every node which has been tested will appear in this map.
    ///   - However, not every node in the graph will have a status in this map
    ///   (which may be prohibitive if the graph is large). A node without a
    ///   status should be treated logically the same as being
    ///   `Status::Untested`.
    ///   - The implementor may treat the set of nodes in `statuses` as an
    ///   additional set of search bounds
    ///
    /// ## Return
    ///
    /// Returns a midpoint between the search bounds, or `None` when the search
    /// should exit.
    ///
    /// - The implementor should arrange for the search to exit when the bounds
    /// are maximally "tight". That is, when no choice of return value, after
    /// being tested, could change the bounds.
    ///
    /// NOTE: This function should be deterministic. The `Search` may call it
    /// multiple times with inconsistent arguments as part of parallel
    /// speculative search.
    ///
    /// - Example: A status in `statuses` may undergo a normally-illegal state
    /// transition between subsequent calls, such as from `Status::Success` to
    /// `Status::Failure`.
    ///
    /// NOTE: The returned value does not need to be present in `statuses` (not
    /// even as `Status::Untested`).
    ///
    /// NOTE: The implementor must decide what the behavior of the strategy is
    /// when one or both bounds are empty.
    ///
    /// - One option is for the caller to always initialize the `Search` with
    /// some number of nodes via the `Search::new_with_nodes` constructor, and
    /// use that set of nodes as implicit bounds on the graph.
    ///
    /// NOTE: This must not return a value that has already been excluded by the
    /// success or failure bounds, since then you would search it again in a
    /// loop indefinitely. In that case, you must return `None` instead.
    ///
    /// - This coincides with the condition that the node is either not present in
    /// `statuses` or is `Status::Untested.
    fn midpoint(
        &self,
        graph: &G,
        bounds: &Bounds<G::Node>,
        statuses: &IndexMap<G::Node, Status>,
    ) -> Result<Option<G::Node>, Self::Error>;
}

/// The results of the search so far. The search is complete if `next_to_search` is empty.
pub struct LazySolution<'a, TNode: Debug + Eq + Hash + 'a, TError> {
    /// The bounds of the search so far.
    pub bounds: Bounds<TNode>,

    /// The next nodes to search in a suggested order. Normally, you would only
    /// consume the first node in this iterator and then call `Search::notify`
    /// with the result. However, if you want to parallelize or speculate on
    /// further nodes, you can consume more nodes from this iterator.
    ///
    /// This will be empty when the bounds are as tight as possible, i.e. the
    /// search is complete.
    pub next_to_search: Box<dyn Iterator<Item = Result<TNode, TError>> + 'a>,
}

impl<'a, TNode: Debug + Eq + Hash + 'a, TError> LazySolution<'a, TNode, TError> {
    /// Convenience function to call `EagerSolution::from` on this `LazySolution`.
    pub fn into_eager(self) -> Result<EagerSolution<TNode>, TError> {
        let LazySolution {
            bounds,
            next_to_search,
        } = self;
        Ok(EagerSolution {
            bounds,
            next_to_search: next_to_search.collect::<Result<Vec<_>, TError>>()?,
        })
    }
}

/// Primarily for debugging. This is like `LazySolution` but with a `Vec<Node>`
/// for `next_to_search` instead of an iterator.
#[derive(Debug, Eq, PartialEq)]
pub struct EagerSolution<Node: Debug + Hash + Eq> {
    /// Same as [`LazySolution::bounds`].
    pub bounds: Bounds<Node>,

    /// Same as [`LazySolution::next_to_search`], but as a `Vec` instead of an `Iterator`.
    pub next_to_search: Vec<Node>,
}

/// The error type returned by [`Search::search`].
#[allow(missing_docs)]
#[derive(Debug, thiserror::Error)]
pub enum SearchError<TNode, TGraphError, TStrategyError> {
    #[error("node {node:?} has already been classified as a {status:?} node, but was returned as a new midpoint to search; this would loop indefinitely")]
    AlreadySearchedMidpoint { node: TNode, status: Status },

    #[error(transparent)]
    Graph(TGraphError),

    #[error(transparent)]
    Strategy(TStrategyError),
}

/// The error type returned by [`Search::notify`].
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
    /// all nodes as a directed acyclic graph.
    pub fn new(graph: G) -> Self {
        Self {
            graph,
            nodes: Default::default(),
        }
    }

    /// Construct a new search. The provided `graph` represents the universe of
    /// all nodes, and `initial_nodes` represents a subset of that universe to
    /// search in.
    ///
    /// The provided `initial_nodes` set is just a convenience parameter
    /// equivalent to calling `Search::notify(node, Status::Untested)` for each
    /// `node` in the set. It's oftentimes easier to implement
    /// `Strategy::midpoint` if the input set of `statuses` is non-empty.
    ///
    /// For example, if `graph` corresponds to the source control graph, then
    /// `nodes` might correspond to a recent range of commits where the first
    /// one is passing and the last one is failing.
    pub fn new_with_nodes(graph: G, initial_nodes: impl IntoIterator<Item = G::Node>) -> Self {
        let nodes = initial_nodes
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
    pub fn search<'a, S: Strategy<G>>(
        &'a self,
        strategy: &'a S,
    ) -> Result<
        LazySolution<G::Node, SearchError<G::Node, G::Error, S::Error>>,
        SearchError<G::Node, G::Error, S::Error>,
    > {
        let success_bounds = self.success_bounds().map_err(SearchError::Graph)?;
        let failure_bounds = self.failure_bounds().map_err(SearchError::Graph)?;

        #[derive(Debug)]
        struct State<G: Graph> {
            bounds: Bounds<G::Node>,
            statuses: IndexMap<G::Node, Status>,
        }

        struct Iter<'a, G: Graph, S: Strategy<G>> {
            graph: &'a G,
            strategy: &'a S,
            seen: HashSet<G::Node>,
            states: VecDeque<State<G>>,
        }

        impl<'a, G: Graph, S: Strategy<G>> Iterator for Iter<'a, G, S> {
            type Item = Result<G::Node, SearchError<G::Node, G::Error, S::Error>>;

            fn next(&mut self) -> Option<Self::Item> {
                while let Some(state) = self.states.pop_front() {
                    debug!(?state, "Popped speculation state");
                    let State { bounds, statuses } = state;

                    let node = match self.strategy.midpoint(self.graph, &bounds, &statuses) {
                        Ok(Some(node)) => node,
                        Ok(None) => continue,
                        Err(err) => return Some(Err(SearchError::Strategy(err))),
                    };

                    let Bounds { success, failure } = bounds;
                    for success_node in success.iter() {
                        match self.graph.is_ancestor(node.clone(), success_node.clone()) {
                            Ok(true) => {
                                return Some(Err(SearchError::AlreadySearchedMidpoint {
                                    node,
                                    status: Status::Success,
                                }));
                            }
                            Ok(false) => (),
                            Err(err) => return Some(Err(SearchError::Graph(err))),
                        }
                    }
                    for failure_node in failure.iter() {
                        match self.graph.is_ancestor(failure_node.clone(), node.clone()) {
                            Ok(true) => {
                                return Some(Err(SearchError::AlreadySearchedMidpoint {
                                    node,
                                    status: Status::Failure,
                                }));
                            }
                            Ok(false) => (),
                            Err(err) => return Some(Err(SearchError::Graph(err))),
                        }
                    }

                    // Speculate failure:
                    self.states.push_back(State {
                        bounds: Bounds {
                            success: success.clone(),
                            failure: {
                                let mut failure_bounds = failure.clone();
                                failure_bounds.insert(node.clone());
                                match self.graph.simplify_failure_bounds(failure_bounds) {
                                    Ok(bounds) => bounds,
                                    Err(err) => return Some(Err(SearchError::Graph(err))),
                                }
                            },
                        },
                        statuses: {
                            let mut statuses = statuses.clone();
                            statuses.insert(node.clone(), Status::Failure);
                            statuses
                        },
                    });

                    // Speculate success:
                    self.states.push_back(State {
                        bounds: Bounds {
                            success: {
                                let mut success_bounds = success.clone();
                                success_bounds.insert(node.clone());
                                match self.graph.simplify_success_bounds(success_bounds) {
                                    Ok(bounds) => bounds,
                                    Err(err) => return Some(Err(SearchError::Graph(err))),
                                }
                            },
                            failure: failure.clone(),
                        },
                        statuses: {
                            let mut statuses = statuses.clone();
                            statuses.insert(node.clone(), Status::Success);
                            statuses
                        },
                    });

                    if self.seen.insert(node.clone()) {
                        return Some(Ok(node));
                    }
                }
                None
            }
        }

        let initial_state = State {
            bounds: Bounds {
                success: success_bounds.clone(),
                failure: failure_bounds.clone(),
            },
            statuses: self.nodes.clone(),
        };
        let iter = Iter {
            graph: &self.graph,
            strategy,
            seen: Default::default(),
            states: [initial_state].into_iter().collect(),
        };

        Ok(LazySolution {
            bounds: Bounds {
                success: success_bounds,
                failure: failure_bounds,
            },
            next_to_search: Box::new(iter),
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
