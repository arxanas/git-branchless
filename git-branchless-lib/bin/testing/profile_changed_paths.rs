use std::path::PathBuf;

use branchless::git::{NonZeroOid, Repo};
use tracing_chrome::ChromeLayerBuilder;
use tracing_error::ErrorLayer;
use tracing_subscriber::prelude::*;

fn main() -> eyre::Result<()> {
    color_eyre::install()?;

    if std::env::var("RUST_PROFILE").is_ok() {
        let include_args = std::env::var("RUST_PROFILE_INCLUDE_ARGS").is_ok();
        let (profile_layer, _profile_layer_guard) =
            ChromeLayerBuilder::new().include_args(include_args).build();

        tracing_subscriber::registry()
            .with(ErrorLayer::default())
            .with(profile_layer)
            .try_init()?;
    }

    let path_to_repo: PathBuf = std::env::var("PATH_TO_REPO")
        .expect("No `PATH_TO_REPO` was set")
        .into();
    println!("Path to repo: {:?}", path_to_repo);

    let repo = Repo::from_dir(&path_to_repo)?;
    let commit = match std::env::var("COMMIT_OID") {
        Ok(commit_oid) => {
            let commit_oid: NonZeroOid = commit_oid.parse()?;
            repo.find_commit_or_fail(commit_oid)?
        }
        Err(_) => {
            let head_oid = repo
                .get_head_info()?
                .oid
                .expect("No `COMMIT_OID` was set, and no `HEAD` OID is available");
            repo.find_commit_or_fail(head_oid)?
        }
    };
    println!("Commit to check: {:?}", &commit);

    let result = repo.get_paths_touched_by_commit(&commit)?;
    println!("Result: {:?}", result);

    Ok(())
}
