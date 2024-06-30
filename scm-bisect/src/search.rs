//! A search algorithm for directed acyclic graphs to find the nodes which
//! "flip" from passing to failing a predicate.

use std::collections::{HashSet, VecDeque};
use std::fmt::Debug;
use std::hash::Hash;

use indexmap::IndexMap;
use tracing::{debug, instrument};

/// A set of nodes.
pub type NodeSet<Node> = rpds::HashTrieSetSync<Node>;

/// Extension methods for [`NodeSet`].
pub trait NodeSetExt {
    /// Return the union of this set with the `other` set.
    fn union(self, other: Self) -> Self;
}

impl<Node: Clone + Eq + Hash> NodeSetExt for NodeSet<Node> {
    fn union(mut self, other: Self) -> Self {
        if self.is_empty() {
            other
        } else if other.is_empty() {
            self
        } else {
            for node in other.into_iter() {
                self.insert_mut(node.clone());
            }
            self
        }
    }
}

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
    ///   recursively).
    fn is_ancestor(
        &self,
        ancestor: &Self::Node,
        descendant: &Self::Node,
    ) -> Result<bool, Self::Error>;

    /// Filter `nodes` to only include nodes that are not ancestors of any other
    /// node in `nodes`. This is not strictly necessary, but it improves
    /// performance as some operations are linear in the size of the success
    /// bounds, and it can make the intermediate results more sensible.
    ///
    /// This operation is called `heads` in e.g. Mercurial. @nocommit no longer accurate
    #[instrument]
    fn add_success_bound(
        &self,
        nodes: NodeSet<Self::Node>,
        node: &Self::Node,
    ) -> Result<NodeSet<Self::Node>, Self::Error> {
        Ok(nodes.insert(node.clone()))
    }

    /// Filter `nodes` to only include nodes that are not descendants of any
    /// other node in `nodes`. This is not strictly necessary, but it improves
    /// performance as some operations are linear in the size of the failure
    /// bounds, and it can make the intermediate results more sensible.
    ///
    /// This operation is called `roots` in e.g. Mercurial. @nocommit no longer accurate
    #[instrument]
    fn add_failure_bound(
        &self,
        nodes: NodeSet<Self::Node>,
        node: &Self::Node,
    ) -> Result<NodeSet<Self::Node>, Self::Error> {
        Ok(nodes.insert(node.clone()))
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
    pub success: NodeSet<Node>,

    /// The lower bounds of the search. The ancestors of this set have (or are
    /// assumed to have) `Status::Failure`.
    pub failure: NodeSet<Node>,
}

impl<Node: Debug + Eq + Hash> Default for Bounds<Node> {
    fn default() -> Self {
        Bounds {
            success: Default::default(),
            failure: Default::default(),
        }
    }
}

impl<Node: Debug + Eq + Hash> Clone for Bounds<Node> {
    fn clone(&self) -> Self {
        let Self { success, failure } = self;
        Self {
            success: success.clone(),
            failure: failure.clone(),
        }
    }
}

/// A search strategy to select the next node to search in the graph.
pub trait Strategy<G: Graph>: Debug {
    /// An error type.
    type Error: std::error::Error;

