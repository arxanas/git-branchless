use eyre::WrapErr;
use git_branchless_testing::{make_git, GitRunOptions};

#[test]
fn test_git_is_not_a_wrapper() -> eyre::Result<()> {
    let git = make_git()?;
    {
        let (_stdout, stderr) = git
            .run_with_options(
                &["config", "--global", "--list"],
                &GitRunOptions {
                    expected_exit_code: 128,
                    ..Default::default()
                },
            )
            .wrap_err(
                "The Git global configuration should not exist during tests, \
as the HOME environment variable is not set. \
Check that the Git executable is not being wrapped in a shell script.",
            )?;
        insta::assert_snapshot!(stderr, @r###"
        fatal: $HOME not set
        "###);
    }
    Ok(())
}
