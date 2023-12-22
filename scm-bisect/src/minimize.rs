//! A minimization algorithm to find the smallest subset of nodes that, when
//! combined, satisfies a certain predicate.
//!
//! Generally speaking, the minimization of a set of nodes of type `T` can be
//! viewed as a kind of search on the [power set][1] of nodes of type `T`. That
//! is, a `Minimization<T>` is essentially a wrapper around a
//! `search::Search<HashSet<T>>`.
//!
//! The trick is merely in how we define the "bisection" function, which
//! produces the next node to check. Such a node has type `HashSet<T>`, and
//! should be "between" the lower and upper bounds of type `HashSet<T>` (a
//! superset of the lower bound and a subset of the upper bound).
//!
//! For a base search graph with `n` nodes, the minimization earch graph has
//! `2^n` nodes, so the minimization search space is exponentially larger, and
//! therefore an exhaustive search may be infeasible. Therefore, we often want
//! to apply a heuristic search that will produce a useful and probable answer,
//! even if it's possibly suboptimal.
//!
//! [1]: @nocommit link to power set.

use std::collections::{BTreeSet, HashSet};
use std::hash::Hash;

use tracing::instrument;

use crate::search;

/// @nocommit: use a more efficient frozen hash-set type.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Subset<T: Eq + Ord> {
    inner: BTreeSet<T>,
}

impl<T: Eq + Ord> Subset<T> {
    pub fn into_iter(self) -> impl Iterator<Item = T> {
        self.inner.into_iter()
    }
}

impl<A: Ord> FromIterator<A> for Subset<A> {
    fn from_iter<T: IntoIterator<Item = A>>(iter: T) -> Self {
        Self {
            inner: iter.into_iter().collect(),
        }
    }
}

#[derive(Clone, Debug)]
struct MinimizeGraph<G: search::Graph>
where
    G::Node: Ord,
{
    graph: G,
}

#[derive(Debug, thiserror::Error)]
enum MinimizeError<G: search::Graph> {
    #[error(transparent)]
    Graph(G::Error),
}

impl<G: search::Graph> search::Graph for MinimizeGraph<G>
where
    G::Node: Ord,
{
    type Node = Subset<G::Node>;
    type Error = MinimizeError<G>;

    fn is_ancestor(
        &self,
        ancestor: Self::Node,
        descendant: Self::Node,
    ) -> Result<bool, Self::Error> {
        // @nocommit is this correct?
        Ok(ancestor.inner.is_subset(&descendant.inner))
    }

    fn simplify_success_bounds(
        &self,
        nodes: HashSet<Self::Node>,
    ) -> Result<HashSet<Self::Node>, Self::Error> {
        // @nocommit implement
        Ok(nodes)
    }

    fn simplify_failure_bounds(
        &self,
        nodes: HashSet<Self::Node>,
    ) -> Result<HashSet<Self::Node>, Self::Error> {
        // @nocommit implement
        Ok(nodes)
    }
}

impl<G: search::Graph> MinimizeGraph<G>
where
    G::Node: Ord,
{
    pub fn new(graph: G) -> Self {
        Self { graph }
    }
}

/// The minimization algorithm.
///
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
pub struct Minimize<G: search::Graph>
where
    G::Node: Ord,
{
    search: search::Search<MinimizeGraph<G>>,
}

impl<G: search::Graph> Minimize<G>
where
    G::Node: Ord,
{
    /// Construct a new minimization.
    pub fn new(graph: G, nodes: impl IntoIterator<Item = G::Node>) -> Self {
        let nodes: BTreeSet<G::Node> = nodes.into_iter().collect();
        let nodes = Subset {
            inner: nodes.clone(),
        };
        let search = search::Search::new(MinimizeGraph::new(graph), nodes.into_iter());
        Self { search }
    }

    /// Summarize the current search progress and suggest the next node(s) to
    /// search. The caller is responsible for calling `notify` with the result.
    #[instrument]
    pub fn search(
        &self,
        strategy: Strategy,
    ) -> Result<search::LazySolution<Subset<G::Node>, MinimizeError<G>>, MinimizeError<G>> {
        todo!();
    }

    /// Update the search state with the result of a search.
    #[instrument]
    pub fn notify(
        &mut self,
        node: Subset<G::Node>,
        status: search::Status,
    ) -> Result<(), MinimizeError<G>> {
        self.search.notify(node, status)?;
        Ok(())
    }
}

