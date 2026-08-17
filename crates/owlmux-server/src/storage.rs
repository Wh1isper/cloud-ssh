use std::time::Duration;

use sqlx::{Connection as _, PgConnection, PgPool, Row as _, postgres::PgPoolOptions};
use tokio::time::timeout;
use uuid::Uuid;

use crate::{build, config::Config, crypto};

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
const MIGRATION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct Database {
    critical: PgPool,
    ordinary: PgPool,
    deployment_id: Uuid,
}

impl Database {
    /// Apply migrations, initialize or validate the Deployment, and create serving pools.
    ///
    /// # Errors
    ///
    /// Returns a sanitized startup failure when `PostgreSQL`, schema, custody, or configuration
    /// validation fails.
    pub async fn bootstrap(config: &Config) -> Result<Self, StorageError> {
        migrate(config.database_url()).await?;
        let critical = pool(config.database_url(), 2).await?;
        let ordinary = pool(config.database_url(), 8).await?;
        let deployment_id = initialize_or_validate(&critical, config).await?;
        Ok(Self {
            critical,
            ordinary,
            deployment_id,
        })
    }

    #[must_use]
    pub const fn deployment_id(&self) -> Uuid {
        self.deployment_id
    }

    #[must_use]
    pub const fn critical(&self) -> &PgPool {
        &self.critical
    }

    #[must_use]
    pub const fn ordinary(&self) -> &PgPool {
        &self.ordinary
    }

    pub async fn close(&self) {
        self.ordinary.close().await;
        self.critical.close().await;
    }
}

async fn migrate(database_url: &str) -> Result<(), StorageError> {
    let mut connection = PgConnection::connect(database_url)
        .await
        .map_err(|_| StorageError::Unavailable)?;
    sqlx::query("SET lock_timeout = '5s'")
        .execute(&mut connection)
        .await
        .map_err(|_| StorageError::Migration)?;
    sqlx::query("SET statement_timeout = '20s'")
        .execute(&mut connection)
        .await
        .map_err(|_| StorageError::Migration)?;
    timeout(MIGRATION_TIMEOUT, MIGRATOR.run(&mut connection))
        .await
        .map_err(|_| StorageError::Migration)?
        .map_err(|_| StorageError::Migration)?;
    connection
        .close()
        .await
        .map_err(|_| StorageError::Migration)
}

