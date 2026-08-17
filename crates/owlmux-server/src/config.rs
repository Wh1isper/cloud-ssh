use std::{env, ffi::OsString, fmt, net::SocketAddr, path::PathBuf, time::Duration};

use crate::{auth::ApiKey, crypto::EncryptionKey};

const DEFAULT_ADDRESS: &str = "127.0.0.1:8080";
const DEFAULT_WEB_DIR: &str = "apps/web/dist";
const DEFAULT_PUBLIC_ORIGIN: &str = "http://127.0.0.1:8080";
const DEFAULT_SSH_RUNTIME_ROOT: &str = "/tmp/owlmux-ssh";
const DEFAULT_SHUTDOWN_SECONDS: u64 = 10;
const DEFAULT_LEASE_SECONDS: u64 = 30;
const DEFAULT_LEASE_MARGIN_SECONDS: u64 = 5;
const MAX_SHUTDOWN_SECONDS: u64 = 60;

pub struct Config {
    address: SocketAddr,
    web_dir: PathBuf,
    public_origin: String,
    database_url: String,
    api_key: ApiKey,
    encryption_key: EncryptionKey,
    ssh_runtime_root: PathBuf,
    epoch: i64,
    lease_ttl: Duration,
    lease_safety_margin: Duration,
    shutdown_timeout: Duration,
    node_name: Option<String>,
}

impl Config {
    /// Load immutable Deployment and node settings from the process environment.
    ///
    /// # Errors
    ///
    /// Returns an error naming the absent or invalid setting without including its value.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::load(|key| env::var_os(key))
    }

    pub(crate) fn load(
        mut read: impl FnMut(&str) -> Option<OsString>,
    ) -> Result<Self, ConfigError> {
        let address = optional_utf8(&mut read, "OWLMUX_ADDR")?
            .unwrap_or_else(|| DEFAULT_ADDRESS.to_owned())
            .parse()
            .map_err(|_| ConfigError::Invalid("OWLMUX_ADDR"))?;
        let web_dir = read("OWLMUX_WEB_DIR")
            .filter(|value| !value.is_empty())
            .map_or_else(|| PathBuf::from(DEFAULT_WEB_DIR), PathBuf::from);
        let public_origin = optional_utf8(&mut read, "OWLMUX_PUBLIC_ORIGIN")?
            .unwrap_or_else(|| DEFAULT_PUBLIC_ORIGIN.to_owned());
        validate_origin(&public_origin)?;
        let database_url = required_utf8(&mut read, "OWLMUX_DATABASE_URL")?;
        if !database_url.starts_with("postgres://") && !database_url.starts_with("postgresql://") {
            return Err(ConfigError::Invalid("OWLMUX_DATABASE_URL"));
        }
        let api_key = ApiKey::parse(&required_utf8(&mut read, "OWLMUX_API_KEY")?)
            .map_err(|_| ConfigError::Invalid("OWLMUX_API_KEY"))?;
        let encryption_key =
            EncryptionKey::parse(&required_utf8(&mut read, "OWLMUX_SSH_KEY_ENCRYPTION_KEY")?)
                .map_err(|_| ConfigError::Invalid("OWLMUX_SSH_KEY_ENCRYPTION_KEY"))?;
        let ssh_runtime_root = read("OWLMUX_SSH_RUNTIME_ROOT")
            .filter(|value| !value.is_empty())
            .map_or_else(|| PathBuf::from(DEFAULT_SSH_RUNTIME_ROOT), PathBuf::from);
        if !ssh_runtime_root.is_absolute() {
            return Err(ConfigError::Invalid("OWLMUX_SSH_RUNTIME_ROOT"));
        }
        let config_epoch = parse_number(&mut read, "OWLMUX_CONFIG_EPOCH", 1_i64)?;
        if config_epoch <= 0 {
            return Err(ConfigError::Invalid("OWLMUX_CONFIG_EPOCH"));
        }
        let lease_seconds = parse_number(
            &mut read,
            "OWLMUX_NODE_LEASE_TTL_SECONDS",
            DEFAULT_LEASE_SECONDS,
        )?;
        let margin_seconds = parse_number(
            &mut read,
            "OWLMUX_NODE_LEASE_SAFETY_MARGIN_SECONDS",
            DEFAULT_LEASE_MARGIN_SECONDS,
        )?;
        if margin_seconds == 0 || margin_seconds >= lease_seconds || lease_seconds > 300 {
            return Err(ConfigError::Invalid(
                "OWLMUX_NODE_LEASE_SAFETY_MARGIN_SECONDS",
            ));
        }
        let shutdown_seconds = parse_number(
            &mut read,
            "OWLMUX_SHUTDOWN_TIMEOUT_SECONDS",
            DEFAULT_SHUTDOWN_SECONDS,
        )?;
        if !(1..=MAX_SHUTDOWN_SECONDS).contains(&shutdown_seconds) {
            return Err(ConfigError::Invalid("OWLMUX_SHUTDOWN_TIMEOUT_SECONDS"));
        }
        let node_name = optional_utf8(&mut read, "OWLMUX_NODE_NAME")?;
        if node_name.as_ref().is_some_and(|name| {
            name.is_empty() || name.len() > 128 || name.chars().any(char::is_control)
        }) {
            return Err(ConfigError::Invalid("OWLMUX_NODE_NAME"));
        }

        Ok(Self {
            address,
            web_dir,
            public_origin,
            database_url,
            api_key,
            encryption_key,
            ssh_runtime_root,
            epoch: config_epoch,
            lease_ttl: Duration::from_secs(lease_seconds),
            lease_safety_margin: Duration::from_secs(margin_seconds),
            shutdown_timeout: Duration::from_secs(shutdown_seconds),
            node_name,
        })
    }

    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }
    #[must_use]
    pub fn web_dir(&self) -> &std::path::Path {
        &self.web_dir
    }
    #[must_use]
    pub fn public_origin(&self) -> &str {
        &self.public_origin
    }
    #[must_use]
    pub fn database_url(&self) -> &str {
        &self.database_url
    }
    #[must_use]
    pub const fn api_key(&self) -> &ApiKey {
        &self.api_key
    }
    #[must_use]
    pub const fn encryption_key(&self) -> &EncryptionKey {
        &self.encryption_key
    }
    #[must_use]
    pub fn ssh_runtime_root(&self) -> &std::path::Path {
        &self.ssh_runtime_root
    }
    #[must_use]
    pub const fn config_epoch(&self) -> i64 {
        self.epoch
    }
    #[must_use]
    pub const fn lease_ttl(&self) -> Duration {
        self.lease_ttl
    }
    #[must_use]
    pub const fn lease_safety_margin(&self) -> Duration {
        self.lease_safety_margin
    }
    #[must_use]
    pub const fn shutdown_timeout(&self) -> Duration {
        self.shutdown_timeout
    }
    #[must_use]
    pub fn node_name(&self) -> Option<&str> {
        self.node_name.as_deref()
    }
}

