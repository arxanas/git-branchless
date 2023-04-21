use git_branchless_testing::{
    make_git_with_remote_repo, GitInitOptions, GitRunOptions, GitWrapperWithRemoteRepo,
};
use lib::git::GitVersion;

/// Minimum version due to changes in the output of `git push`.
const MIN_VERSION: GitVersion = GitVersion(2, 36, 0);

fn redact_remotes(output: String) -> String {
    output
        .lines()
        .map(|line| {
            if line.contains("To file://") {
                "To: file://<remote>\n".to_string()
            } else if line.contains("From file://") {
                "From: file://<remote>\n".to_string()
            } else if line.contains("error: failed to push some refs to 'file://") {
                "error: failed to push some refs to 'file://<remote>'\n".to_string()
            } else {
                format!("{line}\n")
            }
        })
        .collect()
}

#[test]
fn test_submit() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _guard,
        original_repo,
        cloned_repo,
    } = make_git_with_remote_repo()?;

    if original_repo.get_version()? < MIN_VERSION {
        return Ok(());
    }

    {
        original_repo.init_repo()?;
        original_repo.commit_file("test1", 1)?;
        original_repo.commit_file("test2", 2)?;

        original_repo.clone_repo_into(&cloned_repo, &[])?;
    }

    cloned_repo.init_repo_with_options(&GitInitOptions {
        make_initial_commit: false,
        ..Default::default()
    })?;
    cloned_repo.run(&["checkout", "-b", "foo"])?;
    cloned_repo.commit_file("test3", 3)?;
    cloned_repo.run(&["checkout", "-b", "bar", "master"])?;
    cloned_repo.commit_file("test4", 4)?;
    cloned_repo.run(&["checkout", "-b", "qux"])?;
    cloned_repo.commit_file("test5", 5)?;
    {
        let (stdout, stderr) = cloned_repo.run(&["submit"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        Skipped 2 branches (not yet on remote): bar, qux
        These branches were skipped because they were not already associated with a remote repository. To
        create and push them, retry this operation with the --create option.
        "###);
    }

    {
        let (stdout, stderr) = cloned_repo.run(&["submit", "--create"])?;
        let stderr = redact_remotes(stderr);
        insta::assert_snapshot!(stderr, @r###"
        branchless: processing 1 update: branch bar
        branchless: processing 1 update: branch qux
        To: file://<remote>
         * [new branch]      bar -> bar
         * [new branch]      qux -> qux
        "###);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> push --set-upstream origin bar qux
        branch 'bar' set up to track 'origin/bar'.
        branch 'qux' set up to track 'origin/qux'.
        Created 2 branches: bar, qux
        "###);
    }

    {
        let (stdout, stderr) = original_repo.run(&["branch", "-a"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
          bar
        * master
          qux
        "###);
    }

    cloned_repo.run(&["commit", "--amend", "-m", "updated message"])?;
    {
        let (stdout, stderr) = cloned_repo.run(&["submit"])?;
        let stderr = redact_remotes(stderr);
        insta::assert_snapshot!(stderr, @r###"
        From: file://<remote>
         * branch            bar        -> FETCH_HEAD
         * branch            qux        -> FETCH_HEAD
        branchless: processing 1 update: branch qux
        To: file://<remote>
         + 20230db...bae8307 qux -> qux (forced update)
        "###);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> fetch origin refs/heads/bar refs/heads/qux
        branchless: running command: <git-executable> push --force-with-lease origin qux
        Pushed 1 branch: qux
        Skipped 1 branch (already up-to-date): bar
        "###);
    }

    // Test case where there are no remote branches to create, even though user has asked for `--create`
    {
        let (stdout, stderr) = cloned_repo.run(&["submit", "--create"])?;
        let stderr = redact_remotes(stderr);
        insta::assert_snapshot!(stderr, @r###"
        From: file://<remote>
         * branch            bar        -> FETCH_HEAD
         * branch            qux        -> FETCH_HEAD
        "###);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> fetch origin refs/heads/bar refs/heads/qux
        Skipped 2 branches (already up-to-date): bar, qux
        "###);
    }

    Ok(())
}

#[test]
fn test_submit_multiple_remotes() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _guard,
        original_repo,
        cloned_repo,
    } = make_git_with_remote_repo()?;

    if original_repo.get_version()? < MIN_VERSION {
        return Ok(());
    }

    {
        original_repo.init_repo()?;
        original_repo.commit_file("test1", 1)?;
        original_repo.commit_file("test2", 2)?;

        original_repo.clone_repo_into(&cloned_repo, &[])?;
    }

    cloned_repo.init_repo_with_options(&GitInitOptions {
        make_initial_commit: false,
        ..Default::default()
    })?;
    cloned_repo.run(&["checkout", "-b", "foo"])?;
    cloned_repo.commit_file("test3", 3)?;
    cloned_repo.run(&["branch", "--unset-upstream", "master"])?;
    cloned_repo.run(&["remote", "add", "other-repo", "file://dummy-file"])?;

    {
        let (stdout, stderr) = cloned_repo.branchless_with_options(
            "submit",
            &["--create"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        No upstream repository was associated with branch master and no value was
        specified for `remote.pushDefault`, so cannot push these branches: foo
        Configure a value with: git config remote.pushDefault <remote>
        These remotes are available: origin, other-repo
        "###);
    }

    Ok(())
}

#[test]
fn test_submit_existing_branch() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _guard,
        original_repo,
        cloned_repo,
    } = make_git_with_remote_repo()?;

    if original_repo.get_version()? < MIN_VERSION {
        return Ok(());
    }

    original_repo.init_repo()?;
    original_repo.commit_file("test1", 1)?;
    original_repo.commit_file("test2", 2)?;

    original_repo.clone_repo_into(&cloned_repo, &[])?;
    cloned_repo.init_repo_with_options(&GitInitOptions {
        make_initial_commit: false,
        ..Default::default()
    })?;

    original_repo.run(&["checkout", "-b", "feature"])?;
    original_repo.commit_file("test3", 3)?;
    cloned_repo.run(&["checkout", "-b", "feature"])?;
    cloned_repo.commit_file("test4", 4)?;

    {
        let (stdout, stderr) = cloned_repo.branchless_with_options(
            "submit",
            &["--create"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        let stderr = redact_remotes(stderr);
        insta::assert_snapshot!(stderr, @r###"
        To: file://<remote>
         ! [rejected]        feature -> feature (fetch first)
        error: failed to push some refs to 'file://<remote>'
        hint: Updates were rejected because the remote contains work that you do
        hint: not have locally. This is usually caused by another repository pushing
        hint: to the same ref. You may want to first integrate the remote changes
        hint: (e.g., 'git pull ...') before pushing again.
        hint: See the 'Note about fast-forwards' in 'git push --help' for details.
        "###);
        insta::assert_snapshot!(stdout, @"branchless: running command: <git-executable> push --set-upstream origin feature
");
    }

    {
        cloned_repo.run(&["fetch"])?;
        let (stdout, _stderr) = cloned_repo.run(&["branch", "--all", "--verbose"])?;
        insta::assert_snapshot!(stdout, @r###"
        * feature                f57e36f create test4.txt
          master                 96d1c37 create test2.txt
          remotes/origin/HEAD    -> origin/master
          remotes/origin/feature 70deb1e create test3.txt
          remotes/origin/master  96d1c37 create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_submit_up_to_date_branch() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _guard,
        original_repo,
        cloned_repo,
    } = make_git_with_remote_repo()?;

    if original_repo.get_version()? < MIN_VERSION {
        return Ok(());
    }

    {
        original_repo.init_repo()?;
        original_repo.commit_file("test1", 1)?;
        original_repo.commit_file("test2", 2)?;
        original_repo.clone_repo_into(&cloned_repo, &[])?;
        cloned_repo.init_repo_with_options(&GitInitOptions {
            make_initial_commit: false,
            ..Default::default()
        })?;
    }

    cloned_repo.run(&["checkout", "-b", "feature"])?;
    cloned_repo.commit_file("test3", 3)?;

    {
        let (stdout, stderr) = cloned_repo.run(&["submit", "--create", "feature"])?;
        let stderr = redact_remotes(stderr);
        insta::assert_snapshot!(stderr, @r###"
        branchless: processing 1 update: branch feature
        To: file://<remote>
         * [new branch]      feature -> feature
        "###);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> push --set-upstream origin feature
        branch 'feature' set up to track 'origin/feature'.
        Created 1 branch: feature
        "###);
    }

    cloned_repo.detach_head()?;
    {
        let (stdout, stderr) = cloned_repo.run(&["submit", "feature"])?;
        let stderr = redact_remotes(stderr);
        insta::assert_snapshot!(stderr, @r###"
        From: file://<remote>
         * branch            feature    -> FETCH_HEAD
        "###);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> fetch origin refs/heads/feature
        Skipped 1 branch (already up-to-date): feature
        "###);
    }

    Ok(())
}