async fn pool(database_url: &str, max_connections: u32) -> Result<PgPool, StorageError> {
    timeout(
        Duration::from_secs(5),
        PgPoolOptions::new()
            .max_connections(max_connections)
            .min_connections(0)
            .acquire_timeout(Duration::from_secs(3))
            .idle_timeout(Duration::from_mins(1))
            .max_lifetime(Duration::from_mins(10))
            .after_connect(|connection, _| {
                Box::pin(async move {
                    sqlx::query("SET statement_timeout = '5s'")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SET lock_timeout = '3s'")
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(database_url),
    )
    .await
    .map_err(|_| StorageError::Unavailable)?
    .map_err(|_| StorageError::Unavailable)
}

#[allow(clippy::too_many_lines)]
async fn initialize_or_validate(pool: &PgPool, config: &Config) -> Result<Uuid, StorageError> {
    let mut transaction = pool.begin().await.map_err(|_| StorageError::Unavailable)?;
    sqlx::query("SELECT pg_advisory_xact_lock(1877568117)")
        .execute(&mut *transaction)
        .await
        .map_err(|_| StorageError::Unavailable)?;
    let existing = sqlx::query(
        "SELECT id, config_epoch, server_build_id, profile, config_proof FROM deployment WHERE singleton = true FOR UPDATE",
    )
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| StorageError::Unavailable)?;

    let deployment_id = if let Some(row) = existing {
        let deployment_id: Uuid = row.try_get("id").map_err(|_| StorageError::Invariant)?;
        let stored_epoch: i64 = row
            .try_get("config_epoch")
            .map_err(|_| StorageError::Invariant)?;
        let stored_build: String = row
            .try_get("server_build_id")
            .map_err(|_| StorageError::Invariant)?;
        let stored_profile: String = row
            .try_get("profile")
            .map_err(|_| StorageError::Invariant)?;
        let stored_proof: Option<Vec<u8>> = row
            .try_get("config_proof")
            .map_err(|_| StorageError::Invariant)?;
        let expected_proof = config.configuration_proof(deployment_id).map(Vec::from);
        validate_credential_custody(&mut transaction, deployment_id, config).await?;
        if stored_epoch == config.config_epoch() {
            if stored_build != build::BUILD_ID
                || stored_profile != config.profile_database_value()
                || stored_proof != expected_proof
            {
                return Err(StorageError::ConfigurationMismatch);
            }
        } else if config.config_epoch() == stored_epoch + 1 {
            let valid_nodes: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM server_nodes WHERE lease_until > clock_timestamp())",
            )
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| StorageError::Unavailable)?;
            if valid_nodes {
                return Err(StorageError::ConfigurationTransitionBlocked);
            }
            sqlx::query(
                "UPDATE deployment SET config_epoch = $1, server_build_id = $2, profile = $3, config_proof = $4 WHERE singleton = true",
            )
            .bind(config.config_epoch())
            .bind(build::BUILD_ID)
            .bind(config.profile_database_value())
            .bind(expected_proof)
            .execute(&mut *transaction)
            .await
            .map_err(|_| StorageError::Unavailable)?;
            append_audit(
                &mut transaction,
                deployment_id,
                "deployment",
                None,
                None,
                "configuration_transition",
            )
            .await?;
        } else {
            return Err(StorageError::ConfigurationMismatch);
        }
        deployment_id
    } else {
        let deployment_id = Uuid::new_v4();
        let credential_id = Uuid::new_v4();
        let generated =
            crypto::generate_credential(config.encryption_key(), deployment_id, credential_id)
                .map_err(|_| StorageError::Custody)?;
        let config_proof = config.configuration_proof(deployment_id).map(Vec::from);
        sqlx::query(
            "INSERT INTO deployment (id, default_ssh_credential_id, config_epoch, server_build_id, profile, config_proof) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(deployment_id)
        .bind(credential_id)
        .bind(config.config_epoch())
        .bind(build::BUILD_ID)
        .bind(config.profile_database_value())
        .bind(config_proof)
        .execute(&mut *transaction)
        .await
        .map_err(|error| classify_write(&error))?;
        sqlx::query(
            "INSERT INTO ssh_credentials (id, deployment_id, name, public_key, public_fingerprint_sha256, encrypted_private_envelope) VALUES ($1, $2, 'Default', $3, $4, $5)",
        )
        .bind(credential_id)
        .bind(deployment_id)
        .bind(generated.public_key)
        .bind(generated.public_fingerprint_sha256)
        .bind(generated.encrypted_private_envelope)
        .execute(&mut *transaction)
        .await
        .map_err(|error| classify_write(&error))?;
        append_audit(
            &mut transaction,
            deployment_id,
            "deployment",
            None,
            None,
            "initialize",
        )
        .await?;
        append_audit(
            &mut transaction,
            deployment_id,
            "ssh_credential",
            None,
            Some(credential_id),
            "create_default",
        )
        .await?;
        deployment_id
    };

    transaction
        .commit()
        .await
        .map_err(|_| StorageError::Ambiguous)?;
    Ok(deployment_id)
}

async fn validate_credential_custody(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    deployment_id: Uuid,
    config: &Config,
) -> Result<(), StorageError> {
    let references_active: bool = sqlx::query_scalar(
        "SELECT NOT EXISTS(SELECT 1 FROM deployment d JOIN ssh_credentials c ON c.id = d.default_ssh_credential_id WHERE c.status <> 'active') AND NOT EXISTS(SELECT 1 FROM machines m JOIN ssh_credentials c ON c.id = m.ssh_credential_id WHERE c.status <> 'active')",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|_| StorageError::Unavailable)?;
    if !references_active {
        return Err(StorageError::Invariant);
    }
    let rows = sqlx::query("SELECT id, encrypted_private_envelope FROM ssh_credentials WHERE deployment_id = $1 AND status = 'active' ORDER BY id LIMIT 257")
        .bind(deployment_id)
        .fetch_all(&mut **transaction)
        .await
        .map_err(|_| StorageError::Unavailable)?;
    if rows.is_empty() || rows.len() > 256 {
        return Err(StorageError::Invariant);
    }
    for row in rows {
        let credential_id: Uuid = row.try_get("id").map_err(|_| StorageError::Invariant)?;
        let envelope: Vec<u8> = row
            .try_get("encrypted_private_envelope")
            .map_err(|_| StorageError::Invariant)?;
        let _private_key = crypto::open(
            config.encryption_key(),
            deployment_id,
            credential_id,
            &envelope,
        )
        .map_err(|_| StorageError::Custody)?;
    }
    Ok(())
}

pub(crate) async fn append_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    deployment_id: Uuid,
    resource_kind: &str,
    machine_id: Option<Uuid>,
    credential_id: Option<Uuid>,
    action: &str,
) -> Result<(), StorageError> {
    append_audit_outcome(
        transaction,
        deployment_id,
        resource_kind,
        machine_id,
        credential_id,
        action,
        "success",
    )
    .await
}

pub(crate) async fn append_audit_outcome(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    deployment_id: Uuid,
    resource_kind: &str,
    machine_id: Option<Uuid>,
    credential_id: Option<Uuid>,
    action: &str,
    outcome: &'static str,
) -> Result<(), StorageError> {
    insert_audit(
        &mut **transaction,
        deployment_id,
        resource_kind,
        machine_id,
        credential_id,
        action,
        outcome,
    )
    .await
}

