//! A minimization algorithm to find a smallest subset of nodes that, when
//! combined, satisfies a certain predicate.
//!
//! Simple binary search (as implemented with
//! [`basic_search::StrategyKind::Binary`]) can find *one* problematic node, but
//! there are times when there are logically *multiple* nodes that are
//! problematic only in combination.
//!
//! ## Examples of minimization use-cases
//!
//! - In graphs with merge commits, oftentimes there is one problematic node
//! from each of two branches of the graph, that cause a bug only when combined,
//! and it can be annoying to track them both down. Minimization can be used to
//! find a minimal set of commits from both branches that causes the bug by
//! operating on the commit graph.
//!   - Note that this may or may not work well in practice depending on the
//!   nature of the changes. If the branches contain many conflicting changes, it
//!   may not be possible to merge subsets of the branches automatically for
//!   testing.
//!   - So called "rebase-merging" can alleviate this to some degree, in which
//!   any branch to be merged is first rebased on top of the target and the
//!   branch author manually resolves any merge conflicts. Then the merge
//!   operation is guaranteed to succeed (be a "fast-forward" merge, in Git
//!   terminology), and many merges of subsets of the two branches are likely to
//!   succeed as well in practice.
//! - A set of features or modes have poor test coverage when used in
//! combination and a bug was introduced at some point that only triggers when
//! both features/modes are enabled at the same time on a certain input.
//! Minimization can be used to find a minimal set of features/modes that
//! exposes the bug when used together.
//!   - In these cases, the caller will probably express the features/modes as
//!   (temporary) nodes in the commit graph, such as via changes to a test
//!   script, and then search the resulting commit graph.
//! - A continuous integration system may try to merge batches of commits
//! together ("merge queueing") after first validating that all checks pass on
//! each commit in isolation, and then validating that all checks pass on the
//! batch as a whole. Rarely, a check may fail for the batch as a whole but pass
//! for each individual commit in the batch. The continuous integration system
//! will want to proceed and merge a largest subset of the batch for which all
//! checks pass. Minimization can be used to find such a subset.
//!   - See
//!   [Keeping master green at scale](https://www.uber.com/blog/research/keeping-master-green-at-scale/)
//!   (Uber 2019) for details of such a system.
//!
//! ## Lifting the search from the base graph
//!
//! @nocommit: rename `Subset` to `Set`
//!
//! Generally speaking, the minimization of a set of nodes of type `T` can be
//! viewed as a kind of search on the [power set] of nodes of type `T`. In this
//! module, we refer to the underlying [`search::Graph`] as the "base graph" and
//! its nodes of type [`search::Graph::Node`] as "base nodes". The [`Minimize`]
//! wrapped around the base graph is like a [`search::Search`], but instead of
//! searching nodes of type [`search::Graph::Node`], it searches nodes of type
//! [`Subset<search::Graph::Node>`] instead. The edges indicate subset inclusion
//! rather than preserving the structure of the base graph. We can say that the
//! search is "lifted" from the base graph to the power set of the base graph. @nocommit revise description to specify the full structure of the minimization graph in a clear way
//!
//! [power set]: https://en.wikipedia.org/wiki/Power_set
//!
//! Given a base graph with `n` nodes, the minimization graph (the power set of
//! the base graph) has `2^n` nodes, ! so the minimization graph is
//! exponentially larger and an exhaustive search ! may be infeasible. A
//! locally-minimal set of failing nodes can be found in `O(n)` time, but this
//! is considerably worse than the `O(log(n))` time that we enjoy for simple
//! bisection.
//!
//! Therefore, the primary difficulty in searching a large base graph is the
//! development of heuristics ([`Strategy`]) to terminate search in a reasonable
//! amount of time, potentially at the expense of producing a suboptimal
//! answer (but one that we hope is still useful).
//!
//! Since this is still a search over a directed acyclic graph, we can reuse the
//! [`search::Search`] algorithms. The difference is in how we define the
//! "bisection" function, which produces the midpoint node(s) to search next. In
//! a normal source control graph, bisecting by finding a node with roughly equal
//! transitive in-degree and out-degree is a good choice. However, the graph
//! structure for the minimization is fully connected, so this choice of midpoint tends to not produce very much information
//! [`Subset<T>`], and should be "between" the lower and upper bounds of type
//! [`Subset<T>`] (a superset of the lower bound and a subset of the upper
//! bound). It may not be immediately obvious what kind of bisection

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt::{Debug, Formatter};
use std::hash::Hash;

