use std::collections::HashSet;

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use scm_bisect::minimize;
use scm_bisect::search;
use scm_bisect::testing::UsizeGraph;

fn bench_minimize(
    strategy: minimize::Strategy,
    graph_size: usize,
    speculation_size: usize,
) -> search::Bounds<minimize::Subset<usize>> {
    let graph = UsizeGraph { max: graph_size };
    let nodes = 0..graph.max;
    let is_problem = |set: &HashSet<usize>| -> bool { set.contains(&2) && set.contains(&4) };
    // @nocommit: remove explicit type annotation once graph and strategy are merged
    let mut minimize = minimize::Minimize::<UsizeGraph>::new_with_nodes(nodes);

    let bounds = loop {
        let search_nodes = {
            let search::LazySolution {
                bounds,
                next_to_search,
            } = minimize.search(&strategy).unwrap();
            let search_nodes = next_to_search
                .take(speculation_size)
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            if search_nodes.is_empty() {
                break bounds.clone();
            } else {
                search_nodes
            }
        };
        for search_node in search_nodes {
            let status = if is_problem(&search_node.iter().copied().collect()) {
                search::Status::Failure
            } else {
                search::Status::Success
            };
            minimize.notify(search_node, status).unwrap();
        }
    };
    bounds
}

fn bench_minimize_per_graph_size(c: &mut Criterion) {
    // @nocommit increase graph size significantly
    for graph_size in [4, 8, 32, 128] {
        c.bench_function(&format!("bench_minimize_graph_size_{graph_size}"), |b| {
            b.iter(|| black_box(bench_minimize(minimize::Strategy::Add, graph_size, 1)))
        });
    }
}

// @nocommit: fix slow speculation
fn bench_minimize_per_speculation_size(c: &mut Criterion) {
    for speculation_size in [1, 4, 8] {
        c.bench_function(
            &format!("bench_minimize_speculation_size_{speculation_size}"),
            |b| b.iter(|| black_box(bench_minimize(minimize::Strategy::Add, 8, speculation_size))),
        );
    }
}

criterion_group!(
    benches,
    bench_minimize_per_graph_size,
    bench_minimize_per_speculation_size
);
criterion_main!(benches);
