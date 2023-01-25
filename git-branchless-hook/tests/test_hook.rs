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
