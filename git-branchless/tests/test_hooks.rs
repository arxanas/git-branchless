use eyre::{eyre, Context};
use lib::core::effects::Effects;
use lib::core::eventlog::testing::{
    get_event_replayer_events, redact_event_id, redact_event_timestamp,
};
use lib::core::eventlog::{Event, EventLogDb, EventReplayer};
use lib::core::formatting::Glyphs;
use lib::git::GitVersion;
use lib::testing::make_git;
use lib::util::get_sh;
use std::process::Command;

#[test]
fn test_abandoned_commit_message() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    git.commit_file("test1", 1)?;

    {
        let (_stdout, stderr) = git.run(&["commit", "--amend", "-m", "amend test1"])?;
        insta::assert_snapshot!(stderr, @r###"
        branchless: processing 2 updates: branch master, ref HEAD
        branchless: processed commit: 9e8dbe9 amend test1
        branchless: processing 1 rewritten commit
        "###);
    }

    git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD^"])?;
    git.run(&["branch", "-f", "master"])?;

    {
        let (_stdout, stderr) = git.run(&["commit", "--amend", "-m", "amend test1 again"])?;
        insta::assert_snapshot!(stderr, @r###"
        branchless: processing 1 update: ref HEAD
        branchless: processed commit: c1e22fd amend test1 again
        branchless: processing 1 rewritten commit
        branchless: This operation abandoned 1 commit and 1 branch (master)!
        branchless: Consider running one of the following:
        branchless:   - git restack: re-apply the abandoned commits/branches
        branchless:     (this is most likely what you want to do)
        branchless:   - git smartlog: assess the situation
        branchless:   - git hide [<commit>...]: hide the commits from the smartlog
        branchless:   - git undo: undo the operation
        hint: disable this hint by running: git config --global branchless.hint.restackWarnAbandoned false
        "###);
    }

    Ok(())
}

#[test]
fn test_abandoned_branch_message() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.run(&["branch", "abc"])?;
    git.detach_head()?;

    {
        let (_stdout, stderr) = git.run(&["commit", "--amend", "-m", "amend test1"])?;
        insta::assert_snapshot!(stderr, @r###"
        branchless: processing 1 update: ref HEAD
        branchless: processed commit: 9e8dbe9 amend test1
        branchless: processing 1 rewritten commit
        branchless: This operation abandoned 2 branches (abc, master)!
        branchless: Consider running one of the following:
        branchless:   - git restack: re-apply the abandoned commits/branches
        branchless:     (this is most likely what you want to do)
        branchless:   - git smartlog: assess the situation
        branchless:   - git hide [<commit>...]: hide the commits from the smartlog
        branchless:   - git undo: undo the operation
        hint: disable this hint by running: git config --global branchless.hint.restackWarnAbandoned false
        "###);
    }

    Ok(())
}

#[test]
fn test_fixup_no_abandoned_commit_message() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["commit", "--amend", "-m", "fixup! create test1.txt"])?;
    git.commit_file("test3", 3)?;
    git.run(&["commit", "--amend", "-m", "fixup! create test1.txt"])?;

    let git_version = git.get_version()?;
    {
        let (_stdout, stderr) = git.run(&["rebase", "-i", "master", "--autosquash"])?;
        if git_version < GitVersion(2, 35, 0) {
            insta::assert_snapshot!(stderr, @r###"
            branchless: processing 1 update: ref HEAD
            branchless: processing 1 update: ref HEAD
            branchless: processing 3 rewritten commits
            Successfully rebased and updated detached HEAD.
            "###);
        }
    }

    Ok(())
}

#[test]
fn test_rebase_individual_commit() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.run(&["checkout", "HEAD^"])?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;

    let git_version = git.get_version()?;
    {
        let (_stdout, stderr) = git.run(&["rebase", "master", "HEAD^"])?;
        if git_version < GitVersion(2, 35, 0) {
            insta::assert_snapshot!(stderr, @r###"
            branchless: processing 1 update: ref HEAD
            branchless: processing 1 rewritten commit
            branchless: This operation abandoned 1 commit!
            branchless: Consider running one of the following:
            branchless:   - git restack: re-apply the abandoned commits/branches
            branchless:     (this is most likely what you want to do)
            branchless:   - git smartlog: assess the situation
            branchless:   - git hide [<commit>...]: hide the commits from the smartlog
            branchless:   - git undo: undo the operation
            hint: disable this hint by running: git config --global branchless.hint.restackWarnAbandoned false
            Successfully rebased and updated detached HEAD.
            "###);
        }
    }

    Ok(())
}