use indexmap::IndexMap;
use tracing::instrument;

use crate::search::{self, NodeSet};

/// Helper trait to identify nodes that come from the "base graph" of the
/// minimization, rather than nodes of the minimization itself (namely,
/// [`Subset`]s of this type of node).
pub trait BaseGraphNode: Eq + Ord {}

impl<T: Eq + Ord> BaseGraphNode for T {}

/// @nocommit: use a more efficient frozen hash-set type.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct Subset<T: BaseGraphNode> {
    inner: BTreeSet<T>,
}

impl<T: BaseGraphNode> Debug for Subset<T>
where
    T: Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let Self { inner } = self;
        write!(f, "Subset {inner:?}")
    }
}

impl<T: BaseGraphNode> Subset<T> {
    /// Iterate over  the elements in this subset.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.inner.iter()
    }

    /// Returns `true` if the set contains no elements.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the number of elements in the set.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Add the provided node to this set. Returns the updated set.
    pub fn insert(self, node: T) -> Self {
        let Self { mut inner } = self;
        inner.insert(node);
        Self { inner }
    }

    /// Returns `true` if the set contains `node`.
    pub fn contains(&self, node: &T) -> bool {
        self.inner.contains(node)
    }

    /// Returns `true` if every element in this set is also in `other`.
    pub fn is_subset(&self, other: &Self) -> bool {
        self.inner.is_subset(&other.inner)
    }
}

impl<T: BaseGraphNode> Default for Subset<T> {
    fn default() -> Self {
        Self {
            inner: Default::default(),
        }
    }
}

impl<T: BaseGraphNode> PartialOrd for Subset<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self.is_subset(other), other.is_subset(self)) {
            (true, true) => Some(Ordering::Equal),
            (true, false) => Some(Ordering::Less),
            (false, true) => Some(Ordering::Greater),
            (false, false) => None,
        }
    }
}

impl<TBaseNode: Ord> FromIterator<TBaseNode> for Subset<TBaseNode> {
    fn from_iter<T: IntoIterator<Item = TBaseNode>>(iter: T) -> Self {
        Self {
            inner: iter.into_iter().collect(),
        }
    }
}

impl<TBaseNode: Ord> IntoIterator for Subset<TBaseNode> {
    type Item = TBaseNode;
    type IntoIter = <BTreeSet<TBaseNode> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}

type MinimizeGraphError<TBaseGraph> = <TBaseGraph as search::Graph>::Error;

type MinimizeSearchError<TBaseGraph> = search::SearchError<
    Subset<<TBaseGraph as search::Graph>::Node>,
    MinimizeGraphError<TBaseGraph>,
    StrategyError,
>;

type LazyMinimizeSolution<'a, TBaseGraph> = search::LazySolution<
    'a,
    Subset<<TBaseGraph as search::Graph>::Node>,
    MinimizeSearchError<TBaseGraph>,
>;

/// Wrapper around an existing [`search::Graph`]. Whereas the wrapped graph `G`
/// has nodes of type `G::Node`, this graph has nodes of type `Subset<G::Node>`.
#[derive(Clone, Debug)]
struct MinimizeGraph<TBaseGraph: search::Graph>
where
    TBaseGraph::Node: BaseGraphNode,
{
    base_nodes: BTreeSet<TBaseGraph::Node>,
}

impl<TBaseGraph: search::Graph> search::Graph for MinimizeGraph<TBaseGraph>
where
    TBaseGraph::Node: Ord,
{
    type Node = Subset<TBaseGraph::Node>;
    type Error = MinimizeGraphError<TBaseGraph>;

    fn is_ancestor(
        &self,
        ancestor: &Self::Node,
        descendant: &Self::Node,
    ) -> Result<bool, Self::Error> {
        // @nocommit is this correct?
        Ok(ancestor.is_subset(descendant))
    }

    fn add_success_bound(
        &self,
        nodes: NodeSet<Self::Node>,
        node: &Self::Node,
    ) -> Result<NodeSet<Self::Node>, Self::Error> {
        if nodes
            .iter()
            .any(|success_node| node.is_subset(success_node))
        {
            return Ok(nodes);
        }

        Ok(nodes
            .into_iter()
            .filter(|success_node| !success_node.is_subset(node))
            .cloned()
            .chain([node.clone()])
            .collect())
    }

    fn add_failure_bound(
        &self,
        nodes: NodeSet<Self::Node>,
        node: &Self::Node,
    ) -> Result<NodeSet<Self::Node>, Self::Error> {
        if nodes
            .iter()
            .any(|failure_node| failure_node.is_subset(node))
        {
            return Ok(nodes);
        }

        Ok(nodes
            .into_iter()
            .filter(|failure_node| !node.is_subset(failure_node))
            .cloned()
            .chain([node.clone()])
            .collect())
    }
}

