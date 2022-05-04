use std::collections::HashSet;
use std::path::PathBuf;

use branchless::core::dag::Dag;
use branchless::core::effects::Effects;
use branchless::core::eventlog::{EventLogDb, EventReplayer};
use branchless::core::formatting::Glyphs;
use branchless::core::repo_ext::RepoExt;
use branchless::core::rewrite::{BuildRebasePlanOptions, RebasePlanBuilder, RepoResource};
use branchless::git::{CherryPickFastOptions, Commit, Diff, Repo};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use rayon::ThreadPoolBuilder;

fn get_repo() -> Repo {
    let repo_dir =
        std::env::var("PATH_TO_REPO").expect("`PATH_TO_REPO` environment variable not set");
    Repo::from_dir(&PathBuf::from(repo_dir)).unwrap()
}

fn nth_parent(commit: Commit, n: usize) -> Commit {
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
        let references_snapshot = repo.get_references_snapshot().unwrap();
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
        let dag = Dag::open_and_sync(
            &effects,
            &repo,
            &event_replayer,
            event_cursor,
            &references_snapshot,
        )
        .unwrap();
        let pool = ThreadPoolBuilder::new().build().unwrap();
        let repo_pool = RepoResource::new_pool(&repo).unwrap();

        let mut builder = RebasePlanBuilder::new(&dag);
        builder
            .move_subtree(later_commit.get_oid(), earlier_commit.get_oid())
            .unwrap();
        b.iter_batched(
            || builder.clone(),
            |builder| {
                builder
                    .build(
                        &effects,
                        &pool,
                        &repo_pool,
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

fn bench_cherry_pick_fast(c: &mut Criterion) {
    let mut group = c.benchmark_group("cherry-pick");
    group.sample_size(10);
    group.bench_function("Repo::cherry_pick_commit", |b| {
        let repo = get_repo();
        let head_oid = repo.get_head_info().unwrap().oid.unwrap();
        let head_commit = repo.find_commit_or_fail(head_oid).unwrap();
        let target_commit = nth_parent(head_commit.clone(), 1);

        b.iter(|| {
            let mut index = repo
                .cherry_pick_commit(&head_commit, &target_commit, 0)
                .unwrap();
            let tree_oid = repo.write_index_to_tree(&mut index).unwrap();
            repo.find_tree(tree_oid).unwrap().unwrap()
        })
    });
    group.bench_function("Repo::cherry_pick_fast", |b| {
        let repo = get_repo();
        let head_oid = repo.get_head_info().unwrap().oid.unwrap();
        let head_commit = repo.find_commit_or_fail(head_oid).unwrap();
        let target_commit = nth_parent(head_commit.clone(), 1);

        b.iter(|| {
            repo.cherry_pick_fast(
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

fn bench_diff_fast(c: &mut Criterion) {
    let mut group = c.benchmark_group("diff");
    group.sample_size(10);
    group.bench_function("git2::Repository::diff_tree_to_tree", |b| {
        let repo = get_repo();
        let repo = git2::Repository::open(repo.get_path()).unwrap();
        let commit = repo.head().unwrap().peel_to_commit().unwrap();

        b.iter(|| -> git2::Diff {
            repo.diff_tree_to_tree(
                Some(&commit.tree().unwrap()),
                Some(&commit.parent(0).unwrap().tree().unwrap()),
                None,
            )
            .unwrap()
        })
    });

    group.bench_function("Repo::get_patch_for_commit", |b| {
        let repo = get_repo();
        let oid = repo.get_head_info().unwrap().oid.unwrap();
        let commit = repo.find_commit_or_fail(oid).unwrap();
        let effects = Effects::new_suppress_for_test(Glyphs::text());

        b.iter(|| -> Option<Diff> { repo.get_patch_for_commit(&effects, &commit).unwrap() });
    });
}

fn bench_get_paths_touched_by_commits(c: &mut Criterion) {
    c.bench_function("Repo::get_paths_touched_by_commit", |b| {
        let repo = get_repo();
        let oid = repo.get_head_info().unwrap().oid.unwrap();
        let commit = repo.find_commit_or_fail(oid).unwrap();

        b.iter(|| -> Option<HashSet<PathBuf>> {
            repo.get_paths_touched_by_commit(&commit).unwrap()
        });
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default().sample_size(10);
    targets =
        bench_cherry_pick_fast,
        bench_diff_fast,
        bench_get_paths_touched_by_commits,
        bench_rebase_plan,
);
criterion_main!(benches);
