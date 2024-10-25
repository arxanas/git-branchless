use std::{collections::HashSet, path::PathBuf};

use clap::Parser;
use git_branchless_opts::{ResolveRevsetOptions, Revset};
use git_branchless_revset::resolve_commits;
use git_branchless_submit::phabricator;
use lib::core::dag::{sorted_commit_set, Dag};
use lib::core::effects::Effects;
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::formatting::Glyphs;
use lib::core::repo_ext::RepoExt;
use lib::git::{GitRunInfo, NonZeroOid, Repo};

#[derive(Debug, Parser)]
struct Opts {
    #[clap(short = 'C', long = "working-directory")]
    working_directory: Option<PathBuf>,

    #[clap(default_value = ".")]
    revset: Revset,
}

fn main() -> eyre::Result<()> {
    let Opts {
        working_directory,
        revset,
    } = Opts::try_parse()?;
    match working_directory {
        Some(working_directory) => {
            std::env::set_current_dir(working_directory)?;
        }
        None => {
            eprintln!("Warning: --working-directory was not passed, so running in current directory. But git-branchless is not hosted on Phabricator, so this seems unlikely to be useful, since there will be no assocated Phabricator information. Try running in your repository.");
        }
    }

    let effects = Effects::new(Glyphs::detect());
    let git_run_info = GitRunInfo {
        path_to_git: "git".into(),
        working_directory: std::env::current_dir()?,
        env: Default::default(),
    };
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let references_snapshot = repo.get_references_snapshot()?;
    let mut dag = Dag::open_and_sync(
        &effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let resolved_commits = resolve_commits(
        &effects,
        &repo,
        &mut dag,
        &[revset.clone()],
        &ResolveRevsetOptions {
            show_hidden_commits: false,
        },
    )?;
    let commits = match resolved_commits.as_slice() {
        [commits] => commits,
        other => eyre::bail!("Unexpected number of returned commit sets for {revset}: {other:?}"),
    };
    let commit_oids: HashSet<NonZeroOid> = dag.commit_set_to_vec(commits)?.into_iter().collect();

    let commits = sorted_commit_set(&repo, &dag, commits)?;
    let phabricator = phabricator::PhabricatorForge {
        effects: &effects,
        git_run_info: &git_run_info,
        repo: &repo,
        dag: &mut dag,
        event_log_db: &event_log_db,
        event_replayer: &event_replayer,
        revset: &revset,
    };
    let dependency_oids = phabricator.query_remote_dependencies(commit_oids)?;
    for commit in commits {
        println!(
            "Dependencies for {} {}:",
            revision_id_str(&phabricator, commit.get_oid())?,
            effects
                .get_glyphs()
                .render(commit.friendly_describe(effects.get_glyphs())?)?
        );

        let dependency_oids = &dependency_oids[&commit.get_oid()];
        if dependency_oids.is_empty() {
            println!("- <none>");
        } else {
            for dependency_oid in dependency_oids {
                let dependency_commit = repo.find_commit_or_fail(*dependency_oid)?;
                println!(
                    "- {} {}",
                    revision_id_str(&phabricator, *dependency_oid)?,
                    effects
                        .get_glyphs()
                        .render(dependency_commit.friendly_describe(effects.get_glyphs())?)?
                );
            }
        }
    }
    Ok(())
}

fn revision_id_str(
    phabricator: &phabricator::PhabricatorForge,
    commit_oid: NonZeroOid,
) -> eyre::Result<String> {
    let id = phabricator.get_revision_id(commit_oid)?;
    let revision_id = match id.as_ref() {
        Some(phabricator::Id(id)) => id,
        None => "???",
    };
    Ok(format!("D{revision_id}"))
}
