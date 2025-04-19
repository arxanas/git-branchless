use std::time::SystemTime;

use branchless::core::dag::Dag;
use branchless::core::effects::Effects;
use branchless::core::rewrite::testing::{
    get_builder_touched_paths_cache, omnipotent_rebase_plan_permissions,
};
use rayon::ThreadPoolBuilder;

use branchless::core::check_out::CheckOutCommitOptions;
use branchless::core::eventlog::{EventLogDb, EventReplayer};
use branchless::core::formatting::Glyphs;
use branchless::core::repo_ext::RepoExt;
use branchless::core::rewrite::{
    execute_rebase_plan, BuildRebasePlanOptions, ExecuteRebasePlanOptions, ExecuteRebasePlanResult,
    RebasePlan, RebasePlanBuilder, RepoResource,
};
use branchless::git::SignOption;
use branchless::testing::{make_git, Git};

#[test]
fn test_cache_shared_between_builders() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;

    let effects = Effects::new_suppress_for_test(Glyphs::text());
    let repo = git.get_repo()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let references_snapshot = repo.get_references_snapshot()?;
    let dag = Dag::open_and_sync(
        &effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let build_options = BuildRebasePlanOptions {
        force_rewrite_public_commits: true,
        dump_rebase_constraints: false,
        dump_rebase_plan: false,
        detect_duplicate_commits_via_patch_id: true,
    };
    let permissions = omnipotent_rebase_plan_permissions(&dag, build_options)?;
    let pool = ThreadPoolBuilder::new().build()?;
    let repo_pool = RepoResource::new_pool(&repo)?;
    let mut builder = RebasePlanBuilder::new(&dag, permissions);
    let builder2 = builder.clone();
    builder.move_subtree(test3_oid, vec![test1_oid])?;
    let result = builder.build(&effects, &pool, &repo_pool)?;
    let result = result.unwrap();
    let _ignored: Option<RebasePlan> = result;
    assert!(get_builder_touched_paths_cache(&builder).contains_key(&test1_oid));
    assert!(get_builder_touched_paths_cache(&builder2).contains_key(&test1_oid));

    Ok(())
}

#[test]
fn test_plan_moving_subtree_again_overrides_previous_move() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.move_subtree(test4_oid, vec![test1_oid])?;
        builder.move_subtree(test4_oid, vec![test2_oid])?;
        Ok(())
    })?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |\
        | @ 8556cef create test4.txt
        |
        o 70deb1e create test3.txt
        "###);

    Ok(())
}

#[test]
fn test_plan_moving_subtree_within_moved_subtree() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.move_subtree(test3_oid, vec![test1_oid])?;
        builder.move_subtree(test4_oid, vec![test1_oid])?;
        Ok(())
    })?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | @ 8e71a64 create test4.txt
        |\
        | o ad2c2fc create test3.txt
        |
        o 96d1c37 create test2.txt
        "###);

    Ok(())
}

#[test]
fn test_plan_moving_subtree_within_moved_subtree_in_other_order() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.move_subtree(test4_oid, vec![test1_oid])?;
        builder.move_subtree(test3_oid, vec![test1_oid])?;
        Ok(())
    })?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | @ 8e71a64 create test4.txt
        |\
        | o ad2c2fc create test3.txt
        |
        o 96d1c37 create test2.txt
        "###);

    Ok(())
}

#[test]
fn test_plan_moving_commit_again_overrides_previous_move() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.move_commit(test4_oid, test1_oid)?;
        builder.move_commit(test4_oid, test2_oid)?;
        Ok(())
    })?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |\
        | @ 8556cef create test4.txt
        |
        o 70deb1e create test3.txt
        "###);

    Ok(())
}

#[test]
fn test_plan_moving_nonconsecutive_commits() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;
    let test5_oid = git.commit_file("test5", 5)?;
    git.commit_file("test6", 6)?;

    git.smartlog()?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.move_commit(test3_oid, test1_oid)?;
        builder.move_commit(test5_oid, test1_oid)?;
        Ok(())
    })?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o 4ec3989 create test5.txt
        |\
        | o ad2c2fc create test3.txt
        |
        o 96d1c37 create test2.txt
        |
        o 8556cef create test4.txt
        |
        @ 0a34830 create test6.txt
        "###);

    Ok(())
}

#[test]
fn test_plan_moving_consecutive_commits() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.commit_file("test5", 5)?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.move_commit(test3_oid, test1_oid)?;
        builder.move_commit(test4_oid, test1_oid)?;
        Ok(())
    })?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o 8e71a64 create test4.txt
        |\
        | o ad2c2fc create test3.txt
        |
        o 96d1c37 create test2.txt
        |
        @ f26f28e create test5.txt
        "###);

    Ok(())
}

