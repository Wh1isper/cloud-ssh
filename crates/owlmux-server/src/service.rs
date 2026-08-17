use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::sync::Semaphore;

use crate::{
    config::Config,
    deployment::NodeLease,
    internal::{InternalClient, InternalLimits},
    relay::RelayRegistry,
    ssh::SshRuntime,
    storage::Database,
    writer::WriterRegistry,
};

pub struct ServerState {
    pub config: Arc<Config>,
    pub database: Database,
    pub lease: Arc<NodeLease>,
    pub relays: RelayRegistry,
    pub ssh: Arc<SshRuntime>,
    pub writers: WriterRegistry,
    pub internal: Option<InternalClient>,
    pub relay_connection_limit: Arc<Semaphore>,
    pub attachment_connection_limit: Arc<Semaphore>,
    pub api_mutation_limit: Arc<Semaphore>,
    pub(crate) internal_limits: InternalLimits,
    pub preauth_attempt_limit: Arc<Semaphore>,
    pub source_admission: SourceAdmission,
    pub observability: Observability,
}

impl ServerState {
    /// Bootstrap durable state and register one fresh node incarnation.
    ///
    /// # Errors
    ///
    /// Returns a sanitized startup error if storage initialization or node registration fails.
    pub async fn bootstrap(
        config: Config,
        internal: Option<InternalClient>,
    ) -> Result<Arc<Self>, BootstrapError> {
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
            writers: WriterRegistry::new(),
            internal,
            relay_connection_limit: Arc::new(Semaphore::new(128)),
            attachment_connection_limit: Arc::new(Semaphore::new(128)),
            api_mutation_limit: Arc::new(Semaphore::new(32)),
            internal_limits: InternalLimits::new(),
            preauth_attempt_limit: Arc::new(Semaphore::new(64)),
            source_admission: SourceAdmission::new(),
            observability: Observability::default(),
        }))
    }
}

#[derive(Default)]
pub struct Observability {
    api_authenticated_requests: AtomicU64,
    api_auth_rejections: AtomicU64,
    api_mutation_overloads: AtomicU64,
    owner_local_resolutions: AtomicU64,
    owner_remote_resolutions: AtomicU64,
    owner_absent_resolutions: AtomicU64,
    owner_resolution_failures: AtomicU64,
}

impl Observability {
    pub(crate) fn api_authenticated(&self) {
        self.api_authenticated_requests
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn api_auth_rejected(&self) {
        self.api_auth_rejections.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn api_mutation_overloaded(&self) {
        self.api_mutation_overloads.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn owner_local(&self) {
        self.owner_local_resolutions.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn owner_remote(&self) {
        self.owner_remote_resolutions
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn owner_absent(&self) {
        self.owner_absent_resolutions
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn owner_resolution_failed(&self) {
        self.owner_resolution_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    #[must_use]
    pub fn snapshot(&self) -> ObservabilitySnapshot {
        ObservabilitySnapshot {
            api_authenticated_requests_total: self
                .api_authenticated_requests
                .load(Ordering::Relaxed),
            api_auth_rejections_total: self.api_auth_rejections.load(Ordering::Relaxed),
            api_mutation_overloads_total: self.api_mutation_overloads.load(Ordering::Relaxed),
            owner_local_resolutions_total: self.owner_local_resolutions.load(Ordering::Relaxed),
            owner_remote_resolutions_total: self.owner_remote_resolutions.load(Ordering::Relaxed),
            owner_absent_resolutions_total: self.owner_absent_resolutions.load(Ordering::Relaxed),
            owner_resolution_failures_total: self.owner_resolution_failures.load(Ordering::Relaxed),
        }
    }
}

pub struct ObservabilitySnapshot {
    pub api_authenticated_requests_total: u64,
    pub api_auth_rejections_total: u64,
    pub api_mutation_overloads_total: u64,
    pub owner_local_resolutions_total: u64,
    pub owner_remote_resolutions_total: u64,
    pub owner_absent_resolutions_total: u64,
    pub owner_resolution_failures_total: u64,
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
