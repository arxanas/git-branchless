use tracing::instrument;

use super::{Repo, RepoError};
use crate::git::config::ConfigRead;

/// GPG-signing option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignOption {
    /// Sign commits conditionally based on the `commit.gpgsign` configuration and
    /// and the key `user.signingkey`.
    UseConfig,
    /// Sign commits using the key from `user.signingkey` configuration.
    UseConfigKey,
    /// Sign commits using the provided signing key.
    KeyOverride(String),
    /// Do not sign commits.
    Disable,
}

impl SignOption {
    /// GPG-signing flag to pass to Git.
    pub fn as_git_flag(&self) -> Option<String> {
        match self {
            Self::UseConfig => None,
            Self::UseConfigKey => Some("--gpg-sign".to_string()),
            Self::KeyOverride(keyid) => Some(format!("--gpg-sign={}", keyid)),
            Self::Disable => Some("--no-gpg-sign".to_string()),
        }
    }

    /// GPG-signing flag to use for interactive rebase
    pub fn as_rebase_flag(&self, repo: &Repo) -> eyre::Result<Option<String>> {
        Ok(match self {
            Self::UseConfig => match repo.get_readonly_config()?.get("commit.gpgsign")? {
                Some(true) => Some("-S".to_string()),
                Some(false) | None => None,
            },
            Self::UseConfigKey => Some("-S".to_string()),
            Self::KeyOverride(keyid) => Some(format!("-S{}", keyid)),
            Self::Disable => None,
        })
    }
}

/// Get commit signer configured from CLI arguments and repository configurations.
#[allow(clippy::as_conversions)]
#[instrument]
pub fn get_signer(
    repo: &Repo,
    option: &SignOption,
) -> eyre::Result<Option<Box<dyn git2_ext::ops::Sign>>> {
    match option {
        SignOption::UseConfig | SignOption::UseConfigKey => {
            if *option == SignOption::UseConfig
                && !repo
                    .get_readonly_config()?
                    .get_or("commit.gpgsign", false)?
            {
                return Ok(None);
            }

            let config = repo.inner.config().map_err(RepoError::ReadConfig)?;
            let signer = git2_ext::ops::UserSign::from_config(&repo.inner, &config)
                .map_err(RepoError::ReadConfig)?;
            Ok(Some(Box::new(signer) as Box<dyn git2_ext::ops::Sign>))
        }
        SignOption::KeyOverride(keyid) => {
            let config = repo.get_readonly_config()?;
            let format = config.get_or_else("gpg.format", || "openpgp".to_owned())?;

            let signer = match format.as_str() {
                "openpgp" => {
                    let program = match config.get("gpg.openpgp.program")? {
                        Some(program) => program,
                        None => config.get_or_else("gpg.program", || "gpg".to_owned())?,
                    };

                    Box::new(git2_ext::ops::GpgSign::new(program, keyid.to_string()))
                        as Box<dyn git2_ext::ops::Sign>
                }
                "x509" => {
                    let program = config.get_or_else("gpg.x509.program", || "gpgsm".to_owned())?;

                    Box::new(git2_ext::ops::GpgSign::new(program, keyid.to_string()))
                        as Box<dyn git2_ext::ops::Sign>
                }
                "ssh" => {
                    let program =
                        config.get_or_else("gpg.ssh.program", || "ssh-keygen".to_owned())?;

                    Box::new(git2_ext::ops::SshSign::new(program, keyid.to_string()))
                        as Box<dyn git2_ext::ops::Sign>
                }
                format => {
                    return Err(RepoError::ReadConfig(git2::Error::new(
                        git2::ErrorCode::Invalid,
                        git2::ErrorClass::Config,
                        format!("invalid value for gpg.format: {}", format),
                    ))
                    .into())
                }
            };
            Ok(Some(signer))
        }
        SignOption::Disable => Ok(None),
    }
}