#[test]
fn test_plan_moving_consecutive_commits_in_other_order() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.commit_file("test5", 5)?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.move_commit(test4_oid, test1_oid)?;
        builder.move_commit(test3_oid, test1_oid)?;
        Ok(())
    })?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o 8e71a64 create test4.txt
        |\
        | o ad2c2fc create test3.txt
        |
        o 96d1c37 create test2.txt
        |
        @ f26f28e create test5.txt
        "###);

    Ok(())
}

#[test]
fn test_plan_moving_commit_and_one_child_leaves_other_child() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.detach_head()?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.run(&["checkout", &test3_oid.to_string()])?;
    git.commit_file("test5", 5)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        o 70deb1e create test3.txt
        |\
        | o 355e173 create test4.txt
        |
        @ 9ea1b36 create test5.txt
        "###);

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.move_commit(test3_oid, test1_oid)?;
        builder.move_commit(test4_oid, test1_oid)?;
        Ok(())
    })?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o 8e71a64 create test4.txt
        |\
        | o ad2c2fc create test3.txt
        |
        o 96d1c37 create test2.txt
        |
        @ f26f28e create test5.txt
        "###);

    Ok(())
}

#[test]
fn test_plan_moving_commit_add_then_giving_it_a_child() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;
    let test5_oid = git.commit_file("test5", 5)?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.move_commit(test3_oid, test1_oid)?;
        builder.move_commit(test5_oid, test3_oid)?;
        Ok(())
    })?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o ad2c2fc create test3.txt
        | |
        | @ ee4aebf create test5.txt
        |
        o 96d1c37 create test2.txt
        |
        o 8556cef create test4.txt
        "###);

    Ok(())
}

#[test]
fn test_plan_moving_range_again_overrides_previous_move() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    let test5_oid = git.commit_file("test5", 5)?;
    git.commit_file("test6", 6)?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.move_range(test4_oid, test5_oid, test1_oid)?;
        builder.move_range(test4_oid, test5_oid, test2_oid)?;
        Ok(())
    })?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |\
        | o 8556cef create test4.txt
        | |
        | o 566236a create test5.txt
        |
        o 70deb1e create test3.txt
        |
        @ 35928ae create test6.txt
        "###);

    Ok(())
}

#[test]
fn test_plan_moving_range_and_then_partial_beginning_range_again() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    let test5_oid = git.commit_file("test5", 5)?;
    git.commit_file("test6", 6)?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.move_range(test3_oid, test5_oid, test1_oid)?;
        builder.move_range(test3_oid, test4_oid, test1_oid)?;
        Ok(())
    })?;

    let stdout = git.smartlog()?;

    // FIXME This output is correct mathematically, but feels like it should
    // be incorrect. What *should* we be doing if the user moves 2 ranges w/
    // the same source/root oid but different end oids??
    //
    // NOTE See also the next test for the other case: where the end of the
    // range is moved again. That *does* feel correct.
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o ad2c2fc create test3.txt
        | |
        | o 2b45b52 create test4.txt
        |
        o 96d1c37 create test2.txt
        |\
        | @ 99a62a3 create test6.txt
        |
        o f26f28e create test5.txt
        "###);

    Ok(())
}

#[test]
fn test_plan_moving_range_and_then_partial_ending_range_again() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    let test5_oid = git.commit_file("test5", 5)?;
    git.commit_file("test6", 6)?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.move_range(test3_oid, test5_oid, test1_oid)?;
        builder.move_range(test4_oid, test5_oid, test1_oid)?;
        Ok(())
    })?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o 8e71a64 create test4.txt
        | |
        | o 3d57d30 create test5.txt
        |\
        | o ad2c2fc create test3.txt
        |
        o 96d1c37 create test2.txt
        |
        @ 99a62a3 create test6.txt
        "###);

    Ok(())
}

#[test]
fn test_plan_moving_subtree_and_commit_within_subtree() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    let _test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    let _test5_oid = git.commit_file("test5", 5)?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.move_subtree(test3_oid, vec![test1_oid])?;
        builder.move_commit(test4_oid, test1_oid)?;
        Ok(())
    })?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o 8e71a64 create test4.txt
        |\
        | o ad2c2fc create test3.txt
        | |
        | @ ee4aebf create test5.txt
        |
        o 96d1c37 create test2.txt
        "###);

    Ok(())
}