fn required_utf8(
    read: &mut impl FnMut(&str) -> Option<OsString>,
    key: &'static str,
) -> Result<String, ConfigError> {
    optional_utf8(read, key)?
        .filter(|value| !value.is_empty())
        .ok_or(ConfigError::Missing(key))
}

fn optional_utf8(
    read: &mut impl FnMut(&str) -> Option<OsString>,
    key: &'static str,
) -> Result<Option<String>, ConfigError> {
    read(key)
        .map(|value| value.into_string().map_err(|_| ConfigError::Invalid(key)))
        .transpose()
}

fn parse_number<T: std::str::FromStr>(
    read: &mut impl FnMut(&str) -> Option<OsString>,
    key: &'static str,
    default: T,
) -> Result<T, ConfigError> {
    optional_utf8(read, key)?.map_or(Ok(default), |value| {
        value.parse().map_err(|_| ConfigError::Invalid(key))
    })
}

fn validate_origin(origin: &str) -> Result<(), ConfigError> {
    let remainder = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .ok_or(ConfigError::Invalid("OWLMUX_PUBLIC_ORIGIN"))?;
    if remainder.is_empty()
        || remainder.contains('/')
        || remainder.contains('?')
        || remainder.contains('#')
    {
        return Err(ConfigError::Invalid("OWLMUX_PUBLIC_ORIGIN"));
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
pub enum ConfigError {
    Missing(&'static str),
    Invalid(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(key) => write!(formatter, "missing {key}"),
            Self::Invalid(key) => write!(formatter, "invalid {key}"),
        }
    }
}
impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use std::{collections::HashMap, ffi::OsStr};

    fn base() -> HashMap<String, OsString> {
        [
            (
                "OWLMUX_DATABASE_URL",
                "postgres://owlmux:owlmux@127.0.0.1/owlmux".to_owned(),
            ),
            (
                "OWLMUX_API_KEY",
                format!("owlmux_sk_v1_{}", URL_SAFE_NO_PAD.encode([7_u8; 32])),
            ),
            (
                "OWLMUX_SSH_KEY_ENCRYPTION_KEY",
                URL_SAFE_NO_PAD.encode([9_u8; 32]),
            ),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), OsString::from(value)))
        .collect()
    }

    fn load(overrides: &[(&str, &OsStr)]) -> Result<Config, ConfigError> {
        let mut values = base();
        values.extend(
            overrides
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_os_string())),
        );
        Config::load(|key| values.get(key).cloned())
    }

    #[test]
    fn defaults_are_local_and_bounded() {
        let config = load(&[]).expect("valid defaults");
        assert_eq!(config.address(), "127.0.0.1:8080".parse().expect("address"));
        assert_eq!(config.lease_ttl(), Duration::from_secs(30));
        assert_eq!(config.lease_safety_margin(), Duration::from_secs(5));
    }

    #[test]
    fn rejects_missing_and_noncanonical_secrets_without_values() {
        let mut values = base();
        values.remove("OWLMUX_API_KEY");
        let error = Config::load(|key| values.get(key).cloned())
            .err()
            .expect("missing key");
        assert_eq!(error.to_string(), "missing OWLMUX_API_KEY");
        let error = load(&[("OWLMUX_API_KEY", OsStr::new("secret-invalid"))])
            .err()
            .expect("invalid key");
        assert_eq!(error.to_string(), "invalid OWLMUX_API_KEY");
        assert!(!error.to_string().contains("secret-invalid"));
    }

    #[test]
    fn validates_lease_margin() {
        let error = load(&[("OWLMUX_NODE_LEASE_SAFETY_MARGIN_SECONDS", OsStr::new("30"))])
            .err()
            .expect("margin");
        assert_eq!(
            error,
            ConfigError::Invalid("OWLMUX_NODE_LEASE_SAFETY_MARGIN_SECONDS")
        );
    }
}
