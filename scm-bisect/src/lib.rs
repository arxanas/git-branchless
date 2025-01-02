//! Reusable algorithms for identifying the first commit satisfying a given
//! predicate in a directed acyclic graph.
//!
//! This is similar to `git bisect` and related commands being used to find the
//! first "bad" commit in the graph.
//!
//! ## Goals
//!
//! The objective of this crate is to provide reusable bisection support for new
//! source control systems. Features:
//!
//! - **Scalability**: Supports searching the graph without necessarily evaluating
//!   its full structure, as many source control graphs are large.
//!
//!   - For large graphs, [`search::Search`] is suitable, as it only needs to be
//!     able to query if one node is an ancestor of another.
//!   - For small graphs, [`basic_search::BasicSourceControlGraph`] provides a
//!     more convenient implementation of [`search::Graph`], at the expense of
//!     potentially evaluating the full graph structure.
//!
//! - **Flexibility**: Callers can provide custom implementations of
//!   [`search::Strategy`] without having to reimplement features like speculative
//!   search.
//!
//! - **Parallelism**: Supports parallel search. The search interface does not
//!   block while waiting for test results from the caller. Tests do not have to
//!   be performed serially or in any particular order.
//!
//! > **NOTE**: This crate does not provide any facilities for test execution,
//! > queueing, or distributing work. "Support for parallelism" just means that
//! > the search interface does not block.
//!
//! - **Speculation**: Callers can search using an arbitrary amount of parallelism
//!   by requesting additional, speculative nodes to search.  This does not affect
//!   the asymptotic complexity of testing, but can reduce wall-clock time.
//!
//! - **Adaptation**: Callers can add or remove parallelism at runtime without
//!   considerably disrupting the search. Ongoing tests can be cancelled without
//!   affecting search invariants.
//!
//! > **NOTE**: Specifically, the [`search::Search`] algorithm does recursive
//! > 2-section instead of n-section. For more discussion, see:
//! > <https://github.com/martinvonz/jj/issues/2987#issuecomment-1937856057>
//!
//! ## Example
//!
//! The basic approach is to implement [`search::Graph`] and
//! [`search::Strategy`], then use a [`search::Search`] with them in a loop.
//!
//! ```
//! # use std::collections::HashSet;
//! use scm_bisect::{basic_search, search};
//! use scm_bisect::basic_search::{BasicStrategy, BasicStrategyKind};
//!
//! /// Represents a "stick" graph where each `n` is the immediate parent of
//! /// `n + 1`. Real-world uses will typically involve a more complicated directed
//! /// acyclic graph instead.
//! #[derive(Clone, Debug)]
//! struct Graph {
//!     min: isize,
//!     max: isize,
//!     /// The first node that's considered "bad". This is the node that
//!     /// we're searching for as part of bisection.
//!     first_bad_value: isize,
//! }
//!
//! impl Graph {
//!     /// Test to see whether a given node is "good" or not. Note that the
//!     /// binary search strategy requires that the test function satisfies the
//!     /// monotonic hypothesis: all descendants of the first failing node must
//!     /// also fail.
//!     fn test(&self, value: isize) -> bool {
//!         value < self.first_bad_value
//!     }
//! }
//!
//! /// For demonstration, we use this helper trait that wraps [`search::Graph`]
//! /// and implements the [`BasicSearchKind::Binary`] strategy for us.
//! impl basic_search::BasicSourceControlGraph for Graph {
//!     type Node = isize;
//!     type Error = std::convert::Infallible;
//!     fn ancestors(&self, node: isize) -> Result<HashSet<isize>, Self::Error> {
//!         Ok((self.min..=node).collect())
//!     }
//!     fn descendants(&self, node: isize) -> Result<HashSet<isize>, Self::Error> {
//!         Ok((node..=self.max).collect())
//!     }
//! }
//!
//! # fn main() {
//! let graph = Graph { min: 0, max: 10, first_bad_value: 7 };
//! let mut search = search::Search::new_with_nodes(graph.clone(), 0..=10);
//! let strategy = BasicStrategy::new(BasicStrategyKind::Binary);
//!
//! // Search loop:
//! let bounds = loop {
//!     let next_node_to_test = {
//!         // Ask for the next node to test. Nodes are lazily generated, so we
//!         // consume the next element from the `next_to_search` iterator.
//!         let mut solution = search.search(&strategy).unwrap();
//!         match solution.next_to_search.next() {
//!             Some(node) => node.unwrap(),
//!             None => break solution.bounds,
//!         }
//!     };
//!
//!     let status = if graph.test(next_node_to_test) {
//!         search::Status::Success
//!     } else {
//!         search::Status::Failure
//!     };
//!     search.notify(next_node_to_test, status).unwrap();
//! };
//!
//! // Confirm results:
//! assert_eq!(bounds.success, HashSet::from_iter([6]));
//! assert_eq!(bounds.failure, HashSet::from_iter([7]));
//! # }
//! ```

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

pub mod basic_search;
pub mod search;

#[cfg(test)]
pub mod testing;
