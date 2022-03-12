use std::collections::HashMap;

use branchless::testing::{make_git, GitRunOptions};
use itertools::Itertools;

#[test]
fn test_commands() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test", 1)?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 3df4b935 (> master) create test.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["hide", "3df4b935"])?;
        insta::assert_snapshot!(stdout, @r###"
Hid commit: 3df4b935 create test.txt
To unhide this commit, run: git unhide 3df4b935
"###);
    }

    {
        let (stdout, _stderr) = git.run(&["unhide", "3df4b935"])?;
        insta::assert_snapshot!(stdout, @r###"
Unhid commit: 3df4b935 create test.txt
To hide this commit, run: git hide 3df4b935
"###);
    }

    {
        let (stdout, _stderr) = git.run(&["prev"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout f777ecc9b0db5ed372b2615695191a8a17f79f24
        @ f777ecc9 create initial.txt
        |
        O 3df4b935 (master) create test.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["next"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 3df4b9355b3b072aa6c50c6249bf32e289b3a661
        :
        @ 3df4b935 (master) create test.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_profiling() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.run_with_options(
        &["smartlog"],
        &GitRunOptions {
            env: {
                let mut env: HashMap<String, String> = HashMap::new();
                env.insert("RUST_PROFILE".to_string(), "1".to_string());
                env
            },
            ..Default::default()
        },
    )?;

    let entries: Vec<_> = std::fs::read_dir(&git.repo_path)?
        .into_iter()
        .try_collect()?;
    assert!(entries
        .iter()
        .any(|entry| entry.file_name().to_str().unwrap().contains("trace-")));

    Ok(())
}
