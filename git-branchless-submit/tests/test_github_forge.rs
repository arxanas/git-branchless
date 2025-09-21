use std::collections::HashMap;
use std::fs;

use git_branchless_submit::github::testing::MockGithubClient;
use lib::git::{GitVersion, SerializedNonZeroOid};
use lib::testing::{
    make_git_with_remote_repo, remove_rebase_lines, Git, GitRunOptions, GitWrapperWithRemoteRepo,
};

/// Minimum version due to changes in the output of `git push`.
const MIN_VERSION: GitVersion = GitVersion(2, 36, 0);

fn mock_env(git: &Git) -> HashMap<String, String> {
    git.get_base_env(0)
        .into_iter()
        .map(|(k, v)| {
            (
                k.to_str().unwrap().to_string(),
                v.to_str().unwrap().to_string(),
            )
        })
        .chain([(
            git_branchless_submit::github::MOCK_REMOTE_REPO_PATH_ENV_KEY.to_string(),
            git.repo_path.clone().to_str().unwrap().to_owned(),
        )])
        .collect()
}

fn dump_state(local_repo: &Git, remote_repo: &Git) -> eyre::Result<String> {
    let local_repo_smartlog: String = local_repo.smartlog()?;
    let remote_repo_smartlog = remote_repo.smartlog()?;
    let client = MockGithubClient {
        remote_repo_path: remote_repo.repo_path.clone(),
    };
    let pull_request_info_path = client.state_path();
    let pull_request_info =
        fs::read_to_string(pull_request_info_path).unwrap_or_else(|err| format!("Error: {err}"));
    let state = format!(
        "\
Local state:
{local_repo_smartlog}

Remote state:
{remote_repo_smartlog}

Pull request info:
{pull_request_info}
"
    );
    Ok(state)
}