/// The minimization algorithm.
///
/// @nocommit: move docs to `BaseNode` marker trait.
/// Note that this `struct` imposes an additional `G::Node: Ord` bound. This is
/// so that the search is deterministic when arbitrarily selecting one node from
/// a set of nodes. Any total ordering is acceptable, as long as different nodes
/// compare to be unequal. Specifically:
///
/// - Reflexivity: `A = A`
/// - Anti-reflexivity: `A != B` for different A and B (@nocommit correct term?)
/// - Symmetry: `A = B -> B = A` and `A < B -> B > A`
/// - Transitivity: `A < B && B < C -> A < C`
///
/// In particular, `Ord` is *not* required to match the topology of the graph.
#[derive(Clone, Debug)]
pub struct Minimize<TBaseGraph: search::Graph>
where
    TBaseGraph::Node: Ord,
{
    search: search::Search<MinimizeGraph<TBaseGraph>>,
}

impl<TBaseGraph: search::Graph> Minimize<TBaseGraph>
where
    TBaseGraph::Node: Ord,
{
    /// Construct a new minimization.
    pub fn new_with_nodes(nodes: impl IntoIterator<Item = TBaseGraph::Node>) -> Self {
        let base_nodes: BTreeSet<TBaseGraph::Node> = nodes.into_iter().collect();
        let none = Subset::default();
        let all = Subset {
            inner: base_nodes.clone(),
        };
        let minimize_graph = MinimizeGraph { base_nodes };
        let search = search::Search::new_with_nodes(minimize_graph, [none, all]);
        Self { search }
    }

    /// Summarize the current search progress and suggest the next node(s) to
    /// search. The caller is responsible for calling `notify` with the result.
    /// @nocommit comment about how speculation can be expensive?
    #[instrument]
    pub fn search<'a>(
        &'a self,
        strategy: &'a Strategy,
    ) -> Result<LazyMinimizeSolution<'a, TBaseGraph>, MinimizeSearchError<TBaseGraph>> {
        let result = self.search.search(strategy)?;
        Ok(result)
    }

    /// Update the search state with the result of a search.
    #[instrument]
    pub fn notify(
        &mut self,
        node: Subset<TBaseGraph::Node>,
        status: search::Status,
    ) -> Result<(), search::NotifyError<Subset<TBaseGraph::Node>, MinimizeGraphError<TBaseGraph>>>
    {
        self.search.notify(node, status)?;
        Ok(())
    }
}

type StrategyError = std::convert::Infallible;

/// The possible strategies for searching the subsets of the graph.
#[derive(Clone, Debug)]
pub enum Strategy {
    /// Start with the empty subset of nodes and search by adding nodes one at a
    /// time. Stop when no individual node, when added, causes the subset to
    /// still satisfy the predicate.
    ///
    /// This produces a global minimum.
    Add,

    /// Start with the full subset of nodes and search by removing nodes one at a
    /// time. Stop when no individual node, when removed, causes the subset to
    /// still satisfy the predicate.
    ///
    /// This produces a local minimum: it may be possible to remove *more* than
    /// one additional node to produce a new subset that still satisfies the
    /// predicate.
    Remove,
}