pub(crate) async fn record_audit(
    pool: &sqlx::PgPool,
    deployment_id: Uuid,
    resource_kind: &str,
    machine_id: Option<Uuid>,
    credential_id: Option<Uuid>,
    action: &str,
    outcome: &'static str,
) -> Result<(), StorageError> {
    insert_audit(
        pool,
        deployment_id,
        resource_kind,
        machine_id,
        credential_id,
        action,
        outcome,
    )
    .await
}

async fn insert_audit<'e, E>(
    executor: E,
    deployment_id: Uuid,
    resource_kind: &str,
    machine_id: Option<Uuid>,
    credential_id: Option<Uuid>,
    action: &str,
    outcome: &'static str,
) -> Result<(), StorageError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    debug_assert!(matches!(outcome, "success" | "rejected" | "ambiguous"));
    sqlx::query(
        "INSERT INTO audit_events (id, deployment_id, resource_kind, machine_id, ssh_credential_id, action, outcome_class) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::new_v4())
    .bind(deployment_id)
    .bind(resource_kind)
    .bind(machine_id)
    .bind(credential_id)
    .bind(action)
    .bind(outcome)
    .execute(executor)
    .await
    .map_err(|error| classify_write(&error))?;
    Ok(())
}

fn classify_write(error: &sqlx::Error) -> StorageError {
    if error.as_database_error().is_some() {
        StorageError::Conflict
    } else {
        StorageError::Unavailable
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageError {
    Unavailable,
    Migration,
    Invariant,
    ConfigurationMismatch,
    ConfigurationTransitionBlocked,
    Custody,
    Conflict,
    Ambiguous,
    NotFound,
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "PostgreSQL is unavailable",
            Self::Migration => "database migration failed",
            Self::Invariant => "database invariant validation failed",
            Self::ConfigurationMismatch => "Deployment configuration does not match",
            Self::ConfigurationTransitionBlocked => {
                "Deployment configuration transition is blocked by a valid node lease"
            }
            Self::Custody => "SSH credential custody failed",
            Self::Conflict => "durable state conflict",
            Self::Ambiguous => "database commit outcome is ambiguous",
            Self::NotFound => "resource not found",
        })
    }
}

impl std::error::Error for StorageError {}

#[cfg(test)]
mod container_tests {
    use std::{collections::HashMap, env, ffi::OsString};

    use testcontainers::{
        ContainerAsync, GenericImage, ImageExt,
        core::{IntoContainerPort as _, WaitFor},
        runners::AsyncRunner as _,
    };

    use super::*;
    use crate::{config::Config, deployment::NodeLease};

    const POSTGRES_PORT: u16 = 5432;

    fn docker_required() -> bool {
        env::var("OWLMUX_REQUIRE_DOCKER").is_ok_and(|value| value == "1")
    }

    async fn postgres() -> Option<ContainerAsync<GenericImage>> {
        match GenericImage::new("postgres", "17.10-alpine")
            .with_exposed_port(POSTGRES_PORT.tcp())
            .with_wait_for(WaitFor::message_on_stderr(
                "database system is ready to accept connections",
            ))
            .with_env_var("POSTGRES_DB", "owlmux_test")
            .with_env_var("POSTGRES_USER", "owlmux")
            .with_env_var("POSTGRES_PASSWORD", "owlmux_test")
            .start()
            .await
        {
            Ok(container) => Some(container),
            Err(error) => {
                assert!(
                    !docker_required(),
                    "required PostgreSQL container failed: {error}"
                );
                eprintln!("skipping PostgreSQL container test: {error}");
                None
            }
        }
    }

    fn config(database_url: String) -> Config {
        config_with_encryption(database_url, "YmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmI")
    }

    fn config_with_encryption(database_url: String, encryption_key: &str) -> Config {
        let values = HashMap::<String, OsString>::from([
            ("OWLMUX_DATABASE_URL".to_owned(), database_url.into()),
            (
                "OWLMUX_API_KEY".to_owned(),
                "owlmux_sk_v1_YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE".into(),
            ),
            (
                "OWLMUX_SSH_KEY_ENCRYPTION_KEY".to_owned(),
                encryption_key.into(),
            ),
            ("OWLMUX_NODE_LEASE_TTL_SECONDS".to_owned(), "6".into()),
            (
                "OWLMUX_NODE_LEASE_SAFETY_MARGIN_SECONDS".to_owned(),
                "2".into(),
            ),
        ]);
        Config::load(|key| values.get(key).cloned()).expect("test config")
    }