fn rebase_and_merge(remote_repo: &Git, branch_name: &str) -> eyre::Result<()> {
    remote_repo.run(&["cherry-pick", branch_name])?;
    remote_repo.run(&["branch", "-f", branch_name, "HEAD"])?;
    let head_info = remote_repo.get_repo()?.get_head_info()?;
    let head_oid = head_info.oid.unwrap();
    let client = MockGithubClient {
        remote_repo_path: remote_repo.repo_path.clone(),
    };
    client.with_state_mut(|state| {
        state
            .pull_requests
            .get_mut(branch_name)
            .unwrap()
            .head_ref_oid = SerializedNonZeroOid(head_oid);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn test_github_forge_reorder_commits() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _temp_dir,
        original_repo: remote_repo,
        cloned_repo: local_repo,
    } = make_git_with_remote_repo()?;
    if remote_repo.get_version()? < MIN_VERSION {
        return Ok(());
    }

    remote_repo.init_repo()?;
    remote_repo.clone_repo_into(&local_repo, &[])?;

    local_repo.detach_head()?;
    local_repo.commit_file("test1", 1)?;
    local_repo.commit_file("test2", 2)?;
    {
        let (stdout, _stderr) = local_repo.branchless_with_options(
            "submit",
            &["--create", "--forge", "github"],
            &GitRunOptions {
                env: mock_env(&remote_repo),
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> push --set-upstream origin mock-github-username/create-test1-txt
        branch 'mock-github-username/create-test1-txt' set up to track 'origin/mock-github-username/create-test1-txt'.
        branchless: running command: <git-executable> push --set-upstream origin mock-github-username/create-test2-txt
        branch 'mock-github-username/create-test2-txt' set up to track 'origin/mock-github-username/create-test2-txt'.
        Updating pull request (title, body) for commit 62fc20d create test1.txt
        branchless: running command: <git-executable> push --force-with-lease origin mock-github-username/create-test1-txt
        Updating pull request (base branch, title, body) for commit 96d1c37 create test2.txt
        branchless: running command: <git-executable> push --force-with-lease origin mock-github-username/create-test2-txt
        Submitted 62fc20d create test1.txt (as mock-github-username/create-test1-txt)
        Submitted 96d1c37 create test2.txt (as mock-github-username/create-test2-txt)
        "###);
    }
    {
        let state = dump_state(&local_repo, &remote_repo)?;
        insta::assert_snapshot!(state, @r###"
        Local state:
        O f777ecc (master) create initial.txt
        |
        o 62fc20d (mock-github-username/create-test1-txt) create test1.txt
        |
        @ 96d1c37 (mock-github-username/create-test2-txt) create test2.txt


        Remote state:
        @ f777ecc (> master) create initial.txt
        |
        o 62fc20d (mock-github-username/create-test1-txt) create test1.txt
        |
        o 96d1c37 (mock-github-username/create-test2-txt) create test2.txt


        Pull request info:
        {
          "pull_request_index": 2,
          "pull_requests": {
            "mock-github-username/create-test1-txt": {
              "number": 1,
              "url": "https://example.com/mock-github-username/mock-github-repo/pulls/1",
              "headRefName": "mock-github-username/create-test1-txt",
              "headRefOid": "62fc20d2a290daea0d52bdc2ed2ad4be6491010e",
              "baseRefName": "master",
              "closed": false,
              "isDraft": false,
              "title": "[1/2] create test1.txt",
              "body": "**Stack:**\n\n* https://example.com/mock-github-username/mock-github-repo/pulls/1\n* https://example.com/mock-github-username/mock-github-repo/pulls/2\n\n\n---\n\ncreate test1.txt\n\n"
            },
            "mock-github-username/create-test2-txt": {
              "number": 2,
              "url": "https://example.com/mock-github-username/mock-github-repo/pulls/2",
              "headRefName": "mock-github-username/create-test2-txt",
              "headRefOid": "96d1c37a3d4363611c49f7e52186e189a04c531f",
              "baseRefName": "mock-github-username/create-test1-txt",
              "closed": false,
              "isDraft": false,
              "title": "[2/2] create test2.txt",
              "body": "**Stack:**\n\n* https://example.com/mock-github-username/mock-github-repo/pulls/1\n* https://example.com/mock-github-username/mock-github-repo/pulls/2\n\n\n---\n\ncreate test2.txt\n\n"
            }
          }
        }
        "###);
    }

    local_repo.branchless(
        "move",
        &["--source", "HEAD", "--dest", "master", "--insert"],
    )?;
    {
        let (stdout, _stderr) = local_repo.branchless_with_options(
            "submit",
            &["--forge", "github"],
            &GitRunOptions {
                env: mock_env(&remote_repo),
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Updating pull request (commit, base branch, title, body) for commit fe65c1f create test2.txt
        branchless: running command: <git-executable> push --force-with-lease origin mock-github-username/create-test2-txt
        Updating pull request (commit, base branch, title, body) for commit 0770943 create test1.txt
        branchless: running command: <git-executable> push --force-with-lease origin mock-github-username/create-test1-txt
        Updated fe65c1f create test2.txt (as mock-github-username/create-test2-txt)
        Updated 0770943 create test1.txt (as mock-github-username/create-test1-txt)
        "###);
    }
    {
        let state = dump_state(&local_repo, &remote_repo)?;
        insta::assert_snapshot!(state, @r###"
        Local state:
        O f777ecc (master) create initial.txt
        |
        @ fe65c1f (> mock-github-username/create-test2-txt) create test2.txt
        |
        o 0770943 (mock-github-username/create-test1-txt) create test1.txt


        Remote state:
        @ f777ecc (> master) create initial.txt
        |
        o fe65c1f (mock-github-username/create-test2-txt) create test2.txt
        |
        o 0770943 (mock-github-username/create-test1-txt) create test1.txt


        Pull request info:
        {
          "pull_request_index": 2,
          "pull_requests": {
            "mock-github-username/create-test1-txt": {
              "number": 1,
              "url": "https://example.com/mock-github-username/mock-github-repo/pulls/1",
              "headRefName": "mock-github-username/create-test1-txt",
              "headRefOid": "07709435a8f6d1566e0091896d130c78acd429dd",
              "baseRefName": "mock-github-username/create-test2-txt",
              "closed": false,
              "isDraft": false,
              "title": "[2/2] create test1.txt",
              "body": "**Stack:**\n\n* https://example.com/mock-github-username/mock-github-repo/pulls/2\n* https://example.com/mock-github-username/mock-github-repo/pulls/1\n\n\n---\n\ncreate test1.txt\n\n"
            },
            "mock-github-username/create-test2-txt": {
              "number": 2,
              "url": "https://example.com/mock-github-username/mock-github-repo/pulls/2",
              "headRefName": "mock-github-username/create-test2-txt",
              "headRefOid": "fe65c1fe15584744e649b2c79d4cf9b0d878f92e",
              "baseRefName": "master",
              "closed": false,
              "isDraft": false,
              "title": "[1/2] create test2.txt",
              "body": "**Stack:**\n\n* https://example.com/mock-github-username/mock-github-repo/pulls/2\n* https://example.com/mock-github-username/mock-github-repo/pulls/1\n\n\n---\n\ncreate test2.txt\n\n"
            }
          }
        }
        "###);
    }

    Ok(())
}

#[test]
fn test_github_forge_mock_client_closes_pull_requests() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _temp_dir,
        original_repo: remote_repo,
        cloned_repo: local_repo,
    } = make_git_with_remote_repo()?;
    if remote_repo.get_version()? < MIN_VERSION {
        return Ok(());
    }

    remote_repo.init_repo()?;
    remote_repo.clone_repo_into(&local_repo, &[])?;

    local_repo.detach_head()?;
    local_repo.commit_file("test1", 1)?;
    local_repo.commit_file("test2", 2)?;

    {
        let (stdout, _stderr) = local_repo.branchless_with_options(
            "submit",
            &["--forge", "github", "--create"],
            &GitRunOptions {
                env: mock_env(&remote_repo),
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> push --set-upstream origin mock-github-username/create-test1-txt
        branch 'mock-github-username/create-test1-txt' set up to track 'origin/mock-github-username/create-test1-txt'.
        branchless: running command: <git-executable> push --set-upstream origin mock-github-username/create-test2-txt
        branch 'mock-github-username/create-test2-txt' set up to track 'origin/mock-github-username/create-test2-txt'.
        Updating pull request (title, body) for commit 62fc20d create test1.txt
        branchless: running command: <git-executable> push --force-with-lease origin mock-github-username/create-test1-txt
        Updating pull request (base branch, title, body) for commit 96d1c37 create test2.txt
        branchless: running command: <git-executable> push --force-with-lease origin mock-github-username/create-test2-txt
        Submitted 62fc20d create test1.txt (as mock-github-username/create-test1-txt)
        Submitted 96d1c37 create test2.txt (as mock-github-username/create-test2-txt)
        "###);
    }
    {
        let state = dump_state(&local_repo, &remote_repo)?;
        insta::assert_snapshot!(state, @r###"
        Local state:
        O f777ecc (master) create initial.txt
        |
        o 62fc20d (mock-github-username/create-test1-txt) create test1.txt
        |
        @ 96d1c37 (mock-github-username/create-test2-txt) create test2.txt


        Remote state:
        @ f777ecc (> master) create initial.txt
        |
        o 62fc20d (mock-github-username/create-test1-txt) create test1.txt
        |
        o 96d1c37 (mock-github-username/create-test2-txt) create test2.txt


        Pull request info:
        {
          "pull_request_index": 2,
          "pull_requests": {
            "mock-github-username/create-test1-txt": {
              "number": 1,
              "url": "https://example.com/mock-github-username/mock-github-repo/pulls/1",
              "headRefName": "mock-github-username/create-test1-txt",
              "headRefOid": "62fc20d2a290daea0d52bdc2ed2ad4be6491010e",
              "baseRefName": "master",
              "closed": false,
              "isDraft": false,
              "title": "[1/2] create test1.txt",
              "body": "**Stack:**\n\n* https://example.com/mock-github-username/mock-github-repo/pulls/1\n* https://example.com/mock-github-username/mock-github-repo/pulls/2\n\n\n---\n\ncreate test1.txt\n\n"
            },
            "mock-github-username/create-test2-txt": {
              "number": 2,
              "url": "https://example.com/mock-github-username/mock-github-repo/pulls/2",
              "headRefName": "mock-github-username/create-test2-txt",
              "headRefOid": "96d1c37a3d4363611c49f7e52186e189a04c531f",
              "baseRefName": "mock-github-username/create-test1-txt",
              "closed": false,
              "isDraft": false,
              "title": "[2/2] create test2.txt",
              "body": "**Stack:**\n\n* https://example.com/mock-github-username/mock-github-repo/pulls/1\n* https://example.com/mock-github-username/mock-github-repo/pulls/2\n\n\n---\n\ncreate test2.txt\n\n"
            }
          }
        }
        "###);
    }

    rebase_and_merge(&remote_repo, "mock-github-username/create-test1-txt")?;
    {
        let (stdout, _stderr) = local_repo.branchless("sync", &["--pull"])?;
        let stdout = remove_rebase_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> fetch --all
        Fast-forwarding branch master to 047b7ad create test1.txt
        Attempting rebase in-memory...
        [1/2] Skipped commit (was already applied upstream): 62fc20d create test1.txt
        [2/2] Committed as: fa46633 create test2.txt
        branchless: running command: <git-executable> checkout mock-github-username/create-test2-txt
        Your branch and 'origin/mock-github-username/create-test2-txt' have diverged,
        and have 2 and 2 different commits each, respectively.
        In-memory rebase succeeded.
        Synced 62fc20d create test1.txt
        "###);
    }
    {
        let stdout = local_repo.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 047b7ad (master) create test1.txt
        |
        @ fa46633 (> mock-github-username/create-test2-txt) create test2.txt
        "###);
    }

    {
        let (stdout, _stderr) = local_repo.branchless_with_options(
            "submit",
            &["--forge", "github"],
            &GitRunOptions {
                env: mock_env(&remote_repo),
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Updating pull request (commit, base branch, title, body) for commit fa46633 create test2.txt
        branchless: running command: <git-executable> push --force-with-lease origin mock-github-username/create-test2-txt
        Updated fa46633 create test2.txt (as mock-github-username/create-test2-txt)
        "###);
    }
    {
        let state = dump_state(&local_repo, &remote_repo)?;
        insta::assert_snapshot!(state, @r###"
        Local state:
        :
        O 047b7ad (master) create test1.txt
        |
        @ fa46633 (> mock-github-username/create-test2-txt) create test2.txt


        Remote state:
        :
        @ 047b7ad (> master, mock-github-username/create-test1-txt) create test1.txt
        |
        o fa46633 (mock-github-username/create-test2-txt) create test2.txt


        Pull request info:
        {
          "pull_request_index": 2,
          "pull_requests": {
            "mock-github-username/create-test1-txt": {
              "number": 1,
              "url": "https://example.com/mock-github-username/mock-github-repo/pulls/1",
              "headRefName": "mock-github-username/create-test1-txt",
              "headRefOid": "047b7ad7790bd443d78ea38854cecb9d9cc7fb7a",
              "baseRefName": "master",
              "closed": true,
              "isDraft": false,
              "title": "[1/2] create test1.txt",
              "body": "**Stack:**\n\n* https://example.com/mock-github-username/mock-github-repo/pulls/1\n* https://example.com/mock-github-username/mock-github-repo/pulls/2\n\n\n---\n\ncreate test1.txt\n\n"
            },
            "mock-github-username/create-test2-txt": {
              "number": 2,
              "url": "https://example.com/mock-github-username/mock-github-repo/pulls/2",
              "headRefName": "mock-github-username/create-test2-txt",
              "headRefOid": "fa46633239bfa767036e41a77b67258286e4ddb9",
              "baseRefName": "master",
              "closed": false,
              "isDraft": false,
              "title": "[1/1] create test2.txt",
              "body": "**Stack:**\n\n* https://example.com/mock-github-username/mock-github-repo/pulls/2\n\n\n---\n\ncreate test2.txt\n\n"
            }
          }
        }
        "###);
    }

    Ok(())
}

#[test]
fn test_github_forge_no_include_unsubmitted_commits_in_stack() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _temp_dir,
        original_repo: remote_repo,
        cloned_repo: local_repo,
    } = make_git_with_remote_repo()?;
    if remote_repo.get_version()? < MIN_VERSION {
        return Ok(());
    }

    remote_repo.init_repo()?;
    remote_repo.clone_repo_into(&local_repo, &[])?;

    local_repo.detach_head()?;
    local_repo.commit_file("test1", 1)?;
    local_repo.commit_file("test2", 2)?;
    local_repo.commit_file("test3", 3)?;
    {
        let stdout = local_repo.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        @ 70deb1e create test3.txt
        "###);
    }

    {
        let (stdout, _stderr) = local_repo.branchless_with_options(
            "submit",
            &["--forge", "github", "--create", "HEAD^^"],
            &GitRunOptions {
                env: mock_env(&remote_repo),
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> push --set-upstream origin mock-github-username/create-test1-txt
        branch 'mock-github-username/create-test1-txt' set up to track 'origin/mock-github-username/create-test1-txt'.
        Updating pull request (title, body) for commit 62fc20d create test1.txt
        branchless: running command: <git-executable> push --force-with-lease origin mock-github-username/create-test1-txt
        Submitted 62fc20d create test1.txt (as mock-github-username/create-test1-txt)
        "###);
    }
    {
        let state = dump_state(&local_repo, &remote_repo)?;
        insta::assert_snapshot!(state, @r###"
        Local state:
        O f777ecc (master) create initial.txt
        |
        o 62fc20d (mock-github-username/create-test1-txt) create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        @ 70deb1e create test3.txt


        Remote state:
        @ f777ecc (> master) create initial.txt
        |
        o 62fc20d (mock-github-username/create-test1-txt) create test1.txt


        Pull request info:
        {
          "pull_request_index": 1,
          "pull_requests": {
            "mock-github-username/create-test1-txt": {
              "number": 1,
              "url": "https://example.com/mock-github-username/mock-github-repo/pulls/1",
              "headRefName": "mock-github-username/create-test1-txt",
              "headRefOid": "62fc20d2a290daea0d52bdc2ed2ad4be6491010e",
              "baseRefName": "master",
              "closed": false,
              "isDraft": false,
              "title": "[1/1] create test1.txt",
              "body": "**Stack:**\n\n* https://example.com/mock-github-username/mock-github-repo/pulls/1\n\n\n---\n\ncreate test1.txt\n\n"
            }
          }
        }
        "###);
    }

    Ok(())
}

#[test]
fn test_github_forge_multiple_commits_in_pull_request() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _temp_dir,
        original_repo: remote_repo,
        cloned_repo: local_repo,
    } = make_git_with_remote_repo()?;
    if remote_repo.get_version()? < MIN_VERSION {
        return Ok(());
    }

    remote_repo.init_repo()?;
    remote_repo.clone_repo_into(&local_repo, &[])?;

    local_repo.detach_head()?;
    local_repo.commit_file("test1", 1)?;
    local_repo.commit_file("test2", 2)?;
    local_repo.commit_file("test3", 3)?;
    {
        let stdout = local_repo.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        @ 70deb1e create test3.txt
        "###);
    }

    {
        let (stdout, _stderr) = local_repo.branchless_with_options(
            "submit",
            &["--forge", "github", "--create", "HEAD"],
            &GitRunOptions {
                env: mock_env(&remote_repo),
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> push --set-upstream origin mock-github-username/create-test3-txt
        branch 'mock-github-username/create-test3-txt' set up to track 'origin/mock-github-username/create-test3-txt'.
        Updating pull request (title, body) for commit 70deb1e create test3.txt
        branchless: running command: <git-executable> push --force-with-lease origin mock-github-username/create-test3-txt
        Submitted 70deb1e create test3.txt (as mock-github-username/create-test3-txt)
        "###);
    }
    {
        let state = dump_state(&local_repo, &remote_repo)?;
        insta::assert_snapshot!(state, @r###"
        Local state:
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        @ 70deb1e (mock-github-username/create-test3-txt) create test3.txt


        Remote state:
        @ f777ecc (> master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        o 70deb1e (mock-github-username/create-test3-txt) create test3.txt


        Pull request info:
        {
          "pull_request_index": 1,
          "pull_requests": {
            "mock-github-username/create-test3-txt": {
              "number": 1,
              "url": "https://example.com/mock-github-username/mock-github-repo/pulls/1",
              "headRefName": "mock-github-username/create-test3-txt",
              "headRefOid": "70deb1e28791d8e7dd5a1f0c871a51b91282562f",
              "baseRefName": "master",
              "closed": false,
              "isDraft": false,
              "title": "[1/1] create test3.txt",
              "body": "**Stack:**\n\n* https://example.com/mock-github-username/mock-github-repo/pulls/1\n\n\n---\n\ncreate test3.txt\n\n"
            }
          }
        }
        "###);
    }

    Ok(())
}
