use branchless::testing::{with_git, GitRunOptions};

/// Remove some of the output from `git rebase`, as it seems to be
/// non-deterministic as to whether or not it appears.
fn remove_rebase_lines(output: String) -> String {
    output
        .lines()
        .filter(|line| !line.contains("First, rewinding head") && !line.contains("Applying:"))
        .map(|line| format!("{}\n", line))
        .collect()
}

#[test]
fn test_restack_amended_commit() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

        git.detach_head()?;
        git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        git.commit_file("test3", 3)?;
        git.run(&["checkout", "HEAD^^"])?;
        git.run(&["commit", "--amend", "-m", "amend test1.txt"])?;

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            O f777ecc9 (master) create initial.txt
            |\
            | @ 024c35ce amend test1.txt
            |
            x 62fc20d2 (rewritten as 024c35ce) create test1.txt
            |
            o 96d1c37a create test2.txt
            |
            o 70deb1e2 create test3.txt
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["restack"])?;
            let stdout = remove_rebase_lines(stdout);
            insta::assert_snapshot!(stdout, @r###"
            branchless: <git-executable> rebase 62fc20d2a290daea0d52bdc2ed2ad4be6491010e 96d1c37a3d4363611c49f7e52186e189a04c531f --onto 024c35ce32dae6b12e981963465ee8a62b7eff9b --committer-date-is-author-date
            branchless: <git-executable> rebase 96d1c37a3d4363611c49f7e52186e189a04c531f 70deb1e28791d8e7dd5a1f0c871a51b91282562f --onto 8cd7de680cafaba911d09f430d2bafb1169d6e65 --committer-date-is-author-date
            branchless: no more abandoned commits to restack
            branchless: no more abandoned branches to restack
            branchless: <git-executable> checkout 024c35ce32dae6b12e981963465ee8a62b7eff9b
            O f777ecc9 (master) create initial.txt
            |
            @ 024c35ce amend test1.txt
            |
            o 8cd7de68 create test2.txt
            |
            o b9a0491a create test3.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_restack_consecutive_rewrites() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

        git.detach_head()?;
        git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        git.commit_file("test3", 3)?;
        git.run(&["checkout", "HEAD^^"])?;
        git.run(&["commit", "--amend", "-m", "amend test1.txt v1"])?;
        git.run(&["commit", "--amend", "-m", "amend test1.txt v2"])?;

        {
            let (stdout, _stderr) = git.run(&["restack"])?;
            let stdout = remove_rebase_lines(stdout);
            insta::assert_snapshot!(stdout, @r###"
            branchless: <git-executable> rebase 62fc20d2a290daea0d52bdc2ed2ad4be6491010e 96d1c37a3d4363611c49f7e52186e189a04c531f --onto 662b451fb905b92404787e024af717ced49e3045 --committer-date-is-author-date
            branchless: <git-executable> rebase 96d1c37a3d4363611c49f7e52186e189a04c531f 70deb1e28791d8e7dd5a1f0c871a51b91282562f --onto 8e9bbde339899eaabf48cf0d8b89d52144db94e1 --committer-date-is-author-date
            branchless: no more abandoned commits to restack
            branchless: no more abandoned branches to restack
            branchless: <git-executable> checkout 662b451fb905b92404787e024af717ced49e3045
            O f777ecc9 (master) create initial.txt
            |
            @ 662b451f amend test1.txt v2
            |
            o 8e9bbde3 create test2.txt
            |
            o 9dc6dd07 create test3.txt
            "###)
        }

        Ok(())
    })
}

#[test]
fn test_move_abandoned_branch() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

        git.commit_file("test1", 1)?;
        git.detach_head()?;
        git.run(&["commit", "--amend", "-m", "amend test1.txt v1"])?;
        git.run(&["commit", "--amend", "-m", "amend test1.txt v2"])?;

        {
            let (stdout, _stderr) = git.run(&["restack"])?;
            let stdout = remove_rebase_lines(stdout);
            insta::assert_snapshot!(stdout, @r###"
            branchless: no more abandoned commits to restack
            branchless: <git-executable> branch -f master 662b451fb905b92404787e024af717ced49e3045
            branchless: no more abandoned branches to restack
            branchless: <git-executable> checkout 662b451fb905b92404787e024af717ced49e3045
            :
            @ 662b451f (master) amend test1.txt v2
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_amended_initial_commit() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

        git.commit_file("test1", 1)?;
        git.run(&["checkout", "HEAD^"])?;
        git.run(&["commit", "--amend", "-m", "new initial commit"])?;

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            @ 9a9f929a new initial commit

            X f777ecc9 (rewritten as 9a9f929a) create initial.txt
            |
            O 62fc20d2 (master) create test1.txt
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["restack"])?;
            let stdout = remove_rebase_lines(stdout);
            insta::assert_snapshot!(stdout, @r###"
            branchless: <git-executable> rebase f777ecc9b0db5ed372b2615695191a8a17f79f24 62fc20d2a290daea0d52bdc2ed2ad4be6491010e --onto 9a9f929a0d4f052ff5d58bedd97b2f761120f8ed --committer-date-is-author-date
            branchless: no more abandoned commits to restack
            branchless: <git-executable> branch -f master 6d85943be6d6e5941d5479f1059d02ebf1c8e307
            branchless: no more abandoned branches to restack
            branchless: <git-executable> checkout 9a9f929a0d4f052ff5d58bedd97b2f761120f8ed
            @ 9a9f929a new initial commit
            |
            O 6d85943b (master) create test1.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_restack_amended_master() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

        git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        git.detach_head()?;
        git.run(&["checkout", "HEAD^"])?;
        git.run(&["commit", "--amend", "-m", "amended test1"])?;

        {
            let (stdout, _stderr) = git.run(&["restack"])?;
            let stdout = remove_rebase_lines(stdout);
            insta::assert_snapshot!(stdout, @r###"
            branchless: <git-executable> rebase 62fc20d2a290daea0d52bdc2ed2ad4be6491010e 96d1c37a3d4363611c49f7e52186e189a04c531f --onto ae94dc2a748bc0965c88fcf3edac2e30074ff7e2 --committer-date-is-author-date
            branchless: no more abandoned commits to restack
            branchless: <git-executable> branch -f master 51452b55e09488387e59770a9f44d999eba27864
            branchless: no more abandoned branches to restack
            branchless: <git-executable> checkout ae94dc2a748bc0965c88fcf3edac2e30074ff7e2
            :
            @ ae94dc2a amended test1
            |
            O 51452b55 (master) create test2.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_restack_aborts_during_rebase_conflict() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.run(&["branch", "foo"])?;
        git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        git.run(&["prev"])?;

        git.write_file("test2", "conflicting test2 contents")?;
        git.run(&["add", "."])?;
        git.run(&["commit", "--amend", "-m", "amend test1 with test2 conflict"])?;
        git.run_with_options(
            &["restack"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;

        Ok(())
    })
}