#[test]
fn test_interactive_rebase_noop() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    let git_version = git.get_version()?;
    {
        let (_stdout, stderr) = git.run(&["rebase", "-i", "master"])?;
        if git_version < GitVersion(2, 35, 0) {
            insta::assert_snapshot!(stderr, @"Successfully rebased and updated detached HEAD.
");
        }
    }

    Ok(())
}

#[test]
fn test_pre_auto_gc() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    // See https://stackoverflow.com/q/3433653/344643, it's hard to get the
    // `pre-auto-gc` hook to be invoked at all. We'll just invoke the hook
    // directly to make sure that it's installed properly.
    let output = Command::new(
        get_sh()
            .ok_or_else(|| eyre!("Could not get sh"))
            .wrap_err("bash needed to run pre-auto-gc")?,
    )
    .arg("-c")
    // Always use a unix style path here, as we are handing it to bash (even on Windows).
    .arg("./.git/hooks/pre-auto-gc")
    .current_dir(&git.repo_path)
    .env_clear()
    .env("PATH", git.get_path_for_env())
    .output()?;

    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        output.status.success(),
        "Pre-auto-gc hook failed with exit code {:?}:
            Stdout:
            {}
            Stderr:
            {}",
        output.status.code(),
        stdout,
        stderr
    );
    insta::assert_snapshot!(stdout, @r###"
    branchless: collecting garbage
    branchless: 0 dangling references deleted
    "###);

    Ok(())
}

#[test]
fn test_merge_commit_recorded() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.run(&["checkout", "HEAD^"])?;
    git.commit_file("test2", 2)?;
    git.run(&["merge", &test1_oid.to_string()])?;

    let effects = Effects::new_suppress_for_test(Glyphs::text());
    let repo = git.get_repo()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&effects, &repo, &event_log_db)?;
    let events: Vec<Event> = get_event_replayer_events(&event_replayer)
        .iter()
        .cloned()
        .map(redact_event_timestamp)
        .map(redact_event_id)
        .collect();
    insta::assert_debug_snapshot!(events, @r###"
    [
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: Id(
                1,
            ),
            ref_name: ReferenceName(
                "HEAD",
            ),
            old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
            new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            message: None,
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: Id(
                1,
            ),
            ref_name: ReferenceName(
                "refs/heads/master",
            ),
            old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
            new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            message: None,
        },
        CommitEvent {
            timestamp: 0.0,
            event_tx_id: Id(
                2,
            ),
            commit_oid: NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: Id(
                3,
            ),
            ref_name: ReferenceName(
                "HEAD",
            ),
            old_oid: 0000000000000000000000000000000000000000,
            new_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
            message: None,
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: Id(
                4,
            ),
            ref_name: ReferenceName(
                "HEAD",
            ),
            old_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            new_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
            message: None,
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: Id(
                5,
            ),
            ref_name: ReferenceName(
                "HEAD",
            ),
            old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
            new_oid: fe65c1fe15584744e649b2c79d4cf9b0d878f92e,
            message: None,
        },
        CommitEvent {
            timestamp: 0.0,
            event_tx_id: Id(
                6,
            ),
            commit_oid: NonZeroOid(fe65c1fe15584744e649b2c79d4cf9b0d878f92e),
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: Id(
                8,
            ),
            ref_name: ReferenceName(
                "HEAD",
            ),
            old_oid: fe65c1fe15584744e649b2c79d4cf9b0d878f92e,
            new_oid: 91a5ccb4feefba38b0ffa4911c5c3f6c225f662e,
            message: None,
        },
        CommitEvent {
            timestamp: 0.0,
            event_tx_id: Id(
                9,
            ),
            commit_oid: NonZeroOid(91a5ccb4feefba38b0ffa4911c5c3f6c225f662e),
        },
    ]
    "###);

    Ok(())
}

#[test]
fn test_git_am_recorded() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.run(&["format-patch", "HEAD^"])?;
    git.run(&["reset", "--hard", "HEAD^"])?;

    {
        let (stdout, _stderr) = git.run(&["am", "0001-create-test1.txt.patch"])?;
        insta::assert_snapshot!(stdout, @r###"
        Applying: create test1.txt
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | @ 047b7ad create test1.txt
        |
        o 62fc20d create test1.txt
        "###);
    }

    git.run(&["reset", "--hard", "HEAD^"])?;
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (master) create initial.txt
        |\
        | o 047b7ad create test1.txt
        |
        o 62fc20d create test1.txt
        "###);
    }

    Ok(())
}
