use std::path::PathBuf;

use branchless::core::eventlog::{EventLogDb, EventReplayer};
use branchless::core::formatting::Glyphs;
use branchless::core::graph::{make_graph, BranchOids, HeadOid, MainBranchOid};
use branchless::core::mergebase::{make_merge_base_db, MergeBaseDb};
use branchless::core::rewrite::{BuildRebasePlanOptions, RebasePlanBuilder};
use branchless::git::{CherryPickFastOptions, Commit, Repo};
use branchless::tui::Effects;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

fn get_repo() -> Repo {
    let repo_dir =
        std::env::var("PATH_TO_REPO").expect("`PATH_TO_REPO` environment variable not set");
    Repo::from_dir(&PathBuf::from(repo_dir)).unwrap()
}

fn nth_parent<'repo>(commit: Commit<'repo>, n: usize) -> Commit<'repo> {
    let mut commit = commit.clone();
    for _i in 0..n {
        commit = match commit.get_parents().first() {
            Some(commit) => commit.clone(),
            None => panic!("Couldn't find parent of: {:?}", commit),
        }
    }
    commit
}

fn bench_rebase_plan(c: &mut Criterion) {
    c.bench_function("RebasePlanBuilder::build", |b| {
        let repo = get_repo();
        let head_oid = repo.get_head_info().unwrap().oid.unwrap();
        let later_commit = nth_parent(repo.find_commit_or_fail(head_oid).unwrap(), 20);
        let earlier_commit = nth_parent(later_commit.clone(), 1000);
        println!("Comparing {:?} with {:?}", &earlier_commit, &later_commit);

        let effects = Effects::new_suppress_for_test(Glyphs::text());
        let conn = repo.get_db_conn().unwrap();
        let event_log_db = EventLogDb::new(&conn).unwrap();
        let event_replayer =
            EventReplayer::from_event_log_db(&effects, &repo, &event_log_db).unwrap();
        let event_cursor = event_replayer.make_default_cursor();
        let merge_base_db = make_merge_base_db(&effects, &repo, &conn, &event_replayer).unwrap();
        let graph = make_graph(
            &effects,
            &repo,
            &merge_base_db,
            &event_replayer,
            event_cursor,
            &HeadOid(Some(head_oid)),
            &MainBranchOid(head_oid),
            &BranchOids(Default::default()),
            true,
        )
        .unwrap();
        println!("Built commit graph ({:?} elements)", graph.len());

        let mut builder =
            RebasePlanBuilder::new(&repo, &graph, &merge_base_db, &MainBranchOid(head_oid));
        builder
            .move_subtree(later_commit.get_oid(), earlier_commit.get_oid())
            .unwrap();
        b.iter_batched(
            || builder.clone(),
            |builder| {
                builder
                    .build(
                        &effects,
                        &BuildRebasePlanOptions {
                            dump_rebase_constraints: false,
                            dump_rebase_plan: false,
                            detect_duplicate_commits_via_patch_id: true,
                        },
                    )
                    .unwrap()
                    .unwrap()
                    .unwrap()
            },
            BatchSize::PerIteration,
        )
    });
}

fn bench_find_path_to_merge_base(c: &mut Criterion) {
    c.bench_function("MergeBaseDb::find_path_to_merge_base", |b| {
        let repo = get_repo();
        let head_oid = repo.get_head_info().unwrap().oid.unwrap();
        let later_commit = nth_parent(repo.find_commit_or_fail(head_oid).unwrap(), 20);
        let earlier_commit = nth_parent(later_commit.clone(), 1000);
        println!(
            "Finding path to merge-base for {:?} and {:?}",
            &earlier_commit, &later_commit
        );

        let effects = Effects::new_suppress_for_test(Glyphs::text());
        let conn = repo.get_db_conn().unwrap();
        let event_log_db = EventLogDb::new(&conn).unwrap();
        let event_replayer =
            EventReplayer::from_event_log_db(&effects, &repo, &event_log_db).unwrap();
        let merge_base_db = make_merge_base_db(&effects, &repo, &conn, &event_replayer).unwrap();
        let merge_base_db: &dyn MergeBaseDb = &merge_base_db;

        b.iter(|| {
            merge_base_db.find_path_to_merge_base(
                &effects,
                &repo,
                later_commit.get_oid(),
                earlier_commit.get_oid(),
            )
        })
    });
}

fn bench_cherrypick_fast(c: &mut Criterion) {
    let mut group = c.benchmark_group("Cherrypick");
    group.sample_size(10);
    group.bench_function("Repo::cherrypick_commit", |b| {
        let repo = get_repo();
        let head_oid = repo.get_head_info().unwrap().oid.unwrap();
        let head_commit = repo.find_commit_or_fail(head_oid).unwrap();
        let target_commit = nth_parent(head_commit.clone(), 1);

        b.iter(|| {
            let mut index = repo
                .cherrypick_commit(&head_commit, &target_commit, 0)
                .unwrap();
            let tree_oid = repo.write_index_to_tree(&mut index).unwrap();
            repo.find_tree(tree_oid).unwrap().unwrap()
        })
    });
    group.bench_function("Repo::cherrypick_fast", |b| {
        let repo = get_repo();
        let head_oid = repo.get_head_info().unwrap().oid.unwrap();
        let head_commit = repo.find_commit_or_fail(head_oid).unwrap();
        let target_commit = nth_parent(head_commit.clone(), 1);

        b.iter(|| {
            repo.cherrypick_fast(
                &head_commit,
                &target_commit,
                &CherryPickFastOptions {
                    reuse_parent_tree_if_possible: false,
                },
            )
            .unwrap()
            .unwrap();
        });
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = bench_rebase_plan, bench_find_path_to_merge_base, bench_cherrypick_fast
);
criterion_main!(benches);
