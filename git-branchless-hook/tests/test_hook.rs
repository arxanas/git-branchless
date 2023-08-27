use lib::testing::{make_git, GitRunOptions};

#[test]
fn test_is_rebase_underway() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    let repo = git.get_repo()?;
    assert!(!repo.is_rebase_underway()?);

    let oid1 = git.commit_file_with_contents("test", 1, "foo")?;
    git.run(&["checkout", "HEAD^"])?;
    git.commit_file_with_contents("test", 1, "bar")?;
    git.run_with_options(
        &["rebase", &oid1.to_string()],
        &GitRunOptions {
            expected_exit_code: 1,
            ..Default::default()
        },
    )?;
    assert!(repo.is_rebase_underway()?);

    Ok(())
}

#[test]
fn test_rebase_no_process_new_commits_until_conclusion() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;

    // Ensure commits aren't preserved if the rebase is aborted.
    {
        git.run_with_options(
            &["rebase", "master", "--force", "--exec", "exit 1"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        git.run(&[
            "commit",
            "--amend",
            "--message",
            "this commit shouldn't show up in the smartlog",
        ])?;
        git.commit_file("test3", 3)?;
        git.run(&["rebase", "--abort"])?;

        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ 96d1c37 create test2.txt
        "###);
        }
    }

    // Ensure commits are preserved if the rebase succeeds.
    {
        git.run(&["checkout", "HEAD^"])?;

        {
            let (stdout, stderr) = git.run_with_options(
                &["rebase", "master", "--force", "--exec", "exit 1"],
                &GitRunOptions {
                    expected_exit_code: 1,
                    ..Default::default()
                },
            )?;

            // As of `38c541ce94048cf72aa4f465be9314423a57f445` (Git >=v2.36.0),
            // `git checkout` is called in fewer cases, which affects the stderr
            // output for the test.
            let stderr: String = stderr
                .lines()
                .filter_map(|line| {
                    if line.starts_with("branchless:") {
                        None
                    } else {
                        Some(format!("{line}\n"))
                    }
                })
                .collect();

            insta::assert_snapshot!(stderr, @r###"
            Executing: exit 1
            warning: execution failed: exit 1
            You can fix the problem, and then run

              git rebase --continue


            "###);
            insta::assert_snapshot!(stdout, @"");
        }

        git.commit_file("test4", 4)?;
        {
            let (stdout, stderr) = git.run(&["rebase", "--continue"])?;
            insta::assert_snapshot!(stderr, @r###"
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
            insta::assert_snapshot!(stdout, @"");
        }

        // Switch away to make sure that the new commit isn't visible just
        // because it's reachable from `HEAD`.
        git.run(&["checkout", &test2_oid.to_string()])?;

        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            O f777ecc (master) create initial.txt
            |\
            | o 047b7ad create test1.txt
            | |
            | o ecab41f create test4.txt
            |
            x 62fc20d (rewritten as 047b7ad7) create test1.txt
            |
            @ 96d1c37 create test2.txt
            hint: there is 1 abandoned commit in your commit graph
            hint: to fix this, run: git restack
            hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
            "###);
        }
    }

    Ok(())
}
