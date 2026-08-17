use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

use crate::{
    build,
    clock::BootClock,
    config::Config,
    storage::{Database, StorageError, append_audit},
};

pub struct NodeLease {
    database: Database,
    incarnation_id: Uuid,
    config_epoch: i64,
    display_name: Option<String>,
    profile: &'static str,
    config_proof: Option<Vec<u8>>,
    internal_wss_url: Option<String>,
    ttl: Duration,
    safety_margin: Duration,
    clock: BootClock,
    hard_deadline_ns: AtomicU64,
    fenced: AtomicBool,
    fence_token: CancellationToken,
    draining: AtomicBool,
    dependency_ready: AtomicBool,
}

impl NodeLease {
    /// Register a fresh Server incarnation and install its conservative local deadline.
    ///
    /// # Errors
    ///
    /// Returns a bounded startup error when the clock, Deployment, or database predicate fails.
    pub async fn register(database: Database, config: &Config) -> Result<Arc<Self>, LeaseError> {
        let deployment_id = database.deployment_id();
        let lease = Arc::new(Self {
            database,
            incarnation_id: Uuid::new_v4(),
            config_epoch: config.config_epoch(),
            display_name: config.node_name().map(ToOwned::to_owned),
            profile: config.profile_database_value(),
            config_proof: config.configuration_proof(deployment_id).map(Vec::from),
            internal_wss_url: config
                .cluster()
                .map(|cluster| cluster.advertised_url().to_owned()),
            ttl: config.lease_ttl(),
            safety_margin: config.lease_safety_margin(),
            clock: BootClock::default(),
            hard_deadline_ns: AtomicU64::new(0),
            fenced: AtomicBool::new(false),
            fence_token: CancellationToken::new(),
            draining: AtomicBool::new(false),
            dependency_ready: AtomicBool::new(true),
        });
        let before = lease.clock.now().map_err(|_| LeaseError::Fenced)?;
        lease.register_database().await?;
        lease.install_deadline(before)?;
        Ok(lease)
    }

    #[must_use]
    pub const fn incarnation_id(&self) -> Uuid {
        self.incarnation_id
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.dependency_ready.load(Ordering::Acquire)
            && !self.draining.load(Ordering::Acquire)
            && self.check().is_ok()
    }

    #[must_use]
    pub fn fence_token(&self) -> CancellationToken {
        self.fence_token.clone()
    }

    /// Check the irreversible local authority fence directly against `CLOCK_BOOTTIME`.
    ///
    /// # Errors
    ///
    /// Returns [`LeaseError::Fenced`] after clock failure, backward movement, or deadline expiry.
    pub fn check(&self) -> Result<(), LeaseError> {
        if self.fenced.load(Ordering::Acquire) {
            return Err(LeaseError::Fenced);
        }
        let now = self.clock.now().map_err(|error| {
            warn!(%error, incarnation_id = %self.incarnation_id, "node lease clock validation failed; hard-fencing node");
            self.fence()
        })?;
        let deadline = self.hard_deadline_ns.load(Ordering::Acquire);
        if deadline == 0 || duration_ns(now) >= deadline {
            warn!(incarnation_id = %self.incarnation_id, "node lease local hard deadline expired; hard-fencing node");
            return Err(self.fence());
        }
        Ok(())
    }

    pub async fn run_renewals(self: Arc<Self>, cancellation: CancellationToken) {
        let interval = (self.ttl / 3).max(Duration::from_secs(1));
        loop {
            tokio::select! {
                () = cancellation.cancelled() => return,
                () = tokio::time::sleep(interval) => {}
            }
            if self.check().is_err() || self.draining.load(Ordering::Acquire) {
                return;
            }
            let Ok(before) = self.clock.now() else {
                self.fence();
                return;
            };
            match self.renew_database().await {
                Ok(()) => {
                    if let Err(error) = self.install_deadline(before) {
                        self.dependency_ready.store(false, Ordering::Release);
                        warn!(%error, incarnation_id = %self.incarnation_id, "node lease renewal could not install a local deadline");
                        return;
                    }
                    self.dependency_ready.store(true, Ordering::Release);
                }
                Err(error) => {
                    self.dependency_ready.store(false, Ordering::Release);
                    warn!(%error, "node lease renewal failed");
                }
            }
        }
    }

