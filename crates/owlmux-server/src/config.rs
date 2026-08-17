use std::{env, ffi::OsString, fmt, net::SocketAddr, path::PathBuf, time::Duration};

use uuid::Uuid;

use crate::{
    auth::ApiKey,
    build,
    cluster::{ClusterKey, ConfigurationInput},
    crypto::EncryptionKey,
};

const DEFAULT_ADDRESS: &str = "127.0.0.1:8080";
const DEFAULT_WEB_DIR: &str = "apps/web/dist";
const DEFAULT_PUBLIC_ORIGIN: &str = "http://127.0.0.1:8080";
const DEFAULT_SSH_RUNTIME_ROOT: &str = "/tmp/owlmux-ssh";
const DEFAULT_SHUTDOWN_SECONDS: u64 = 10;
const DEFAULT_LEASE_SECONDS: u64 = 30;
const DEFAULT_LEASE_MARGIN_SECONDS: u64 = 5;
const MAX_SHUTDOWN_SECONDS: u64 = 60;

pub struct ClusterConfig {
    key: ClusterKey,
    address: SocketAddr,
    advertised_url: String,
    tls_certificate: PathBuf,
    tls_private_key: PathBuf,
    tls_ca: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeploymentProfile {
    SingleNode,
    Clustered,
}

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
    profile: DeploymentProfile,
    cluster: Option<ClusterConfig>,
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
        let profile = match optional_utf8(&mut read, "OWLMUX_PROFILE")?.as_deref() {
            None | Some("single-node") => DeploymentProfile::SingleNode,
            Some("clustered") => DeploymentProfile::Clustered,
            Some(_) => return Err(ConfigError::Invalid("OWLMUX_PROFILE")),
        };
        let cluster = load_cluster_config(&mut read, profile, &api_key, &encryption_key)?;

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
            profile,
            cluster,
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
    #[must_use]
    pub const fn profile(&self) -> DeploymentProfile {
        self.profile
    }
    #[must_use]
    pub const fn cluster(&self) -> Option<&ClusterConfig> {
        self.cluster.as_ref()
    }

    pub(crate) const fn profile_database_value(&self) -> &'static str {
        match self.profile {
            DeploymentProfile::SingleNode => "single_node",
            DeploymentProfile::Clustered => "clustered",
        }
    }

    pub(crate) fn configuration_proof(&self, deployment_id: Uuid) -> Option<[u8; 32]> {
        self.cluster.as_ref().map(|cluster| {
            cluster.key.configuration_proof(&ConfigurationInput {
                deployment_id,
                config_epoch: self.epoch,
                server_build_id: build::BUILD_ID,
                api_key_digest: self.api_key.configuration_digest(),
                encryption_key_digest: self.encryption_key.configuration_digest(),
                public_origin: &self.public_origin,
            })
        })
    }
}

impl ClusterConfig {
    #[must_use]
    pub(crate) const fn key(&self) -> &ClusterKey {
        &self.key
    }
    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }
    #[must_use]
    pub fn advertised_url(&self) -> &str {
        &self.advertised_url
    }
    #[must_use]
    pub fn tls_certificate(&self) -> &std::path::Path {
        &self.tls_certificate
    }
    #[must_use]
    pub fn tls_private_key(&self) -> &std::path::Path {
        &self.tls_private_key
    }
    #[must_use]
    pub fn tls_ca(&self) -> &std::path::Path {
        &self.tls_ca
    }
}

fn load_cluster_config(
    read: &mut impl FnMut(&str) -> Option<OsString>,
    profile: DeploymentProfile,
    api_key: &ApiKey,
    encryption_key: &EncryptionKey,
) -> Result<Option<ClusterConfig>, ConfigError> {
    let names = [
        "OWLMUX_CLUSTER_KEY",
        "OWLMUX_INTERNAL_ADDR",
        "OWLMUX_INTERNAL_URL",
        "OWLMUX_INTERNAL_TLS_CERT",
        "OWLMUX_INTERNAL_TLS_KEY",
        "OWLMUX_INTERNAL_TLS_CA",
    ];
    let values = names
        .map(|name| optional_utf8(read, name).map(|value| (name, value)))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    if profile == DeploymentProfile::SingleNode {
        if let Some((name, _)) = values.iter().find(|(_, value)| value.is_some()) {
            return Err(ConfigError::Invalid(name));
        }
        return Ok(None);
    }

    let required = |index: usize| {
        values[index]
            .1
            .clone()
            .filter(|value| !value.is_empty())
            .ok_or(ConfigError::Missing(values[index].0))
    };
    let key =
        ClusterKey::parse(&required(0)?).map_err(|_| ConfigError::Invalid("OWLMUX_CLUSTER_KEY"))?;
    if key.configuration_digest() == api_key.configuration_digest()
        || key.configuration_digest() == encryption_key.configuration_digest()
    {
        return Err(ConfigError::Invalid("OWLMUX_CLUSTER_KEY"));
    }
    let address = required(1)?
        .parse()
        .map_err(|_| ConfigError::Invalid("OWLMUX_INTERNAL_ADDR"))?;
    let advertised_url = required(2)?;
    validate_internal_url(&advertised_url)?;
    let tls_certificate = PathBuf::from(required(3)?);
    let tls_private_key = PathBuf::from(required(4)?);
    let tls_ca = PathBuf::from(required(5)?);
    if !tls_certificate.is_absolute() || !tls_private_key.is_absolute() || !tls_ca.is_absolute() {
        return Err(ConfigError::Invalid("OWLMUX_INTERNAL_TLS_CERT"));
    }
    Ok(Some(ClusterConfig {
        key,
        address,
        advertised_url,
        tls_certificate,
        tls_private_key,
        tls_ca,
    }))
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

fn validate_internal_url(value: &str) -> Result<(), ConfigError> {
    let url = url::Url::parse(value).map_err(|_| ConfigError::Invalid("OWLMUX_INTERNAL_URL"))?;
    if url.scheme() != "wss"
        || url.host_str().is_none()
        || url.path() != "/internal/v1/owner"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ConfigError::Invalid("OWLMUX_INTERNAL_URL"));
    }
    Ok(())
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

    #[test]
    fn clustered_profile_requires_one_complete_tls_configuration() {
        let cluster_key = URL_SAFE_NO_PAD.encode([11_u8; 32]);
        let config = load(&[
            ("OWLMUX_PROFILE", OsStr::new("clustered")),
            ("OWLMUX_CLUSTER_KEY", OsStr::new(&cluster_key)),
            ("OWLMUX_INTERNAL_ADDR", OsStr::new("127.0.0.1:9443")),
            (
                "OWLMUX_INTERNAL_URL",
                OsStr::new("wss://node-a.example:9443/internal/v1/owner"),
            ),
            (
                "OWLMUX_INTERNAL_TLS_CERT",
                OsStr::new("/run/owlmux/tls.crt"),
            ),
            ("OWLMUX_INTERNAL_TLS_KEY", OsStr::new("/run/owlmux/tls.key")),
            ("OWLMUX_INTERNAL_TLS_CA", OsStr::new("/run/owlmux/ca.crt")),
        ])
        .expect("cluster config");
        assert_eq!(config.profile(), DeploymentProfile::Clustered);
        assert!(config.cluster().is_some());

        let error = load(&[("OWLMUX_CLUSTER_KEY", OsStr::new(&cluster_key))])
            .err()
            .expect("partial cluster config");
        assert_eq!(error, ConfigError::Invalid("OWLMUX_CLUSTER_KEY"));
    }
}