    async fn assert_critical_pool_remains_available(database: &Database) {
        let mut ordinary_connections = Vec::with_capacity(8);
        for _ in 0..8 {
            ordinary_connections.push(
                database
                    .ordinary()
                    .acquire()
                    .await
                    .expect("ordinary connection"),
            );
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), database.ordinary().acquire())
                .await
                .is_err()
        );
        let critical_probe: i32 = sqlx::query_scalar("SELECT 1")
            .fetch_one(database.critical())
            .await
            .expect("critical probe while ordinary pool is full");
        assert_eq!(critical_probe, 1);
    }

    #[tokio::test]
    async fn container_postgres_bootstrap_is_idempotent_and_lease_is_fenced() {
        let Some(postgres) = postgres().await else {
            return;
        };
        let host = postgres.get_host().await.expect("host");
        let port = postgres
            .get_host_port_ipv4(POSTGRES_PORT)
            .await
            .expect("port");
        let database_url = format!("postgres://owlmux:owlmux_test@{host}:{port}/owlmux_test");
        let config = config(database_url);
        let database = Database::bootstrap(&config).await.expect("bootstrap");
        let deployment_id = database.deployment_id();
        let deployment_count: i64 = sqlx::query_scalar("SELECT count(*) FROM deployment")
            .fetch_one(database.ordinary())
            .await
            .expect("deployment count");
        let credential_count: i64 = sqlx::query_scalar("SELECT count(*) FROM ssh_credentials")
            .fetch_one(database.ordinary())
            .await
            .expect("credential count");
        assert_eq!(deployment_count, 1);
        assert_eq!(credential_count, 1);

        assert_critical_pool_remains_available(&database).await;

        let lease = NodeLease::register(database.clone(), &config)
            .await
            .expect("node registration");
        assert!(lease.is_ready());

        let credential_id: Uuid =
            sqlx::query_scalar("SELECT default_ssh_credential_id FROM deployment")
                .fetch_one(database.ordinary())
                .await
                .expect("default credential");
        let machine_id = Uuid::new_v4();
        let enrollment_id = Uuid::new_v4();
        let attempt_id = Uuid::new_v4();
        sqlx::query("INSERT INTO machines (id, deployment_id, ssh_credential_id, alias, lifecycle, target_account, tmux_path, tmux_socket_identity, host_identity) VALUES ($1,$2,$3,'release-history','pending','owlmux','/usr/bin/tmux','owlmux',$4)")
            .bind(machine_id)
            .bind(deployment_id)
            .bind(credential_id)
            .bind("ssh-ed25519 AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
            .execute(database.ordinary())
            .await
            .expect("machine");
        sqlx::query("INSERT INTO relay_enrollments (id, machine_id, token_digest, token_expires_at, consumed_at, status) VALUES ($1,$2,$3,clock_timestamp(),clock_timestamp(),'consumed')")
            .bind(enrollment_id)
            .bind(machine_id)
            .bind(vec![1_u8; 32])
            .execute(database.ordinary())
            .await
            .expect("enrollment");
        sqlx::query("INSERT INTO relay_verification_attempts (id, machine_id, enrollment_id, executing_incarnation_id, route_revision, status, deadline, completed_at) VALUES ($1,$2,$3,$4,1,'failed',clock_timestamp(),clock_timestamp())")
            .bind(attempt_id)
            .bind(machine_id)
            .bind(enrollment_id)
            .bind(lease.incarnation_id())
            .execute(database.ordinary())
            .await
            .expect("completed attempt");

        lease.begin_drain().await.expect("drain");
        assert!(!lease.is_ready());
        let fence = lease.fence_token();
        lease.release().await.expect("release");
        assert!(lease.check().is_err());
        assert!(fence.is_cancelled());
        let retained_executor: Option<Uuid> = sqlx::query_scalar(
            "SELECT executing_incarnation_id FROM relay_verification_attempts WHERE id = $1",
        )
        .bind(attempt_id)
        .fetch_one(database.ordinary())
        .await
        .expect("retained attempt");
        assert_eq!(retained_executor, None);
        let node_count: i64 = sqlx::query_scalar("SELECT count(*) FROM server_nodes")
            .fetch_one(database.ordinary())
            .await
            .expect("node count");
        assert_eq!(node_count, 0);
        database.close().await;

        let wrong_config = config_with_encryption(
            config.database_url().to_owned(),
            "Y2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2M",
        );
        assert!(matches!(
            Database::bootstrap(&wrong_config).await,
            Err(StorageError::Custody)
        ));
        let reopened = Database::bootstrap(&config).await.expect("reopen");
        assert_eq!(reopened.deployment_id(), deployment_id);
        let credential_count: i64 = sqlx::query_scalar("SELECT count(*) FROM ssh_credentials")
            .fetch_one(reopened.ordinary())
            .await
            .expect("credential count");
        assert_eq!(credential_count, 1);
        reopened.close().await;
    }
}
