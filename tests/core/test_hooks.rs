use anyhow::Context;
use branchless::testing::with_git;
use branchless::util::get_sh;
use std::process::Command;

fn preprocess_stderr(stderr: String) -> String {
    stderr
        // Interactive progress displays may update the same line multiple times
        // with a carriage return before emitting the final newline.
        .replace("\r", "\n")
        // Window pseudo console may emit EL 'Erase in Line' VT sequences.
        .replace("\x1b[K", "")
        .lines()
        .filter(|line| {
            !line.chars().all(|c| c.is_whitespace()) && !line.starts_with("branchless: processing")
        })
        .map(|line| line.to_owned() + "\n")
        .collect::<Vec<_>>()
        .join("")
}

#[test]
fn test_abandoned_commit_message() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.commit_file("test1", 1)?;

        {
            let (_stdout, stderr) = git.run(&["commit", "--amend", "-m", "amend test1"])?;
            let stderr = preprocess_stderr(stderr);
            assert_eq!(stderr, "");
        }

        git.commit_file("test2", 2)?;
        git.run(&["checkout", "HEAD^"])?;
        git.run(&["branch", "-f", "master"])?;

        {
            let (_stdout, stderr) = git.run(&["commit", "--amend", "-m", "amend test1 again"])?;
            let stderr = preprocess_stderr(stderr);
            insta::assert_snapshot!(stderr, @r###"
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
    })
}

#[test]
fn test_abandoned_branch_message() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.commit_file("test1", 1)?;
        git.run(&["branch", "abc"])?;
        git.detach_head()?;

        {
            let (_stdout, stderr) = git.run(&["commit", "--amend", "-m", "amend test1"])?;
            let stderr = preprocess_stderr(stderr);
            insta::assert_snapshot!(stderr, @r###"
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
    })
}

#[test]
fn test_fixup_no_abandoned_commit_message() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.detach_head()?;
        git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        git.run(&["commit", "--amend", "-m", "fixup! create test1.txt"])?;
        git.commit_file("test3", 3)?;
        git.run(&["commit", "--amend", "-m", "fixup! create test1.txt"])?;

        {
            let (_stdout, stderr) = git.run(&["rebase", "-i", "master", "--autosquash"])?;
            let stderr = preprocess_stderr(stderr);
            insta::assert_snapshot!(stderr, @r###"
            Rebasing (2/3)
            Rebasing (3/3)
            Successfully rebased and updated detached HEAD.
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_rebase_individual_commit() -> anyhow::Result<()> {
    with_git(|git| {
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
            let stderr = preprocess_stderr(stderr);
            insta::assert_snapshot!(stderr, @r###"
            Rebasing (1/1)
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
    })
}

#[test]
fn test_interactive_rebase_noop() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.detach_head()?;
        git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;

        {
            let (_stdout, stderr) = git.run(&["rebase", "-i", "master"])?;
            let stderr = preprocess_stderr(stderr);
            insta::assert_snapshot!(stderr, @"Successfully rebased and updated detached HEAD.
");
        }

        Ok(())
    })
}

#[test]
fn test_pre_auto_gc() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;

        // See https://stackoverflow.com/q/3433653/344643, it's hard to get the
        // `pre-auto-gc` hook to be invoked at all. We'll just invoke the hook
        // directly to make sure that it's installed properly.
        let output = Command::new(get_sh().context("bash needed to run pre-auto-gc")?)
            .arg("-c")
            // Always use a unix style path here, as we are handing it to bash (even on Windows).
            .arg("./.git/hooks/pre-auto-gc")
            .current_dir(&git.repo_path)
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
    })
}
