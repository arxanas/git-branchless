/// Get the name of the main branch for the repository.
///
/// Args:
/// * `repo`: The Git repository.
///
/// Returns: The name of the main branch for the repository.
pub fn get_main_branch_name(repo: &git2::Repository) -> anyhow::Result<String> {
    let config = repo.config()?;
    config
        .get_string("branchless.mainBranch")
        .or_else(|_| Ok(String::from("master")))
}
