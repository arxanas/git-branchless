use std::fmt::Display;
use std::path::PathBuf;

use eyre::Context;
use tracing::instrument;

use super::repo::wrap_git_error;

/// Wrapper around the config values stored on disk for Git.
pub struct Config {
    inner: git2::Config,
}

impl From<git2::Config> for Config {
    fn from(config: git2::Config) -> Self {
        Config { inner: config }
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<Git repository config>")
    }
}

#[derive(Debug)]
enum ConfigValueInner {
    String(String),
    Bool(bool),
}

/// A wrapper around a possible value that can be set for a config key.
#[derive(Debug)]
pub struct ConfigValue {
    inner: ConfigValueInner,
}

impl From<bool> for ConfigValue {
    fn from(value: bool) -> ConfigValue {
        ConfigValue {
            inner: ConfigValueInner::Bool(value),
        }
    }
}

impl From<String> for ConfigValue {
    fn from(value: String) -> ConfigValue {
        ConfigValue {
            inner: ConfigValueInner::String(value),
        }
    }
}

impl From<&str> for ConfigValue {
    fn from(value: &str) -> ConfigValue {
        ConfigValue {
            inner: ConfigValueInner::String(value.to_string()),
        }
    }
}

impl Display for ConfigValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            ConfigValueInner::String(value) => write!(f, "{}", value),
            ConfigValueInner::Bool(value) => write!(f, "{:?}", value),
        }
    }
}

/// Trait used to make `Config::get` able to return multiple types.
pub trait GetConfigValue<V> {
    /// Get the given type of value from the config object.
    fn get_from_config(config: &Config, key: impl AsRef<str>) -> eyre::Result<Option<V>>;
}

impl GetConfigValue<String> for String {
    fn get_from_config(config: &Config, key: impl AsRef<str>) -> eyre::Result<Option<String>> {
        #[instrument]
        fn inner(config: &Config, key: &str) -> eyre::Result<Option<String>> {
            let value = match config.inner.get_string(key) {
                Ok(value) => Some(value),
                Err(err) if err.code() == git2::ErrorCode::NotFound => None,
                Err(err) => {
                    return Err(wrap_git_error(err))
                        .wrap_err("Looking up string value for config key");
                }
            };
            Ok(value)
        }
        inner(config, key.as_ref())
    }
}

impl GetConfigValue<bool> for bool {
    fn get_from_config(config: &Config, key: impl AsRef<str>) -> eyre::Result<Option<bool>> {
        #[instrument]
        fn inner(config: &Config, key: &str) -> eyre::Result<Option<bool>> {
            let value = match config.inner.get_bool(key) {
                Ok(value) => Some(value),
                Err(err) if err.code() == git2::ErrorCode::NotFound => None,
                Err(err) => {
                    return Err(wrap_git_error(err))
                        .wrap_err("Looking up bool value for config key")
                }
            };
            Ok(value)
        }
        inner(config, key.as_ref())
    }
}

impl GetConfigValue<PathBuf> for PathBuf {
    fn get_from_config(config: &Config, key: impl AsRef<str>) -> eyre::Result<Option<PathBuf>> {
        #[instrument]
        fn inner(config: &Config, key: &str) -> eyre::Result<Option<PathBuf>> {
            let value = match config.inner.get_path(key.as_ref()) {
                Ok(value) => Some(value),
                Err(err) if err.code() == git2::ErrorCode::NotFound => None,
                Err(err) => {
                    return Err(wrap_git_error(err))
                        .wrap_err("Looking up path value for config key")
                }
            };
            Ok(value)
        }
        inner(config, key.as_ref())
    }
}

impl Config {
    #[instrument]
    fn set_inner(&mut self, key: &str, value: ConfigValue) -> eyre::Result<()> {
        match &value.inner {
            ConfigValueInner::String(value) => {
                self.inner.set_str(key, value).map_err(wrap_git_error)
            }
            ConfigValueInner::Bool(value) => {
                self.inner.set_bool(key, *value).map_err(wrap_git_error)
            }
        }
    }

    /// Set the given config key to the given value.
    pub fn set(
        &mut self,
        key: impl AsRef<str> + std::fmt::Debug,
        value: impl Into<ConfigValue>,
    ) -> eyre::Result<()> {
        self.set_inner(key.as_ref(), value.into())
    }

    /// Get a config key of one of various possible types.
    pub fn get<V: GetConfigValue<V>, S: AsRef<str>>(&self, key: S) -> eyre::Result<Option<V>> {
        V::get_from_config(self, key)
    }

    /// Same as `get`, but uses a default value if the config key doesn't exist.
    pub fn get_or<V: GetConfigValue<V>, S: AsRef<str>>(
        &self,
        key: S,
        default: V,
    ) -> eyre::Result<V> {
        let result = self.get(key)?;
        Ok(result.unwrap_or(default))
    }

    /// Same as `get`, but computes a default value if the config key doesn't exist.
    pub fn get_or_else<V: GetConfigValue<V>, S: AsRef<str>, F: FnOnce() -> V>(
        &self,
        key: S,
        default: F,
    ) -> eyre::Result<V> {
        let result = self.get(key)?;
        match result {
            Some(result) => Ok(result),
            None => Ok(default()),
        }
    }

    #[instrument]
    fn remove_inner(&mut self, key: &str) -> eyre::Result<()> {
        self.inner
            .remove(key)
            .map_err(wrap_git_error)
            .wrap_err("Removing config key")?;
        Ok(())
    }

    /// Remove the given key from the configuration.
    pub fn remove(&mut self, key: impl AsRef<str>) -> eyre::Result<()> {
        self.remove_inner(key.as_ref())
    }
}