#[test]
fn test_plan_moving_subtree_and_then_its_parent_commit() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    let _test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    let _test5_oid = git.commit_file("test5", 5)?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.move_subtree(test4_oid, vec![test1_oid])?;
        builder.move_commit(test3_oid, test1_oid)?;
        Ok(())
    })?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o 8e71a64 create test4.txt
        | |
        | @ 3d57d30 create test5.txt
        |\
        | o ad2c2fc create test3.txt
        |
        o 96d1c37 create test2.txt
        "###);

    Ok(())
}

#[test]
fn test_plan_moving_subtree_to_descendant_of_itself() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let _test1_oid = git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let _test4_oid = git.commit_file("test4", 4)?;
    let test5_oid = git.commit_file("test5", 5)?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.move_subtree(test3_oid, vec![test5_oid])?;
        builder.move_subtree(test5_oid, vec![test2_oid])?;
        Ok(())
    })?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        @ f26f28e create test5.txt
        |
        o 2b42d9c create test3.txt
        |
        o c533a65 create test4.txt
        "###);

    Ok(())
}

#[test]
fn test_plan_moving_subtree_with_merge_commit() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let _test1_oid = git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.run(&["checkout", "HEAD~"])?;
    let _test5_oid = git.commit_file("test5", 5)?;
    git.run(&["merge", &test4_oid.to_string()])?;
    git.run(&["checkout", "HEAD~"])?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.move_subtree(test3_oid, vec![test2_oid])?;
        Ok(())
    })?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    O f777ecc (master) create initial.txt
    |
    o 62fc20d create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 70deb1e create test3.txt
    |\
    | o 355e173 create test4.txt
    | & (merge) 8fb706a Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
    |
    @ 9ea1b36 create test5.txt
    |
    | & (merge) 355e173 create test4.txt
    |/
    o 8fb706a Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
    "###);

    Ok(())
}

#[test]
fn test_plan_fixup_child_into_parent() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let _test1_oid = git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.fixup_commit(test3_oid, test2_oid)?;
        Ok(())
    })?;

    let (stdout, _stderr) = git.run(&["smartlog"])?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ 64da0f2 create test2.txt
        "###);

    Ok(())
}

#[test]
fn test_plan_fixup_parent_into_child() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;
    let _test1_oid = git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;

    create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
        builder.fixup_commit(test2_oid, test3_oid)?;
        Ok(())
    })?;

    let (stdout, _stderr) = git.run(&["smartlog"])?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ 1f599a2 create test3.txt
        "###);

    Ok(())
}

/// Helper function to handle the boilerplate involved in creating, building
/// and executing the rebase plan.
fn create_and_execute_plan(
    git: &Git,
    builder_callback_fn: impl Fn(&mut RebasePlanBuilder) -> eyre::Result<()>,
) -> eyre::Result<()> {
    let effects = Effects::new_suppress_for_test(Glyphs::text());
    let repo = git.get_repo()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let references_snapshot = repo.get_references_snapshot()?;
    let dag = Dag::open_and_sync(
        &effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let pool = ThreadPoolBuilder::new().build()?;
    let repo_pool = RepoResource::new_pool(&repo)?;

    let build_options = BuildRebasePlanOptions {
        force_rewrite_public_commits: false,
        dump_rebase_constraints: false,
        dump_rebase_plan: false,
        detect_duplicate_commits_via_patch_id: true,
    };
    let permissions = omnipotent_rebase_plan_permissions(&dag, build_options)?;
    let mut builder = RebasePlanBuilder::new(&dag, permissions);

    builder_callback_fn(&mut builder)?;

    let build_result = builder.build(&effects, &pool, &repo_pool)?;

    let rebase_plan = match build_result {
        Ok(None) => return Ok(()),
        Ok(Some(rebase_plan)) => rebase_plan,
        Err(rebase_plan_error) => {
            eyre::bail!("Error building rebase plan: {:#?}", rebase_plan_error)
        }
    };

    let now = SystemTime::UNIX_EPOCH;
    let options = ExecuteRebasePlanOptions {
        now,
        event_tx_id: event_log_db.make_transaction_id(now, "test plan")?,
        preserve_timestamps: false,
        force_in_memory: false,
        force_on_disk: false,
        resolve_merge_conflicts: true,
        check_out_commit_options: CheckOutCommitOptions {
            additional_args: Default::default(),
            reset: false,
            render_smartlog: false,
        },
        sign_option: SignOption::Disable,
    };
    let git_run_info = git.get_git_run_info();
    let result = execute_rebase_plan(
        &effects,
        &git_run_info,
        &repo,
        &event_log_db,
        &rebase_plan,
        &options,
    )?;
    assert!(matches!(
        result,
        ExecuteRebasePlanResult::Succeeded { rewritten_oids: _ }
    ));

    Ok(())
}