/// The possible strategies for searching the subsets of the graph.
#[derive(Clone, Debug)]
pub enum Strategy {
    /// Start with the empty subset of nodes and search by adding nodes one at a
    /// time. Stop when no individual node, when added, causes the subset to
    /// still satisfy the predicate.
    ///
    /// This produces a local minimum: it may be possible to add *more* than one
    /// additional node to produce a new subset that still satisfies the
    /// predicate.
    LocalMinimum,

    /// Start with the full subset of nodes and search by removing nodes one at a
    /// time. Stop when no individual node, when removed, causes the subset to
    /// still satisfy the predicate.
    ///
    /// This produces a local minimum: it may be possible to remove *more* than
    /// one additional node to produce a new subset that still satisfies the
    /// predicate.
    LocalMaximum,
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;
    use proptest::prelude::Strategy as ProptestStrategy;
    use proptest::prelude::*;
    use proptest::proptest;

    use super::{Minimize, Strategy};
    use crate::basic::tests::{arb_test_graph_and_nodes, TestGraph, UsizeGraph};
    use crate::search;

    use std::collections::BTreeSet;
    use std::collections::HashSet;
    use std::convert::Infallible;

    #[test]
    fn test_minimize() -> Result<(), Infallible> {
        let graph = UsizeGraph { max: 7 };
        let nodes = 0..graph.max;
        let is_problem = |set: &HashSet<usize>| -> bool { set.contains(&3) && set.contains(&5) };
        let search = search::Search::new(graph, nodes);
        let mut minimize = Minimize::new(search);
        todo!();
    }

    #[track_caller]
    fn test_minimize_dag_proptest_inner(graph: TestGraph, failure_nodes: Vec<char>) {
        let nodes = graph.nodes.keys().sorted().copied().collect::<Vec<_>>();
        let failure_nodes: BTreeSet<_> = failure_nodes.into_iter().collect();
        let search = search::Search::new(graph.clone(), nodes);
        let mut minimize = Minimize::new(search);

        let solution = loop {
            let search::EagerSolution {
                bounds: search::Bounds { success, failure },
                mut next_to_search,
            } = minimize
                .search(Strategy::LocalMinimum)
                .unwrap()
                .into_eager();
            for success_node in success {
                assert!(success_node.inner.is_superset(&failure_nodes));
            }
            for failure_node in failure {
                assert!(failure_node.inner.is_subset(&failure_nodes));
            }
            if !next_to_search.is_empty() {
                let node = next_to_search.swap_remove(0);
                let status = if node.inner.iter().all(|node| failure_nodes.contains(node)) {
                    search::Status::Failure
                } else {
                    search::Status::Success
                };
                minimize.notify(node.clone(), status).unwrap();
            } else {
                return;
                // break search::EagerSolution
            }
        };

        // let nodes = graph.nodes.keys().copied().collect::<HashSet<_>>();
        // assert!(solution.bounds.success.is_subset(&nodes));
        // assert!(solution.bounds.failure.is_subset(&nodes));
        // assert!(solution
        //     .bounds
        //     .success
        //     .is_disjoint(&solution.bounds.failure));
        // let all_success_nodes = graph
        //     .ancestors_all(solution.bounds.success.clone())
        //     .unwrap();
        // let all_failure_nodes = graph.descendants_all(solution.bounds.failure).unwrap();
        // assert!(all_success_nodes.is_disjoint(&all_failure_nodes));
        // assert!(
        //         all_success_nodes.union(&all_failure_nodes).copied().collect::<HashSet<_>>() == nodes,
        //         "all_success_nodes: {all_success_nodes:?}, all_failure_nodes: {all_failure_nodes:?}, nodes: {nodes:?}",
        //     );
    }

    proptest! {
        #[test]
        fn test_minimize_dag_proptest((graph, failure_nodes) in arb_test_graph_and_nodes()) {
            test_minimize_dag_proptest_inner(graph, failure_nodes);
        }
    }
}