    /// Return a set of **midpoints** for the search. The returned values become
    /// potential next nodes to test.
    ///
    /// A midpoint "lies between" the success bounds and failure bounds, for
    /// some meaning of "lie between". The definition of "midpoint" is the key
    /// characteristic of the search strategy.
    ///
    /// - Example: Linear search on a [`Vec`] would try to return a child node
    ///   of a node in the provided [`Bounds::success`].
    /// - Example: Binary search on a [`Vec`] would try to return a node in the
    ///   "middle" of the provided [`Bounds::success`] and [`Bounds::failure`],
    ///   such that the returned value has a roughly equal number of untested
    ///   ancestor nodes vs untested descendant nodes.
    ///
    /// ## Parameters
    ///
    /// - `graph`: The graph to search in.
    /// - `bounds`: The current bounds of the search. The returned midpoint
    ///   should lie between [`Bounds::success`] and [`Bounds::failure`].
    /// - `statuses`: The results of all nodes that have been tested (by
    ///   [`Search::notify`]) so far.
    ///   - Every node which has been tested will appear in this map.
    ///   - However, not every node in the graph will have a status in this map
    ///     (which may be prohibitive if the graph is large). A node without a
    ///     status should be treated logically the same as being
    ///     [`Status::Untested`].
    ///   - The implementor may treat the set of nodes in `statuses` as an
    ///     additional set of search bounds, if desired. (This most likely depends
    ///     on whether the implementor uses [`Search::new_with_nodes`] to
    ///     initialize the [`Search`]).
    ///
    /// ## Return
    ///
    /// Returns a list of equally-"good" midpoints between the search bounds,
    /// or the empty list when the search should exit.
    ///
    /// - Example: searching a graph may produce multiple unorderable midpoint
    ///   nodes that split the search space equally well, in which case all
    ///   midpoint nodes can be returned.
    /// - For ease of implementation, the implementor could choose to just find
    ///   and return the first "good" midpoint (or the empty list if no midpoints
    ///   exist).
    /// - The implementor should arrange for the search to exit when the bounds
    ///   are maximally "tight". That is, when no choice of return value, after
    ///   being tested, could change the bounds.
    ///
    /// The return value must not include any nodes that are an ancestor of the
    /// provided [`Bounds::success`] or a descendant of the provided
    /// [`Bounds::failure`], or any nodes that are already present in the
    /// provided [`statuses`], since then you would search it again in a loop
    /// indefinitely.
    ///
    /// NOTE: This function should be deterministic. The [`Search`] may call it
    /// multiple times with inconsistent arguments as part of parallel
    /// speculative search.
    ///
    /// - Example: A status in `statuses` may undergo a normally-illegal state
    ///   transition between subsequent calls, such as from [`Status::Success`] to
    ///   [`Status::Failure`].
    ///
    /// NOTE: The returned value does not need to be present in `statuses` (not
    /// even as [`Status::Untested`]).
    ///
    /// NOTE: The implementor must decide what the behavior of the strategy is
    /// when [`Bounds::success`] and/or [`Bounds::failure`] are empty:
    ///
    /// - One option is for the caller to always initialize the [`Search`] with
    ///   some number of nodes via the [`Search::new_with_nodes`] constructor, and
    ///   use that set of nodes as implicit bounds on the graph. Then, the
    ///   midpoint can lie between those bounds without having to evaluate the
    ///   full search graph.
    fn midpoints(
        &self,
        graph: &G,
        bounds: &Bounds<G::Node>,
        statuses: &IndexMap<G::Node, Status>,
    ) -> Result<Vec<G::Node>, Self::Error>;
}

/// The results of the search so far. The search is complete if `next_to_search` is empty.
pub struct LazySolution<'a, TNode: Debug + Eq + Hash + 'a, TError> {
    /// The bounds of the search so far.
    pub bounds: &'a Bounds<TNode>,

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
            bounds: bounds.clone(),
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
pub enum SearchError<TNode: Debug + Eq + Hash, TGraphError, TStrategyError> {
    #[error("node {node:?} has already been bounded as {bound:?} by {bound_node:?} in (potentially-speculative) bounds {bounds:?}, but it was returned as a new midpoint to search by the search strategy; this would loop indefinitely")]
    AlreadySearchedMidpoint {
        node: TNode,
        bound_node: TNode,
        bound: Status,
        bounds: Bounds<TNode>,
    },

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
    statuses: IndexMap<G::Node, Status>,
    bounds: Bounds<G::Node>,
}

impl<G: Graph> Search<G> {
    /// Construct a new search. The provided `graph` represents the universe of
    /// all nodes as a directed acyclic graph.
    pub fn new(graph: G) -> Self {
        Self {
            graph,
            statuses: Default::default(),
            bounds: Default::default(),
        }
    }

    /// Construct a new search. The provided `graph` represents the universe of
    /// all nodes, and `initial_nodes` represents a subset of that universe to
    /// search in.
    ///
    /// The provided `initial_nodes` set is just a convenience parameter
    /// equivalent to calling `Search::notify(node, Status::Untested)` for each
    /// `node` in the set. It's oftentimes easier to implement
    /// [`Strategy::midpoints`] if the input set of `statuses` is non-empty.
    ///
    /// For example, if `graph` corresponds to the source control graph, then
    /// `nodes` might correspond to a recent range of commits where the first
    /// one is passing and the last one is failing.
    pub fn new_with_nodes(graph: G, initial_nodes: impl IntoIterator<Item = G::Node>) -> Self {
        let nodes = initial_nodes
            .into_iter()
            .map(|node| (node, Status::Untested))
            .collect();
        Self {
            graph,
            statuses: nodes,
            bounds: Default::default(),
        }
    }

