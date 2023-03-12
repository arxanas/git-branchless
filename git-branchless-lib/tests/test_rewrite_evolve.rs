use branchless::core::eventlog::{EventLogDb, EventReplayer};
use branchless::core::formatting::Glyphs;
use branchless::core::rewrite::find_rewrite_target;
use branchless::git::MaybeZeroOid;
use branchless::{core::effects::Effects, git::NonZeroOid};
use git_branchless_testing::{make_git, Git, GitRunOptions};

fn find_rewrite_target_helper(
    effects: &Effects,
    git: &Git,
    oid: NonZeroOid,
) -> eyre::Result<Option<MaybeZeroOid>> {
    let repo = git.get_repo()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();

    let rewrite_target = find_rewrite_target(&event_replayer, event_cursor, oid);
    Ok(rewrite_target)
}

#[test]
fn test_find_rewrite_target() -> eyre::Result<()> {
    let effects = Effects::new_suppress_for_test(Glyphs::text());
    let git = make_git()?;

    git.init_repo()?;
    let commit_time = 1;
    let old_oid = git.commit_file("test1", commit_time)?;

    {
        git.run(&["commit", "--amend", "-m", "test1 amended once"])?;
        let new_oid: MaybeZeroOid = {
            let (stdout, _stderr) = git.run(&["rev-parse", "HEAD"])?;
            stdout.trim().parse()?
        };
        let rewrite_target = find_rewrite_target_helper(&effects, &git, old_oid)?;
        assert_eq!(rewrite_target, Some(new_oid));
    }

    {
        git.run(&["commit", "--amend", "-m", "test1 amended twice"])?;
        let new_oid: MaybeZeroOid = {
            let (stdout, _stderr) = git.run(&["rev-parse", "HEAD"])?;
            stdout.trim().parse()?
        };
        let rewrite_target = find_rewrite_target_helper(&effects, &git, old_oid)?;
        assert_eq!(rewrite_target, Some(new_oid));
    }

    {
        git.run_with_options(
            &["commit", "--amend", "-m", "create test1.txt"],
            &GitRunOptions {
                time: commit_time,
                ..Default::default()
            },
        )?;
        let rewrite_target = find_rewrite_target_helper(&effects, &git, old_oid)?;
        assert_eq!(rewrite_target, None);
    }

    Ok(())
}
