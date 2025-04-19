use std::collections::HashMap;

use branchless::core::config::env_vars::{
    get_path_to_gpg, get_path_to_gpgsm, get_path_to_ssh_keygen,
};
use branchless::testing::{make_git, GitRunOptions};

/// Requires `gpg` to be installed and its path provided in the `TEST_GPG` environment variable.
#[cfg(feature = "test_gpg")]
mod gpg_tests {
    use super::*;

    #[test]
    fn test_valid_gpg_configuration() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;

        let gpg_path = get_path_to_gpg()?.into_os_string().into_string().unwrap();
        git.run(&["config", "gpg.program", &gpg_path])?;
        git.run(&["config", "gpg.format", "openpgp"])?;
        git.run(&["config", "commit.gpgSign", "true"])?;

        // See `test_data/gpg`
        git.run(&["config", "user.signingKey", "B3B9DB339CA11313"])?;

        git.write_file("abcde", "fghij")?;
        git.run(&["add", "."])?;

        let gpg_home = std::env::current_dir()?
            .join("tests/test_data/gpg")
            .into_os_string()
            .into_string()
            .unwrap();

        let run_options = GitRunOptions {
            env: HashMap::from([("GNUPGHOME".to_string(), gpg_home)]),
            time: 1,
            expected_exit_code: 0,
            ..Default::default()
        };
        git.run_with_options(&["commit", "-m", "add dummy file"], &run_options)?;

        let (_verify_out, verify_err) =
            git.run_with_options(&["verify-commit", "HEAD"], &run_options)?;

        assert!(verify_err.contains("gpg: Good signature from \"Testy McTestface (Test GPG key) <test@example.com>\" [ultimate]"));

        Ok(())
    }
}

/// Requires `ssh-keygen` to be installed and its path provided in the `TEST_SSH_KEYGEN`
/// environment variable.
#[cfg(feature = "test_ssh")]
mod ssh_tests {
    use super::*;
    use std::path::Path;

    #[cfg(unix)]
    fn set_mode_700(path: &Path) -> eyre::Result<()> {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        // Git doesn't track permissions and SSH will complain that the permissions on our test
        // cert are too broad.
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions)?;

        Ok(())
    }

    #[cfg(not(unix))]
    fn set_mode_700(_path: &Path) -> eyre::Result<()> {
        Ok(())
    }

    #[test]
    fn test_valid_ssh_configuration() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;

        let ssh_base_dir = std::env::current_dir()?.join("tests/test_data/ssh");
        let ssh_priv_key = ssh_base_dir.join("id_ed25519");
        set_mode_700(ssh_priv_key.as_path())?;

        let ssh_priv_key = ssh_priv_key.into_os_string().into_string().unwrap();
        let ssh_allowed_signers = ssh_base_dir
            .join("ssh-allowed-signers")
            .into_os_string()
            .into_string()
            .unwrap();

        let ssh_keygen_path = get_path_to_ssh_keygen()?
            .into_os_string()
            .into_string()
            .unwrap();
        git.run(&["config", "gpg.format", "ssh"])?;
        git.run(&["config", "commit.gpgSign", "true"])?;
        git.run(&["config", "gpg.ssh.allowedSignersFile", &ssh_allowed_signers])?;
        git.run(&["config", "gpg.ssh.program", &ssh_keygen_path])?;

        // See `test_data/ssh`
        git.run(&["config", "user.signingKey", &ssh_priv_key])?;

        git.write_file("abcde", "fghij")?;
        git.run(&["add", "."])?;

        let run_options = GitRunOptions {
            time: 1,
            expected_exit_code: 0,
            ..Default::default()
        };
        git.run_with_options(&["commit", "-m", "add dummy file"], &run_options)?;

        let (_verify_out, verify_err) =
            git.run_with_options(&["verify-commit", "HEAD"], &run_options)?;

        assert_eq!(verify_err, "Good \"git\" signature for test@example.com with ED25519 key SHA256:R3Yi2je27BFWoROhBpnpb3neSsy86IHXZW9PZ/nchNg\n");

        Ok(())
    }
}

/// Requires `gpgsm` to be installed and its path provided in the `TEST_GPGSM` environment
/// variable.
#[cfg(feature = "test_x509")]
mod x509_tests {
    use super::*;

    #[test]
    fn test_valid_x509_configuration() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;

        let gpgsm_path = get_path_to_gpgsm()?.into_os_string().into_string().unwrap();
        git.run(&["config", "gpg.format", "x509"])?;
        git.run(&["config", "commit.gpgSign", "true"])?;
        git.run(&["config", "gpg.x509.program", &gpgsm_path])?;

        // See `test_data/x509`
        git.run(&["config", "user.signingKey", "0x33553E43"])?;

        git.write_file("abcde", "fghij")?;
        git.run(&["add", "."])?;

        let x509_home = std::env::current_dir()?
            .join("tests/test_data/x509")
            .into_os_string()
            .into_string()
            .unwrap();

        let run_options = GitRunOptions {
            env: HashMap::from([("GNUPGHOME".to_string(), x509_home)]),
            time: 1,
            ..Default::default()
        };
        git.run_with_options(&["commit", "-m", "add dummy file"], &run_options)?;

        let (_verify_out, verify_err) =
            git.run_with_options(&["verify-commit", "HEAD"], &run_options)?;

        assert!(verify_err.contains(
            r###"
gpgsm: Good signature from "/CN=example.com"
gpgsm:                 aka "test@example.com"
gpgsm:                 aka "test2@example.com"
"###
        ));

        Ok(())
    }
}