    /// Summarize the current search progress and suggest the next node(s) to
    /// search. The caller is responsible for calling `notify` with the result.
    #[instrument]
    #[allow(clippy::type_complexity)]
    pub fn search<'a, S: Strategy<G>>(
        &'a self,
        strategy: &'a S,
    ) -> Result<
        LazySolution<'a, G::Node, SearchError<G::Node, G::Error, S::Error>>,
        SearchError<G::Node, G::Error, S::Error>,
    > {
        let initial_state = SearchState {
            next_node: None,
            bounds: self.bounds.clone(),
            statuses: self.statuses.clone(),
        };
        let iter = SearchIter {
            graph: &self.graph,
            strategy,
            seen: Default::default(),
            states: [initial_state].into_iter().collect(),
        };

        Ok(LazySolution {
            bounds: &self.bounds,
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
        match self.statuses.get(&node) {
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

        let bounds = match status {
            Status::Untested | Status::Indeterminate => self.bounds.clone(),

            Status::Success => {
                for failure_node in self.bounds.failure.iter() {
                    if self
                        .graph
                        .is_ancestor(failure_node, &node)
                        .map_err(NotifyError::Graph)?
                    {
                        return Err(NotifyError::InconsistentStateTransition {
                            ancestor_node: failure_node.clone(),
                            ancestor_status: Status::Failure,
                            descendant_node: node,
                            descendant_status: Status::Success,
                        });
                    }
                }

                let Bounds { success, failure } = self.bounds.clone();
                Bounds {
                    success: self
                        .graph
                        .add_success_bound(success, &node)
                        .map_err(NotifyError::Graph)?,
                    failure,
                }
            }

            Status::Failure => {
                for success_node in self.bounds.success.iter() {
                    if self
                        .graph
                        .is_ancestor(&node, success_node)
                        .map_err(NotifyError::Graph)?
                    {
                        return Err(NotifyError::InconsistentStateTransition {
                            ancestor_node: node,
                            ancestor_status: Status::Failure,
                            descendant_node: success_node.clone(),
                            descendant_status: Status::Success,
                        });
                    }
                }

                let Bounds { success, failure } = self.bounds.clone();
                Bounds {
                    success,
                    failure: self
                        .graph
                        .add_failure_bound(failure, &node)
                        .map_err(NotifyError::Graph)?,
                }
            }
        };

        self.statuses.insert(node, status);
        self.bounds = bounds;
        Ok(())
    }
}

/// An intermediate search state for [`SearchIter`]. This may correspond to the
/// actual state of the search, or a speculative state of the search supposing
/// that the test for a given node passed/failed.
#[derive(Debug)]
struct SearchState<G: Graph> {
    /// - If [`Some`], then the search yields this node next.
    /// - If [`None`], then the next node(s) are calculated from the
    ///   [`Strategy::midpoints`] of the [`SearchState::bounds`], and any
    ///   speculative search states are also queued.
    next_node: Option<G::Node>,

    /// The bounds at this point in the search.
    bounds: Bounds<G::Node>,

    /// The results of testing so far.
    statuses: IndexMap<G::Node, Status>,
}

/// An iterator that yields the proposed next nodes to search. This starts with
/// the current midpoints of the success and failure bounds, and then starts
/// *speculating*: it yields the next nodes to search in the hypothetical cases
/// that a previously-yielded node succeeded/failed.
struct SearchIter<'a, G: Graph, S: Strategy<G>> {
    graph: &'a G,
    strategy: &'a S,

    /// The set of already-yielded nodes. These won't be yielded again.
    seen: HashSet<G::Node>,

    /// The set of [`SearchState`]s in the queue, used to yield the nxt nodes.
    states: VecDeque<SearchState<G>>,
}

impl<G: Graph, S: Strategy<G>> SearchIter<'_, G, S> {
    /// Returns [`Err`] if the provided node has already been searched. This
    /// should only happen if the caller's [`Strategy::midpoints`] yields a node
    /// that's already in `statuses`, which it shouldn't do.
    #[allow(clippy::type_complexity)]
    fn check_node_not_searched(
        &self,
        node: &G::Node,
        bounds: &Bounds<G::Node>,
    ) -> Result<(), SearchError<G::Node, G::Error, S::Error>> {
        let Bounds { success, failure } = bounds;
        for success_node in success.iter() {
            match self.graph.is_ancestor(node, success_node) {
                Ok(false) => {}
                Ok(true) => {
                    return Err(SearchError::AlreadySearchedMidpoint {
                        node: node.clone(),
                        bound_node: success_node.clone(),
                        bound: Status::Success,
                        bounds: bounds.clone(),
                    });
                }
                Err(err) => return Err(SearchError::Graph(err)),
            }
        }
        for failure_node in failure.iter() {
            match self.graph.is_ancestor(failure_node, node) {
                Ok(false) => {}
                Ok(true) => {
                    return Err(SearchError::AlreadySearchedMidpoint {
                        node: node.clone(),
                        bound_node: failure_node.clone(),
                        bound: Status::Failure,
                        bounds: bounds.clone(),
                    });
                }
                Err(err) => return Err(SearchError::Graph(err)),
            }
        }
        Ok(())
    }

    /// Return a set of speculative states corresponding to the situations where
    /// the `node`` passed and failed.
    #[allow(clippy::type_complexity)]
    fn speculate_node(
        &mut self,
        node: &G::Node,
        bounds: &Bounds<G::Node>,
        statuses: &IndexMap<G::Node, Status>,
    ) -> Result<[SearchState<G>; 2], SearchError<G::Node, G::Error, S::Error>> {
        let Bounds { success, failure } = bounds;

        let speculate_failure_state = SearchState {
            next_node: None,
            bounds: Bounds {
                success: success.clone(),
                failure: self
                    .graph
                    .add_failure_bound(failure.clone(), node)
                    .map_err(SearchError::Graph)?,
            },
            statuses: {
                let mut statuses = statuses.clone();
                statuses.insert(node.clone(), Status::Failure);
                statuses
            },
        };

        let speculate_success_state = SearchState {
            next_node: None,
            bounds: Bounds {
                success: self
                    .graph
                    .add_success_bound(success.clone(), node)
                    .map_err(SearchError::Graph)?,
                failure: failure.clone(),
            },
            statuses: {
                let mut statuses = statuses.clone();
                statuses.insert(node.clone(), Status::Success);
                statuses
            },
        };

        Ok([speculate_failure_state, speculate_success_state])
    }
}

impl<G: Graph, S: Strategy<G>> Iterator for SearchIter<'_, G, S> {
    type Item = Result<G::Node, SearchError<G::Node, G::Error, S::Error>>;

