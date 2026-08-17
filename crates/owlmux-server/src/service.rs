use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::sync::Semaphore;

use crate::{
    config::Config, deployment::NodeLease, relay::RelayRegistry, ssh::SshRuntime, storage::Database,
};

pub struct ServerState {
    pub config: Arc<Config>,
    pub database: Database,
    pub lease: Arc<NodeLease>,
    pub relays: RelayRegistry,
    pub ssh: Arc<SshRuntime>,
    pub relay_connection_limit: Arc<Semaphore>,
    pub attachment_connection_limit: Arc<Semaphore>,
    pub preauth_attempt_limit: Arc<Semaphore>,
    pub source_admission: SourceAdmission,
}

impl ServerState {
    /// Bootstrap durable state and register one fresh node incarnation.
    ///
    /// # Errors
    ///
    /// Returns a sanitized startup error if storage initialization or node registration fails.
    pub async fn bootstrap(config: Config) -> Result<Arc<Self>, BootstrapError> {
        let config = Arc::new(config);
        let database = Database::bootstrap(&config).await?;
        let ssh = SshRuntime::create(config.ssh_runtime_root())?;
        let lease = NodeLease::register(database.clone(), &config).await?;
        let relays = RelayRegistry::new(lease.clone());
        Ok(Arc::new(Self {
            config,
            database,
            lease,
            relays,
            ssh,
            relay_connection_limit: Arc::new(Semaphore::new(128)),
            attachment_connection_limit: Arc::new(Semaphore::new(128)),
            preauth_attempt_limit: Arc::new(Semaphore::new(64)),
            source_admission: SourceAdmission::new(),
        }))
    }
}

const MAX_ADMISSION_SOURCES: usize = 4096;
const MAX_PREAUTH_PER_SOURCE: usize = 4;
const MAX_PREAUTH_ATTEMPTS_PER_WINDOW: usize = 256;
const ADMISSION_WINDOW: Duration = Duration::from_mins(1);

#[derive(Clone)]
pub struct SourceAdmission {
    entries: Arc<Mutex<HashMap<IpAddr, SourceEntry>>>,
}

struct SourceEntry {
    active: usize,
    attempts: usize,
    expires_at: Instant,
}

pub struct SourcePermit {
    admission: SourceAdmission,
    source: IpAddr,
}

impl SourceAdmission {
    fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[must_use]
    pub fn try_acquire(&self, source: IpAddr) -> Option<SourcePermit> {
        let now = Instant::now();
        let mut entries = self.entries.lock().ok()?;
        entries.retain(|_, entry| entry.active != 0 || entry.expires_at > now);
        if !entries.contains_key(&source) && entries.len() >= MAX_ADMISSION_SOURCES {
            return None;
        }
        let entry = entries.entry(source).or_insert(SourceEntry {
            active: 0,
            attempts: 0,
            expires_at: now + ADMISSION_WINDOW,
        });
        if entry.active == 0 && entry.expires_at <= now {
            entry.attempts = 0;
            entry.expires_at = now + ADMISSION_WINDOW;
        }
        if entry.active >= MAX_PREAUTH_PER_SOURCE
            || entry.attempts >= MAX_PREAUTH_ATTEMPTS_PER_WINDOW
        {
            return None;
        }
        entry.active += 1;
        entry.attempts += 1;
        Some(SourcePermit {
            admission: self.clone(),
            source,
        })
    }
}

impl Drop for SourcePermit {
    fn drop(&mut self) {
        let Ok(mut entries) = self.admission.entries.lock() else {
            return;
        };
        if let Some(entry) = entries.get_mut(&self.source) {
            entry.active = entry.active.saturating_sub(1);
        }
    }
}

#[derive(Debug)]
pub enum BootstrapError {
    Storage(crate::storage::StorageError),
    Lease(crate::deployment::LeaseError),
    Ssh(crate::ssh::SshError),
}

impl From<crate::storage::StorageError> for BootstrapError {
    fn from(value: crate::storage::StorageError) -> Self {
        Self::Storage(value)
    }
}
impl From<crate::ssh::SshError> for BootstrapError {
    fn from(value: crate::ssh::SshError) -> Self {
        Self::Ssh(value)
    }
}
impl From<crate::deployment::LeaseError> for BootstrapError {
    fn from(value: crate::deployment::LeaseError) -> Self {
        Self::Lease(value)
    }
}
impl std::fmt::Display for BootstrapError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) => error.fmt(formatter),
            Self::Lease(error) => error.fmt(formatter),
            Self::Ssh(error) => error.fmt(formatter),
        }
    }
}
impl std::error::Error for BootstrapError {}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    #[test]
    fn source_admission_is_bounded_and_releases_promptly() {
        let admission = SourceAdmission::new();
        let source = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let mut permits = (0..MAX_PREAUTH_PER_SOURCE)
            .map(|_| admission.try_acquire(source).expect("source permit"))
            .collect::<Vec<_>>();
        assert!(admission.try_acquire(source).is_none());
        assert!(
            admission
                .try_acquire(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)))
                .is_some()
        );
        permits.pop();
        assert!(admission.try_acquire(source).is_some());
    }

    #[test]
    fn source_admission_limits_attempts_within_the_expiry_window() {
        let admission = SourceAdmission::new();
        let source = IpAddr::V4(Ipv4Addr::LOCALHOST);
        for _ in 0..MAX_PREAUTH_ATTEMPTS_PER_WINDOW {
            drop(admission.try_acquire(source).expect("window attempt"));
        }
        assert!(admission.try_acquire(source).is_none());
    }
}