    /// Begin an exact-incarnation draining transition.
    ///
    /// # Errors
    ///
    /// Returns an authority error if the lease or Deployment predicate is no longer valid.
    pub async fn begin_drain(&self) -> Result<(), LeaseError> {
        self.draining.store(true, Ordering::Release);
        self.check()?;
        let mut transaction = self
            .database
            .critical()
            .begin()
            .await
            .map_err(|_| LeaseError::Database)?;
        validate_deployment(
            &mut transaction,
            self.config_epoch,
            self.profile,
            self.config_proof.as_deref(),
        )
        .await?;
        let changed = sqlx::query(
            "UPDATE server_nodes SET state = 'draining', renewed_at = clock_timestamp() WHERE incarnation_id = $1 AND state = 'serving' AND config_epoch = $2 AND server_build_id = $3 AND lease_until > clock_timestamp()",
        )
        .bind(self.incarnation_id)
        .bind(self.config_epoch)
        .bind(build::BUILD_ID)
        .execute(&mut *transaction)
        .await
        .map_err(|_| LeaseError::Database)?
        .rows_affected();
        if changed != 1 {
            return Err(self.fence());
        }
        append_audit(
            &mut transaction,
            self.database.deployment_id(),
            "server_node",
            None,
            None,
            "drain",
        )
        .await
        .map_err(map_storage)?;
        transaction
            .commit()
            .await
            .map_err(|_| LeaseError::Database)?;
        Ok(())
    }

    /// Release this exact drained incarnation from durable membership and fence it locally.
    ///
    /// # Errors
    ///
    /// Returns an authority or database error if the exact durable release cannot commit.
    pub async fn release(&self) -> Result<(), LeaseError> {
        let result = async {
            let mut transaction = self
                .database
                .critical()
                .begin()
                .await
                .map_err(|_| LeaseError::Database)?;
            validate_deployment(
            &mut transaction,
            self.config_epoch,
            self.profile,
            self.config_proof.as_deref(),
        )
        .await?;
            let changed = sqlx::query(
                "DELETE FROM server_nodes WHERE incarnation_id = $1 AND state = 'draining' AND config_epoch = $2 AND server_build_id = $3",
            )
            .bind(self.incarnation_id)
            .bind(self.config_epoch)
            .bind(build::BUILD_ID)
            .execute(&mut *transaction)
            .await
            .map_err(|_| LeaseError::Database)?
            .rows_affected();
            if changed != 1 {
                return Err(LeaseError::Fenced);
            }
            append_audit(
                &mut transaction,
                self.database.deployment_id(),
                "server_node",
                None,
                None,
                "release",
            )
            .await
            .map_err(map_storage)?;
            transaction.commit().await.map_err(|_| LeaseError::Database)
        }
        .await;
        self.fence();
        result
    }

