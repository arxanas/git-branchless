use lib::testing::{make_git, make_git_worktree};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn test_smartlog_hides_linked_worktrees_by_default() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.commit_file("test1", 1)?;

    let _worktree = make_git_worktree(&git, "side")?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    @ 62fc20d (> master) create test1.txt
    "###);

    Ok(())
}

#[test]
fn test_smartlog_shows_detached_linked_worktree() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.commit_file("test1", 1)?;

    let _worktree = make_git_worktree(&git, "side")?;

    let (stdout, _stderr) = git.branchless("smartlog", &["--worktrees"])?;
    insta::assert_snapshot!(stdout, @r###"
    :
    @ 62fc20d (> master) (> <repo-name>, wt side) create test1.txt
    "###);

    Ok(())
}

#[test]
fn test_smartlog_shows_current_and_home_worktree_annotations() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.commit_file("test1", 1)?;

    let worktree_wrapper = make_git_worktree(&git, "topic-wt")?;
    let worktree = worktree_wrapper.worktree;

    let (stdout, _stderr) = worktree.branchless("smartlog", &["--worktrees"])?;
    insta::assert_snapshot!(stdout, @r###"
    :
    @ 62fc20d (master) (wt <repo-name>, > topic-wt) create test1.txt
    "###);

    Ok(())
}

#[test]
fn test_smartlog_shows_worktrees_from_config() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.commit_file("test1", 1)?;

    let _worktree = make_git_worktree(&git, "side")?;

    git.run(&["config", "branchless.smartlog.showWorktrees", "true"])?;
    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    @ 62fc20d (> master) (> <repo-name>, wt side) create test1.txt
    "###);

    Ok(())
}

#[test]
fn test_navigation_smartlog_shows_worktrees_from_config() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.commit_file("test1", 1)?;

    let _worktree = make_git_worktree(&git, "side")?;

    git.run(&["config", "branchless.smartlog.showWorktrees", "true"])?;
    let (stdout, _stderr) = git.branchless("prev", &[])?;
    insta::assert_snapshot!(stdout, @r###"
    branchless: running command: <git-executable> checkout f777ecc9b0db5ed372b2615695191a8a17f79f24 --
    @ f777ecc (> <repo-name>) create initial.txt
    |
    O 62fc20d (master) (wt side) create test1.txt
    "###);

    Ok(())
}

#[test]
fn test_smartlog_shows_attached_linked_worktree_outside_default_selection() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    let topic_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    git.run(&["branch", "topic", &topic_oid.to_string()])?;
    let worktree_wrapper = make_git_worktree(&git, "topic-wt")?;
    let worktree = worktree_wrapper.worktree;
    worktree.run(&["checkout", "topic"])?;

    git.run(&["config", "branchless.smartlog.defaultRevset", "none()"])?;
    let (stdout, _stderr) = git.branchless("smartlog", &["--worktrees"])?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (topic) (wt topic-wt) create test1.txt
    |
    @ 96d1c37 (> master) (> <repo-name>) create test2.txt
    "###);

    Ok(())
}

#[test]
fn test_move_smartlog_shows_worktrees_from_config() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;

    let worktree_wrapper = make_git_worktree(&git, "topic-wt")?;
    let worktree = worktree_wrapper.worktree;
    git.run(&["checkout", "master"])?;
    worktree.run(&["config", "branchless.smartlog.showWorktrees", "true"])?;

    let (stdout, _stderr) = worktree.branchless("move", &["-s", "@", "-d", "master"])?;
    insta::assert_snapshot!(stdout, @r###"
    Attempting rebase in-memory...
    [1/1] Committed as: 4838e49 create test3.txt
    branchless: processing 1 rewritten commit
    branchless: running command: <git-executable> checkout 4838e49b08954becdd17c0900c1179c2c654c627 --
    :
    O 62fc20d (master) (wt <repo-name>) create test1.txt
    |\
    | o 96d1c37 create test2.txt
    |
    @ 4838e49 (> topic-wt) create test3.txt
    In-memory rebase succeeded.
    "###);

    Ok(())
}

#[test]
fn test_smartlog_syncs_detached_linked_worktree_head_not_in_dag() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.commit_file("test1", 1)?;

    let (commit_oid, _stderr) = git.run(&["commit-tree", "HEAD^{tree}", "-m", "outside"])?;
    let commit_oid = commit_oid.trim();
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .to_string();
    let temp_dir =
        std::env::temp_dir().join(format!("git-branchless-smartlog-outside-{unique_suffix}"));
    let worktree_path = temp_dir.join("outside-wt");
    std::fs::create_dir_all(worktree_path.parent().unwrap())?;
    git.run(&[
        "worktree",
        "add",
        "--detach",
        worktree_path.to_string_lossy().as_ref(),
        commit_oid,
    ])?;

    let (stdout, _stderr) = git.branchless("smartlog", &["--worktrees"])?;
    insta::assert_snapshot!(stdout, @r###"
    o b1ee011 (wt outside-wt) outside
    :
    @ 62fc20d (> master) (> <repo-name>) create test1.txt
    "###);

    Ok(())
}

#[test]
fn test_smartlog_ignores_prunable_worktree() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.commit_file("test1", 1)?;

    let worktree_wrapper = make_git_worktree(&git, "gone-wt")?;
    std::fs::remove_dir_all(&worktree_wrapper.worktree.repo_path)?;

    let (stdout, _stderr) = git.branchless("smartlog", &["--worktrees"])?;
    insta::assert_snapshot!(stdout, @r###"
    :
    @ 62fc20d (> master) create test1.txt
    "###);

    Ok(())
}

#[test]
fn test_smartlog_handles_worktree_path_with_newline() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.commit_file("test1", 1)?;
    if git.run(&["worktree", "list", "--porcelain", "-z"]).is_err() {
        return Ok(());
    }

    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .to_string();
    let temp_dir =
        std::env::temp_dir().join(format!("git-branchless-smartlog-newline-{unique_suffix}"));
    let worktree_path = temp_dir.join("line\nbreak");
    std::fs::create_dir_all(worktree_path.parent().unwrap())?;

    git.run(&[
        "worktree",
        "add",
        "--detach",
        worktree_path.to_string_lossy().as_ref(),
    ])?;

    let (stdout, _stderr) = git.branchless("smartlog", &["--worktrees"])?;
    insta::assert_snapshot!(stdout, @r###"
    :
    @ 62fc20d (> master) (> <repo-name>, wt line\nbreak) create test1.txt
    "###);

    Ok(())
}

#[test]
fn test_smartlog_disambiguates_duplicate_worktree_names() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.commit_file("test1", 1)?;

    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .to_string();
    let temp_dir =
        std::env::temp_dir().join(format!("git-branchless-smartlog-worktree-{unique_suffix}"));
    let feature_topic = temp_dir.join("feature").join("topic");
    let bugfix_topic = temp_dir.join("bugfix").join("topic");
    std::fs::create_dir_all(feature_topic.parent().unwrap())?;
    std::fs::create_dir_all(bugfix_topic.parent().unwrap())?;

    git.run(&[
        "worktree",
        "add",
        "--detach",
        feature_topic.to_string_lossy().as_ref(),
    ])?;
    git.run(&[
        "worktree",
        "add",
        "--detach",
        bugfix_topic.to_string_lossy().as_ref(),
    ])?;

    let (stdout, _stderr) = git.branchless("smartlog", &["--worktrees"])?;
    insta::assert_snapshot!(stdout, @r###"
    :
    @ 62fc20d (> master) (> <repo-name>, wt bugfix/topic, wt feature/topic) create test1.txt
    "###);

    Ok(())
}