impl<TBaseGraph: search::Graph> search::Strategy<MinimizeGraph<TBaseGraph>> for Strategy
where
    TBaseGraph::Node: BaseGraphNode,
{
    type Error = StrategyError;

    fn midpoints(
        &self,
        graph: &MinimizeGraph<TBaseGraph>,
        bounds: &search::Bounds<Subset<TBaseGraph::Node>>,
        _statuses: &IndexMap<Subset<TBaseGraph::Node>, search::Status>,
    ) -> Result<Vec<Subset<TBaseGraph::Node>>, Self::Error> {
        let midpoints = match self {
            Strategy::Add => {
                if bounds.success.is_empty() {
                    vec![Subset::default()]
                } else {
                    bounds
                        .success
                        .iter()
                        .flat_map(|subset_node| {
                            graph
                                .base_nodes
                                .clone()
                                .into_iter()
                                .map(|base_node| subset_node.clone().insert(base_node))
                        })
                        .collect()
                }
            }
            Strategy::Remove => todo!(),
        };

        let minimum_failure_set_size = bounds
            .failure
            .iter()
            .map(|failure_node| failure_node.len())
            .min();
        let midpoints = midpoints
            .into_iter()
            // If we already have a subset of failing nodes, don't bother searching any subsets that
            // are greater in size (even though it could be technically interesting to find another
            // failing subset of nodes that's not a direct descendant of the known failing subset of
            // nodes). Otherwise we'll try to search a large portion of the power set of the graph
            // "just to make sure" that there's not another interesting failing subset.
            .filter(|midpoint_node| match minimum_failure_set_size {
                Some(minimum_failure_set_size) => midpoint_node.len() <= minimum_failure_set_size,
                None => true,
            })
            // Remove any items that were already excluded by the bounds. FIXME: could be more
            // efficient by not creating the set to begin with if it would be excluded.
            .filter(|midpoint_node| {
                !bounds
                    .success
                    .iter()
                    .any(|success_node| midpoint_node.is_subset(success_node))
                    && !bounds
                        .failure
                        .iter()
                        .any(|failure_node| failure_node.is_subset(midpoint_node))
            })
            .collect();

        Ok(midpoints)
    }
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;
    use proptest::proptest;

    use super::{Minimize, Strategy};
    use crate::minimize::Subset;
    use crate::search;
    use crate::testing::arb_test_graph_and_nodes;
    use crate::testing::TestGraph;

    #[track_caller]
    fn test_minimize_dag_proptest_impl(graph: TestGraph, minimal_failing_base_nodes: Vec<char>) {
        let base_graph_nodes: Subset<_> = graph.nodes.keys().sorted().copied().collect();
        let minimal_failing_subset: Subset<_> = minimal_failing_base_nodes.into_iter().collect();
        let mut minimize =
            // TODO: @nocommit: remove manual type annotation after merging `Graph` and `Strategy`?
            Minimize::<TestGraph>::new_with_nodes(base_graph_nodes.clone().into_iter());

        let mut all_searched_nodes = Vec::new();
        let bounds = loop {
            let search_node = {
                let search::LazySolution {
                    bounds,
                    mut next_to_search,
                } = minimize
                    // TODO: also test `Strategy::Remove`
                    .search(&Strategy::Add)
                    .map_err(|err| err.to_string())
                    .unwrap();

                for success_node in bounds.success.iter() {
                    assert!(!minimal_failing_subset.is_subset(success_node));
                }
                for failure_node in bounds.failure.iter() {
                    assert!(minimal_failing_subset.is_subset(failure_node));
                }

                match next_to_search.next() {
                    Some(search_node) => search_node.unwrap(),
                    None => break bounds.clone(),
                }
            };

            assert!(
                !all_searched_nodes.contains(&search_node),
                "searched node twice: {search_node:?}"
            );
            all_searched_nodes.push(search_node.clone());
            assert!(all_searched_nodes.len() <= 1 << base_graph_nodes.len());

            let status = if minimal_failing_subset.is_subset(&search_node) {
                search::Status::Failure
            } else {
                search::Status::Success
            };
            minimize.notify(search_node, status).unwrap();
        };

        assert!(
            bounds.failure.contains(&minimal_failing_subset),
            "minimal failure subset {minimal_failing_subset:?} not found in failure bounds for {bounds:?}"
        );
        for failure_node in bounds.failure.iter() {
            assert!(
                minimal_failing_subset.is_subset(failure_node),
                "invalid failure node {failure_node:?} found; it should have been a superset of {minimal_failing_subset:?}"
            );
        }

        assert!(bounds
            .success
            .iter()
            .all(|node| node.is_subset(&base_graph_nodes)));
        assert!(bounds
            .failure
            .iter()
            .all(|node| node.is_subset(&base_graph_nodes)));
        assert!(
            bounds.success.is_disjoint(&bounds.failure),
            "bounds were not disjoint: {bounds:?}"
        );

        for success_node in bounds.success.iter() {
            for failure_node in bounds.failure.iter() {
                assert!(
                    !failure_node.is_subset(success_node),
                    "failure node bound {failure_node:?} was inconsistent with success node bound {success_node:?}"
                );
            }
        }
    }

    proptest! {
        #[test]
        fn test_minimize_dag_proptest((graph, failure_nodes) in arb_test_graph_and_nodes(16)) {
            test_minimize_dag_proptest_impl(graph, failure_nodes);
        }
    }
}