    async fn register_database(&self) -> Result<(), LeaseError> {
        let mut transaction = self
            .database
            .critical()
            .begin()
            .await
            .map_err(|_| LeaseError::Database)?;
        let deployment_id = validate_deployment(
            &mut transaction,
            self.config_epoch,
            self.profile,
            self.config_proof.as_deref(),
        )
        .await?;
        if self.profile == "single_node" {
            let another_valid: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM server_nodes WHERE lease_until > clock_timestamp())",
            )
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| LeaseError::Database)?;
            if another_valid {
                return Err(LeaseError::ConfigurationMismatch);
            }
        }
        let ttl = i64::try_from(self.ttl.as_secs()).map_err(|_| LeaseError::Database)?;
        sqlx::query(
            "INSERT INTO server_nodes (incarnation_id, display_name, state, config_epoch, server_build_id, relay_protocol_version, internal_wss_url, lease_until) VALUES ($1, $2, 'serving', $3, $4, 1, $5, clock_timestamp() + $6 * interval '1 second')",
        )
        .bind(self.incarnation_id)
        .bind(&self.display_name)
        .bind(self.config_epoch)
        .bind(build::BUILD_ID)
        .bind(&self.internal_wss_url)
        .bind(ttl)
        .execute(&mut *transaction)
        .await
        .map_err(|_| LeaseError::Database)?;
        append_audit(
            &mut transaction,
            deployment_id,
            "server_node",
            None,
            None,
            "register",
        )
        .await
        .map_err(map_storage)?;
        transaction.commit().await.map_err(|_| LeaseError::Database)
    }

    async fn renew_database(&self) -> Result<(), LeaseError> {
        let mut transaction = self
            .database
            .critical()
            .begin()
            .await
            .map_err(|_| LeaseError::Database)?;
        validate_deployment(
            &mut transaction,
            self.config_epoch,
            self.profile,
            self.config_proof.as_deref(),
        )
        .await?;
        let ttl = i64::try_from(self.ttl.as_secs()).map_err(|_| LeaseError::Database)?;
        let changed = sqlx::query(
            "UPDATE server_nodes SET lease_until = clock_timestamp() + $1 * interval '1 second', renewed_at = clock_timestamp() WHERE incarnation_id = $2 AND state = 'serving' AND config_epoch = $3 AND server_build_id = $4",
        )
        .bind(ttl)
        .bind(self.incarnation_id)
        .bind(self.config_epoch)
        .bind(build::BUILD_ID)
        .execute(&mut *transaction)
        .await
        .map_err(|_| LeaseError::Database)?
        .rows_affected();
        if changed != 1 {
            return Err(self.fence());
        }
        transaction.commit().await.map_err(|_| LeaseError::Database)
    }

    fn install_deadline(&self, before_request: Duration) -> Result<(), LeaseError> {
        if self.fenced.load(Ordering::Acquire) {
            return Err(LeaseError::Fenced);
        }
        let candidate = (before_request + self.ttl)
            .checked_sub(self.safety_margin)
            .ok_or_else(|| self.fence())?;
        let now = self.clock.now().map_err(|_| self.fence())?;
        let previous = self.hard_deadline_ns.load(Ordering::Acquire);
        if (previous != 0 && duration_ns(now) >= previous) || now >= candidate {
            return Err(self.fence());
        }
        self.hard_deadline_ns
            .store(duration_ns(candidate), Ordering::Release);
        Ok(())
    }

    pub(crate) fn hard_fence(&self) {
        let _ = self.fence();
    }

    fn fence(&self) -> LeaseError {
        if !self.fenced.swap(true, Ordering::AcqRel) {
            self.fence_token.cancel();
        }
        LeaseError::Fenced
    }
}

async fn validate_deployment(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    config_epoch: i64,
    profile: &str,
    config_proof: Option<&[u8]>,
) -> Result<Uuid, LeaseError> {
    let deployment_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM deployment WHERE singleton = true AND config_epoch = $1 AND server_build_id = $2 AND relay_protocol_version = 1 AND profile = $3 AND config_proof IS NOT DISTINCT FROM $4 FOR UPDATE",
    )
    .bind(config_epoch)
    .bind(build::BUILD_ID)
    .bind(profile)
    .bind(config_proof)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| LeaseError::Database)?
    .ok_or(LeaseError::ConfigurationMismatch)?;
    Ok(deployment_id)
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn map_storage(error: StorageError) -> LeaseError {
    match error {
        StorageError::ConfigurationMismatch => LeaseError::ConfigurationMismatch,
        _ => LeaseError::Database,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaseError {
    Fenced,
    Database,
    ConfigurationMismatch,
}

impl std::fmt::Display for LeaseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Fenced => "Server incarnation is fenced",
            Self::Database => "node lease database operation failed",
            Self::ConfigurationMismatch => "node lease configuration mismatch",
        })
    }
}
impl std::error::Error for LeaseError {}