    /// FIXME: Each call to `next` can do O(n) work due to cloning graph
    /// traversal data structures. (This could be fixed with some form of
    /// persistent data structures.)
    fn next(&mut self) -> Option<Self::Item> {
        while let Some(state) = self.states.pop_front() {
            debug!(?state, "Popped speculation state");
            let SearchState {
                next_node,
                bounds,
                statuses,
            } = state;

            let next_node = match next_node {
                Some(next_node) => next_node,
                None => {
                    let next_nodes = match self.strategy.midpoints(self.graph, &bounds, &statuses) {
                        Ok(nodes) => nodes,
                        Err(err) => return Some(Err(SearchError::Strategy(err))),
                    };
                    self.states
                        .extend(next_nodes.into_iter().map(|next_node| SearchState {
                            next_node: Some(next_node),
                            bounds: bounds.clone(),
                            statuses: statuses.clone(),
                        }));
                    continue;
                }
            };

            match self.check_node_not_searched(&next_node, &bounds) {
                Ok(()) => {}
                Err(err) => {
                    debug!(
                        ?err,
                        "skipping node returned from `midpoints` as it was already excluded"
                    );
                    continue;
                }
            }
            match self.speculate_node(&next_node, &bounds, &statuses) {
                Ok(states) => {
                    self.states.extend(states);
                }
                Err(err) => {
                    return Some(Err(err));
                }
            }

            if self.seen.insert(next_node.clone()) {
                return Some(Ok(next_node));
            }
        }
        None
    }
}
