use anyhow::Context;
use branchless::testing::make_git;
use branchless::util::get_sh;
use std::process::Command;

#[test]
fn test_abandoned_commit_message() -> anyhow::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    git.commit_file("test1", 1)?;

    {
        let (_stdout, stderr) = git.run(&["commit", "--amend", "-m", "amend test1"])?;
        insta::assert_snapshot!(stderr, @r###"
        branchless: processing 2 updates: ref HEAD, branch master
        branchless: processed commit: 9e8dbe91 amend test1
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
        branchless: processed commit: c1e22fd6 amend test1 again
        branchless: processing 1 rewritten commit
        branchless: This operation abandoned 1 commit and 1 branch (master)!
        branchless: Consider running one of the following:
        branchless:   - git restack: re-apply the abandoned commits/branches
        branchless:     (this is most likely what you want to do)
        branchless:   - git smartlog: assess the situation
        branchless:   - git hide [<commit>...]: hide the commits from the smartlog
        branchless:   - git undo: undo the operation
        branchless:   - git config branchless.restack.warnAbandoned false: suppress this message
        "###);
    }

    Ok(())
}

#[test]
fn test_abandoned_branch_message() -> anyhow::Result<()> {
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
        branchless: processed commit: 9e8dbe91 amend test1
        branchless: processing 1 rewritten commit
        branchless: This operation abandoned 2 branches (abc, master)!
        branchless: Consider running one of the following:
        branchless:   - git restack: re-apply the abandoned commits/branches
        branchless:     (this is most likely what you want to do)
        branchless:   - git smartlog: assess the situation
        branchless:   - git hide [<commit>...]: hide the commits from the smartlog
        branchless:   - git undo: undo the operation
        branchless:   - git config branchless.restack.warnAbandoned false: suppress this message
        "###);
    }

    Ok(())
}

#[test]
fn test_fixup_no_abandoned_commit_message() -> anyhow::Result<()> {
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

    {
        let (_stdout, stderr) = git.run(&["rebase", "-i", "master", "--autosquash"])?;
        insta::assert_snapshot!(stderr, @r###"
        branchless: processing 1 update: ref HEAD
        branchless: processed commit: a84541d7 # This is a combination of 2 commits. # This is the 1st commit message:
        branchless: processing 1 update: ref HEAD
        branchless: processed commit: 7f023a10 create test1.txt
        branchless: processing 3 rewritten commits
        Successfully rebased and updated detached HEAD.
        "###);
    }

    Ok(())
}

#[test]
fn test_rebase_individual_commit() -> anyhow::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.run(&["checkout", "HEAD^"])?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;

    {
        let (_stdout, stderr) = git.run(&["rebase", "master", "HEAD^"])?;
        insta::assert_snapshot!(stderr, @r###"
        branchless: processing 1 update: ref HEAD
        branchless: processed commit: f8d9985b create test2.txt
        branchless: processing 1 rewritten commit
        branchless: This operation abandoned 1 commit!
        branchless: Consider running one of the following:
        branchless:   - git restack: re-apply the abandoned commits/branches
        branchless:     (this is most likely what you want to do)
        branchless:   - git smartlog: assess the situation
        branchless:   - git hide [<commit>...]: hide the commits from the smartlog
        branchless:   - git undo: undo the operation
        branchless:   - git config branchless.restack.warnAbandoned false: suppress this message
        Successfully rebased and updated detached HEAD.
        "###);
    }

    Ok(())
}

#[test]
fn test_interactive_rebase_noop() -> anyhow::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    {
        let (_stdout, stderr) = git.run(&["rebase", "-i", "master"])?;
        insta::assert_snapshot!(stderr, @"Successfully rebased and updated detached HEAD.
");
    }

    Ok(())
}

#[test]
fn test_pre_auto_gc() -> anyhow::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    // See https://stackoverflow.com/q/3433653/344643, it's hard to get the
    // `pre-auto-gc` hook to be invoked at all. We'll just invoke the hook
    // directly to make sure that it's installed properly.
    let output = Command::new(get_sh().context("bash needed to run pre-auto-gc")?)
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
    insta::assert_snapshot!(stdout, @"branchless: collecting garbage
");

    Ok(())
}
