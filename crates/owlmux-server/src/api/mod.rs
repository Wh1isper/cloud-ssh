use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, Path, State},
    http::{Method, Request, StatusCode, header},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand_core::RngCore as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use sqlx::Row as _;
use tokio::sync::OwnedSemaphorePermit;
use uuid::Uuid;

use crate::{
    build, crypto, relay,
    service::ServerState,
    ssh,
    storage::{StorageError, append_audit},
};

const ENROLLMENT_PREFIX: &str = "owlmux_enroll_v1_";
const TOKEN_TTL_SECONDS: i64 = 900;
const MAX_MACHINES: i64 = 1_024;

#[derive(Clone)]
struct MutationAdmission {
    _permit: Arc<OwnedSemaphorePermit>,
}

impl MutationAdmission {
    fn new(permit: OwnedSemaphorePermit) -> Self {
        Self {
            _permit: Arc::new(permit),
        }
    }
}

pub fn router(state: Arc<ServerState>) -> Router<Arc<ServerState>> {
    Router::new()
        .route("/deployment", get(get_deployment))
        .route("/metrics", get(get_metrics))
        .route("/audit-events", get(list_audit_events))
        .route(
            "/ssh-credentials",
            get(list_credentials).post(create_credential),
        )
        .route("/ssh-credentials/{credential_id}", patch(rename_credential))
        .route(
            "/ssh-credentials/{credential_id}/default",
            post(set_default_credential),
        )
        .route("/ssh-credentials/reset", post(reset_default_credential))
        .route(
            "/ssh-credentials/{credential_id}/retire",
            post(retire_credential),
        )
        .route("/machines", get(list_machines).post(create_machine))
        .route(
            "/machines/{machine_id}",
            get(get_machine).patch(rename_machine),
        )
        .route("/machines/{machine_id}/re-enroll", post(re_enroll_machine))
        .route(
            "/machines/{machine_id}/relay/revoke",
            post(revoke_machine_relay),
        )
        .route("/machines/{machine_id}/disable", post(disable_machine))
        .route(
            "/machines/{machine_id}/ssh-credential",
            patch(rebind_machine_credential),
        )
        .route(
            "/machines/{machine_id}/enrollment-token",
            post(issue_enrollment_token).delete(cancel_enrollment_token),
        )
        .layer(from_fn_with_state(state, require_auth))
}

async fn require_auth(
    State(state): State<Arc<ServerState>>,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let Some(peer) = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|connect| connect.0)
    else {
        return ApiError::temporarily_unavailable().into_response();
    };
    let Some(source_permit) = state.source_admission.try_acquire(peer.ip()) else {
        return ApiError::temporarily_unavailable().into_response();
    };
    let Ok(attempt_permit) = state.preauth_attempt_limit.clone().try_acquire_owned() else {
        return ApiError::temporarily_unavailable().into_response();
    };
    if state.lease.check().is_err() {
        return ApiError::temporarily_unavailable().into_response();
    }
    let candidate = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty() && !value.contains(char::is_whitespace));
    let authenticated = candidate.is_some_and(|value| state.config.api_key().verify(value));
    request.headers_mut().remove(header::AUTHORIZATION);
    drop(source_permit);
    drop(attempt_permit);
    if !authenticated {
        state.observability.api_auth_rejected();
        return ApiError::unauthenticated().into_response();
    }
    state.observability.api_authenticated();
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        let Ok(permit) = state.api_mutation_limit.clone().try_acquire_owned() else {
            state.observability.api_mutation_overloaded();
            return ApiError::temporarily_unavailable().into_response();
        };
        request
            .extensions_mut()
            .insert(MutationAdmission::new(permit));
    }
    next.run(request).await
}

#[derive(Serialize)]
struct DeploymentPresentation {
    deployment_id: Uuid,
    default_ssh_credential_id: Uuid,
    config_epoch: i64,
    server_build_id: &'static str,
    profile: &'static str,
}

async fn get_deployment(
    State(state): State<Arc<ServerState>>,
) -> ApiResult<Json<DeploymentPresentation>> {
    let row = sqlx::query(
        "SELECT id, default_ssh_credential_id, config_epoch FROM deployment WHERE singleton = true",
    )
    .fetch_one(state.database.ordinary())
    .await
    .map_err(map_read)?;
    Ok(Json(DeploymentPresentation {
        deployment_id: row.try_get("id").map_err(|_| ApiError::internal())?,
        default_ssh_credential_id: row
            .try_get("default_ssh_credential_id")
            .map_err(|_| ApiError::internal())?,
        config_epoch: row
            .try_get("config_epoch")
            .map_err(|_| ApiError::internal())?,
        server_build_id: build::BUILD_ID,
        profile: state.config.profile_database_value(),
    }))
}

#[derive(Serialize)]
struct MetricsSnapshot {
    node_ready: bool,
    api_authenticated_requests_total: u64,
    api_auth_rejections_total: u64,
    api_mutation_overloads_total: u64,
    owner_local_resolutions_total: u64,
    owner_remote_resolutions_total: u64,
    owner_absent_resolutions_total: u64,
    owner_resolution_failures_total: u64,
}

async fn get_metrics(State(state): State<Arc<ServerState>>) -> Json<MetricsSnapshot> {
    let counters = state.observability.snapshot();
    Json(MetricsSnapshot {
        node_ready: state.lease.is_ready(),
        api_authenticated_requests_total: bounded_metric(counters.api_authenticated_requests_total),
        api_auth_rejections_total: bounded_metric(counters.api_auth_rejections_total),
        api_mutation_overloads_total: bounded_metric(counters.api_mutation_overloads_total),
        owner_local_resolutions_total: bounded_metric(counters.owner_local_resolutions_total),
        owner_remote_resolutions_total: bounded_metric(counters.owner_remote_resolutions_total),
        owner_absent_resolutions_total: bounded_metric(counters.owner_absent_resolutions_total),
        owner_resolution_failures_total: bounded_metric(counters.owner_resolution_failures_total),
    })
}

fn bounded_metric(value: u64) -> u64 {
    value.min(9_007_199_254_740_991)
}

#[derive(Serialize)]
struct AuditEventSummary {
    audit_event_id: Uuid,
    resource_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    machine_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ssh_credential_id: Option<Uuid>,
    action: String,
    outcome_class: String,
    occurred_at: String,
}

async fn list_audit_events(
    State(state): State<Arc<ServerState>>,
) -> ApiResult<Json<Vec<AuditEventSummary>>> {
    let rows = sqlx::query(
        "SELECT id, resource_kind, machine_id, ssh_credential_id, action, outcome_class, to_char(occurred_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS occurred_at FROM audit_events WHERE deployment_id = $1 ORDER BY audit_events.occurred_at DESC, id DESC LIMIT 200",
    )
    .bind(state.database.deployment_id())
    .fetch_all(state.database.ordinary())
    .await
    .map_err(map_read)?;
    rows.into_iter()
        .map(|row| {
            Ok(AuditEventSummary {
                audit_event_id: row.try_get("id").map_err(|_| ApiError::internal())?,
                resource_kind: row
                    .try_get("resource_kind")
                    .map_err(|_| ApiError::internal())?,
                machine_id: row
                    .try_get("machine_id")
                    .map_err(|_| ApiError::internal())?,
                ssh_credential_id: row
                    .try_get("ssh_credential_id")
                    .map_err(|_| ApiError::internal())?,
                action: row.try_get("action").map_err(|_| ApiError::internal())?,
                outcome_class: row
                    .try_get("outcome_class")
                    .map_err(|_| ApiError::internal())?,
                occurred_at: row
                    .try_get("occurred_at")
                    .map_err(|_| ApiError::internal())?,
            })
        })
        .collect::<ApiResult<Vec<_>>>()
        .map(Json)
}

#[derive(Serialize)]
struct CredentialSummary {
    ssh_credential_id: Uuid,
    name: String,
    public_key: String,
    public_fingerprint_sha256: String,
    is_default: bool,
    bound_machine_count: i64,
    status: String,
}

async fn list_credentials(
    State(state): State<Arc<ServerState>>,
) -> ApiResult<Json<Vec<CredentialSummary>>> {
    let rows = sqlx::query(
        "SELECT c.id, c.name, c.public_key, c.public_fingerprint_sha256, c.status, (c.id = d.default_ssh_credential_id) AS is_default, count(m.id)::bigint AS bound_machine_count FROM ssh_credentials c JOIN deployment d ON d.id = c.deployment_id LEFT JOIN machines m ON m.ssh_credential_id = c.id GROUP BY c.id, d.default_ssh_credential_id ORDER BY c.created_at, c.id LIMIT 256",
    ).fetch_all(state.database.ordinary()).await.map_err(map_read)?;
    rows.into_iter()
        .map(|row| {
            Ok(CredentialSummary {
                ssh_credential_id: row.try_get("id").map_err(|_| ApiError::internal())?,
                name: row.try_get("name").map_err(|_| ApiError::internal())?,
                public_key: row
                    .try_get("public_key")
                    .map_err(|_| ApiError::internal())?,
                public_fingerprint_sha256: row
                    .try_get("public_fingerprint_sha256")
                    .map_err(|_| ApiError::internal())?,
                is_default: row
                    .try_get("is_default")
                    .map_err(|_| ApiError::internal())?,
                bound_machine_count: row
                    .try_get("bound_machine_count")
                    .map_err(|_| ApiError::internal())?,
                status: row.try_get("status").map_err(|_| ApiError::internal())?,
            })
        })
        .collect::<ApiResult<Vec<_>>>()
        .map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialNameInput {
    name: String,
}

async fn create_credential(
    State(state): State<Arc<ServerState>>,
    Json(input): Json<CredentialNameInput>,
) -> ApiResult<(StatusCode, Json<CredentialSummary>)> {
    validate_name(&input.name)?;
    let credential_id = Uuid::new_v4();
    let generated = crypto::generate_credential(
        state.config.encryption_key(),
        state.database.deployment_id(),
        credential_id,
    )
    .map_err(|_| ApiError::internal())?;
    let mut transaction = state
        .database
        .ordinary()
        .begin()
        .await
        .map_err(|error| map_write(&error))?;
    lock_current_deployment(&mut transaction, &state).await?;
    enforce_credential_limit(&mut transaction, &state).await?;
    sqlx::query("INSERT INTO ssh_credentials (id, deployment_id, name, public_key, public_fingerprint_sha256, encrypted_private_envelope) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(credential_id).bind(state.database.deployment_id()).bind(&input.name).bind(&generated.public_key)
        .bind(&generated.public_fingerprint_sha256).bind(generated.encrypted_private_envelope)
        .execute(&mut *transaction).await.map_err(|error| map_write(&error))?;
    append_audit(
        &mut transaction,
        state.database.deployment_id(),
        "ssh_credential",
        None,
        Some(credential_id),
        "create",
    )
    .await
    .map_err(map_storage)?;
    transaction
        .commit()
        .await
        .map_err(|_| ApiError::ambiguous())?;
    Ok((
        StatusCode::CREATED,
        Json(CredentialSummary {
            ssh_credential_id: credential_id,
            name: input.name,
            public_key: generated.public_key,
            public_fingerprint_sha256: generated.public_fingerprint_sha256,
            is_default: false,
            bound_machine_count: 0,
            status: "active".to_owned(),
        }),
    ))
}

async fn rename_credential(
    State(state): State<Arc<ServerState>>,
    Path(credential_id): Path<Uuid>,
    Json(input): Json<CredentialNameInput>,
) -> ApiResult<Json<CredentialSummary>> {
    validate_name(&input.name)?;
    let mut transaction = state
        .database
        .ordinary()
        .begin()
        .await
        .map_err(|error| map_write(&error))?;
    lock_current_deployment(&mut transaction, &state).await?;
    let changed = sqlx::query("UPDATE ssh_credentials SET name = $1, updated_at = clock_timestamp() WHERE id = $2 RETURNING id")
        .bind(&input.name)
        .bind(credential_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| map_write(&error))?;
    if changed.is_none() {
        return Err(ApiError::not_found());
    }
    append_audit(
        &mut transaction,
        state.database.deployment_id(),
        "ssh_credential",
        None,
        Some(credential_id),
        "rename",
    )
    .await
    .map_err(map_storage)?;
    transaction
        .commit()
        .await
        .map_err(|_| ApiError::ambiguous())?;
    credential_by_id(&state, credential_id).await.map(Json)
}

async fn set_default_credential(
    State(state): State<Arc<ServerState>>,
    Path(credential_id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let mut transaction = state
        .database
        .ordinary()
        .begin()
        .await
        .map_err(|error| map_write(&error))?;
    lock_current_deployment(&mut transaction, &state).await?;
    let active = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM ssh_credentials WHERE id = $1 AND status = 'active' FOR UPDATE",
    )
    .bind(credential_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_read)?;
    if active.is_none() {
        return Err(ApiError::not_found());
    }
    sqlx::query("UPDATE deployment SET default_ssh_credential_id = $1 WHERE singleton = true")
        .bind(credential_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| map_write(&error))?;
    append_audit(
        &mut transaction,
        state.database.deployment_id(),
        "ssh_credential",
        None,
        Some(credential_id),
        "set_default",
    )
    .await
    .map_err(map_storage)?;
    transaction
        .commit()
        .await
        .map_err(|_| ApiError::ambiguous())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reset_default_credential(
    State(state): State<Arc<ServerState>>,
    Json(input): Json<CredentialNameInput>,
) -> ApiResult<(StatusCode, Json<CredentialSummary>)> {
    validate_name(&input.name)?;
    let credential_id = Uuid::new_v4();
    let generated = crypto::generate_credential(
        state.config.encryption_key(),
        state.database.deployment_id(),
        credential_id,
    )
    .map_err(|_| ApiError::internal())?;
    let mut transaction = state
        .database
        .ordinary()
        .begin()
        .await
        .map_err(|error| map_write(&error))?;
    lock_current_deployment(&mut transaction, &state).await?;
    enforce_credential_limit(&mut transaction, &state).await?;
    sqlx::query("INSERT INTO ssh_credentials (id, deployment_id, name, public_key, public_fingerprint_sha256, encrypted_private_envelope) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(credential_id).bind(state.database.deployment_id()).bind(&input.name).bind(&generated.public_key).bind(&generated.public_fingerprint_sha256).bind(generated.encrypted_private_envelope)
        .execute(&mut *transaction).await.map_err(|error| map_write(&error))?;
    sqlx::query("UPDATE deployment SET default_ssh_credential_id = $1 WHERE singleton = true")
        .bind(credential_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| map_write(&error))?;
    append_audit(
        &mut transaction,
        state.database.deployment_id(),
        "ssh_credential",
        None,
        Some(credential_id),
        "reset_default",
    )
    .await
    .map_err(map_storage)?;
    transaction
        .commit()
        .await
        .map_err(|_| ApiError::ambiguous())?;
    Ok((
        StatusCode::CREATED,
        Json(CredentialSummary {
            ssh_credential_id: credential_id,
            name: input.name,
            public_key: generated.public_key,
            public_fingerprint_sha256: generated.public_fingerprint_sha256,
            is_default: true,
            bound_machine_count: 0,
            status: "active".to_owned(),
        }),
    ))
}

async fn retire_credential(
    State(state): State<Arc<ServerState>>,
    Path(credential_id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let mut transaction = state
        .database
        .ordinary()
        .begin()
        .await
        .map_err(|error| map_write(&error))?;
    lock_current_deployment(&mut transaction, &state).await?;
    sqlx::query("SELECT id FROM ssh_credentials WHERE id = $1 FOR UPDATE")
        .bind(credential_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_read)?
        .ok_or_else(ApiError::not_found)?;
    let changed = sqlx::query("UPDATE ssh_credentials c SET status = 'retired', updated_at = clock_timestamp() WHERE c.id = $1 AND c.status = 'active' AND NOT EXISTS (SELECT 1 FROM deployment d WHERE d.default_ssh_credential_id = c.id) AND NOT EXISTS (SELECT 1 FROM machines m WHERE m.ssh_credential_id = c.id)")
        .bind(credential_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| map_write(&error))?
        .rows_affected();
    if changed != 1 {
        return Err(ApiError::conflict(
            "credential_in_use",
            "Credential is default, bound, retired, or missing.",
        ));
    }
    append_audit(
        &mut transaction,
        state.database.deployment_id(),
        "ssh_credential",
        None,
        Some(credential_id),
        "retire",
    )
    .await
    .map_err(map_storage)?;
    transaction
        .commit()
        .await
        .map_err(|_| ApiError::ambiguous())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn credential_by_id(
    state: &ServerState,
    credential_id: Uuid,
) -> ApiResult<CredentialSummary> {
    let row = sqlx::query("SELECT c.id, c.name, c.public_key, c.public_fingerprint_sha256, c.status, (c.id = d.default_ssh_credential_id) AS is_default, count(m.id)::bigint AS bound_machine_count FROM ssh_credentials c JOIN deployment d ON d.id = c.deployment_id LEFT JOIN machines m ON m.ssh_credential_id = c.id WHERE c.id = $1 GROUP BY c.id, d.default_ssh_credential_id")
        .bind(credential_id).fetch_optional(state.database.ordinary()).await.map_err(map_read)?.ok_or_else(ApiError::not_found)?;
    Ok(CredentialSummary {
        ssh_credential_id: row.try_get("id").map_err(|_| ApiError::internal())?,
        name: row.try_get("name").map_err(|_| ApiError::internal())?,
        public_key: row
            .try_get("public_key")
            .map_err(|_| ApiError::internal())?,
        public_fingerprint_sha256: row
            .try_get("public_fingerprint_sha256")
            .map_err(|_| ApiError::internal())?,
        is_default: row
            .try_get("is_default")
            .map_err(|_| ApiError::internal())?,
        bound_machine_count: row
            .try_get("bound_machine_count")
            .map_err(|_| ApiError::internal())?,
        status: row.try_get("status").map_err(|_| ApiError::internal())?,
    })
}

#[derive(Serialize)]
struct MachineSummary {
    machine_id: Uuid,
    ssh_credential_id: Uuid,
    alias: String,
    lifecycle: String,
    reachability: &'static str,
}

#[derive(Serialize)]
struct MachineDetail {
    #[serde(flatten)]
    summary: MachineSummary,
    target_account: String,
    tmux_path: String,
    tmux_socket_identity: String,
    host_identity: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateMachineInput {
    alias: String,
    target_account: String,
    tmux_path: String,
    tmux_socket_identity: String,
    host_identity: String,
    ssh_credential_id: Option<Uuid>,
}

#[derive(Serialize)]
struct MachineCreated {
    machine: MachineSummary,
    enrollment_token: String,
    enrollment_expires_in: i64,
}

async fn list_machines(
    State(state): State<Arc<ServerState>>,
) -> ApiResult<Json<Vec<MachineSummary>>> {
    relay::recover_expired_attempts(&state, None)
        .await
        .map_err(|_| ApiError::temporarily_unavailable())?;
    let rows = sqlx::query("SELECT m.id, m.ssh_credential_id, m.alias, m.lifecycle, EXISTS(SELECT 1 FROM machine_owners o JOIN server_nodes n ON n.incarnation_id = o.owner_incarnation_id JOIN relay_bindings b ON b.machine_id = m.id AND b.status = 'active' AND b.route_revision = m.route_revision WHERE o.machine_id = m.id AND o.route_revision = m.route_revision AND o.connection_epoch > 0 AND n.state IN ('serving', 'draining') AND n.config_epoch = $1 AND n.server_build_id = $2 AND n.relay_protocol_version = 1 AND n.lease_until > clock_timestamp()) AS owner_valid FROM machines m ORDER BY m.created_at, m.id LIMIT 1024")
        .bind(state.config.config_epoch())
        .bind(build::BUILD_ID)
        .fetch_all(state.database.ordinary()).await.map_err(map_read)?;
    let mut machines = Vec::with_capacity(rows.len());
    for row in rows {
        let connected: bool = row
            .try_get("owner_valid")
            .map_err(|_| ApiError::internal())?;
        machines.push(machine_from_row(&row, connected)?);
    }
    Ok(Json(machines))
}

async fn get_machine(
    State(state): State<Arc<ServerState>>,
    Path(machine_id): Path<Uuid>,
) -> ApiResult<Json<MachineDetail>> {
    relay::recover_expired_attempts(&state, Some(machine_id))
        .await
        .map_err(|_| ApiError::temporarily_unavailable())?;
    let row = sqlx::query("SELECT m.id, m.ssh_credential_id, m.alias, m.lifecycle, m.target_account, m.tmux_path, m.tmux_socket_identity, m.host_identity, EXISTS(SELECT 1 FROM machine_owners o JOIN server_nodes n ON n.incarnation_id = o.owner_incarnation_id JOIN relay_bindings b ON b.machine_id = m.id AND b.status = 'active' AND b.route_revision = m.route_revision WHERE o.machine_id = m.id AND o.route_revision = m.route_revision AND o.connection_epoch > 0 AND n.state IN ('serving', 'draining') AND n.config_epoch = $2 AND n.server_build_id = $3 AND n.relay_protocol_version = 1 AND n.lease_until > clock_timestamp()) AS owner_valid FROM machines m WHERE m.id = $1")
        .bind(machine_id)
        .bind(state.config.config_epoch())
        .bind(build::BUILD_ID)
        .fetch_optional(state.database.ordinary()).await.map_err(map_read)?.ok_or_else(ApiError::not_found)?;
    let connected: bool = row
        .try_get("owner_valid")
        .map_err(|_| ApiError::internal())?;
    Ok(Json(MachineDetail {
        summary: machine_from_row(&row, connected)?,
        target_account: row
            .try_get("target_account")
            .map_err(|_| ApiError::internal())?,
        tmux_path: row.try_get("tmux_path").map_err(|_| ApiError::internal())?,
        tmux_socket_identity: row
            .try_get("tmux_socket_identity")
            .map_err(|_| ApiError::internal())?,
        host_identity: row
            .try_get("host_identity")
            .map_err(|_| ApiError::internal())?,
    }))
}

async fn create_machine(
    State(state): State<Arc<ServerState>>,
    Json(input): Json<CreateMachineInput>,
) -> ApiResult<(StatusCode, Json<MachineCreated>)> {
    validate_machine(&input)?;
    let machine_id = Uuid::new_v4();
    let (token, digest) = enrollment_token();
    let mut transaction = state
        .database
        .ordinary()
        .begin()
        .await
        .map_err(|error| map_write(&error))?;
    lock_current_deployment(&mut transaction, &state).await?;
    let machine_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM machines WHERE deployment_id = $1")
            .bind(state.database.deployment_id())
            .fetch_one(&mut *transaction)
            .await
            .map_err(map_read)?;
    if machine_count >= MAX_MACHINES {
        return Err(ApiError::conflict(
            "machine_limit",
            "The Deployment Machine limit has been reached.",
        ));
    }
    let credential_id = if let Some(id) = input.ssh_credential_id {
        id
    } else {
        sqlx::query_scalar(
            "SELECT default_ssh_credential_id FROM deployment WHERE singleton = true",
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(map_read)?
    };
    let active = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM ssh_credentials WHERE id = $1 AND status = 'active' FOR UPDATE",
    )
    .bind(credential_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_read)?;
    if active.is_none() {
        return Err(ApiError::not_found());
    }
    sqlx::query("INSERT INTO machines (id, deployment_id, ssh_credential_id, alias, target_account, tmux_path, tmux_socket_identity, host_identity) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(machine_id).bind(state.database.deployment_id()).bind(credential_id).bind(&input.alias).bind(&input.target_account).bind(&input.tmux_path).bind(&input.tmux_socket_identity).bind(&input.host_identity)
        .execute(&mut *transaction).await.map_err(|error| map_write(&error))?;
    sqlx::query("INSERT INTO machine_owners (machine_id, route_revision) VALUES ($1, 1)")
        .bind(machine_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| map_write(&error))?;
    insert_enrollment(&mut transaction, machine_id, digest).await?;
    append_audit(
        &mut transaction,
        state.database.deployment_id(),
        "machine",
        Some(machine_id),
        None,
        "create",
    )
    .await
    .map_err(map_storage)?;
    transaction
        .commit()
        .await
        .map_err(|_| ApiError::ambiguous())?;
    Ok((
        StatusCode::CREATED,
        Json(MachineCreated {
            machine: MachineSummary {
                machine_id,
                ssh_credential_id: credential_id,
                alias: input.alias,
                lifecycle: "pending".to_owned(),
                reachability: "unknown",
            },
            enrollment_token: token,
            enrollment_expires_in: TOKEN_TTL_SECONDS,
        }),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineAliasInput {
    alias: String,
}

async fn rename_machine(
    State(state): State<Arc<ServerState>>,
    Path(machine_id): Path<Uuid>,
    Json(input): Json<MachineAliasInput>,
) -> ApiResult<StatusCode> {
    validate_name(&input.alias)?;
    let mut transaction = state
        .database
        .ordinary()
        .begin()
        .await
        .map_err(|error| map_write(&error))?;
    lock_current_deployment(&mut transaction, &state).await?;
    let changed =
        sqlx::query("UPDATE machines SET alias = $1, updated_at = clock_timestamp() WHERE id = $2")
            .bind(input.alias)
            .bind(machine_id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| map_write(&error))?
            .rows_affected();
    if changed != 1 {
        return Err(ApiError::not_found());
    }
    append_audit(
        &mut transaction,
        state.database.deployment_id(),
        "machine",
        Some(machine_id),
        None,
        "rename",
    )
    .await
    .map_err(map_storage)?;
    transaction
        .commit()
        .await
        .map_err(|_| ApiError::ambiguous())?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineCredentialInput {
    ssh_credential_id: Uuid,
}

async fn rebind_machine_credential(
    State(state): State<Arc<ServerState>>,
    Path(machine_id): Path<Uuid>,
    Json(input): Json<MachineCredentialInput>,
) -> ApiResult<StatusCode> {
    let mut transaction = state
        .database
        .ordinary()
        .begin()
        .await
        .map_err(|error| map_write(&error))?;
    lock_current_deployment(&mut transaction, &state).await?;
    let machine = sqlx::query(
        "SELECT ssh_credential_id, credential_revision, lifecycle FROM machines WHERE id = $1 FOR UPDATE",
    )
    .bind(machine_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_read)?
    .ok_or_else(ApiError::not_found)?;
    let lifecycle: String = machine
        .try_get("lifecycle")
        .map_err(|_| ApiError::internal())?;
    if lifecycle != "active" {
        return Err(ApiError::conflict(
            "invalid_lifecycle",
            "Only an active Machine can be rebound.",
        ));
    }
    let current_credential_id: Uuid = machine
        .try_get("ssh_credential_id")
        .map_err(|_| ApiError::internal())?;
    if current_credential_id == input.ssh_credential_id {
        return Err(ApiError::conflict(
            "conflict",
            "Machine already uses that SSH credential.",
        ));
    }
    let credential_revision: i64 = machine
        .try_get("credential_revision")
        .map_err(|_| ApiError::internal())?;
    let next_revision = credential_revision
        .checked_add(1)
        .ok_or_else(ApiError::internal)?;
    let credential_exists = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM ssh_credentials WHERE id = $1 AND status = 'active' FOR UPDATE",
    )
    .bind(input.ssh_credential_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_read)?;
    if credential_exists.is_none() {
        return Err(ApiError::not_found());
    }
    let changed = sqlx::query(
        "UPDATE machines SET ssh_credential_id = $1, credential_revision = $2, updated_at = clock_timestamp() WHERE id = $3 AND lifecycle = 'active' AND credential_revision = $4",
    )
    .bind(input.ssh_credential_id)
    .bind(next_revision)
    .bind(machine_id)
    .bind(credential_revision)
    .execute(&mut *transaction)
    .await
    .map_err(|error| map_write(&error))?
    .rows_affected();
    if changed != 1 {
        return Err(ApiError::temporarily_unavailable());
    }
    append_audit(
        &mut transaction,
        state.database.deployment_id(),
        "machine",
        Some(machine_id),
        Some(input.ssh_credential_id),
        "rebind",
    )
    .await
    .map_err(map_storage)?;
    transaction
        .commit()
        .await
        .map_err(|_| ApiError::ambiguous())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn re_enroll_machine(
    State(state): State<Arc<ServerState>>,
    Path(machine_id): Path<Uuid>,
    Extension(admission): Extension<MutationAdmission>,
) -> ApiResult<StatusCode> {
    let lifecycle: Option<String> =
        sqlx::query_scalar("SELECT lifecycle FROM machines WHERE id = $1")
            .bind(machine_id)
            .fetch_optional(state.database.ordinary())
            .await
            .map_err(map_read)?;
    match lifecycle.as_deref() {
        Some("active") => {
            transition_active_machine(&state, machine_id, "pending", "re_enroll", admission).await
        }
        Some("disabled") => re_enroll_disabled_machine(&state, machine_id).await,
        Some(_) => Err(ApiError::conflict(
            "invalid_lifecycle",
            "Machine cannot be re-enrolled from its current lifecycle.",
        )),
        None => Err(ApiError::not_found()),
    }
}

async fn disable_machine(
    State(state): State<Arc<ServerState>>,
    Path(machine_id): Path<Uuid>,
    Extension(admission): Extension<MutationAdmission>,
) -> ApiResult<StatusCode> {
    transition_active_machine(&state, machine_id, "disabled", "disable", admission).await
}

async fn revoke_machine_relay(
    State(state): State<Arc<ServerState>>,
    Path(machine_id): Path<Uuid>,
    Extension(admission): Extension<MutationAdmission>,
) -> ApiResult<StatusCode> {
    transition_active_machine(&state, machine_id, "disabled", "revoke_relay", admission).await
}

async fn re_enroll_disabled_machine(
    state: &Arc<ServerState>,
    machine_id: Uuid,
) -> ApiResult<StatusCode> {
    let mut transaction = state
        .database
        .ordinary()
        .begin()
        .await
        .map_err(|error| map_write(&error))?;
    lock_current_deployment(&mut transaction, state).await?;
    let changed = sqlx::query(
        "UPDATE machines SET lifecycle = 'pending', updated_at = clock_timestamp() WHERE id = $1 AND lifecycle = 'disabled'",
    )
    .bind(machine_id)
    .execute(&mut *transaction)
    .await
    .map_err(|error| map_write(&error))?
    .rows_affected();
    if changed != 1 {
        return Err(ApiError::conflict(
            "invalid_lifecycle",
            "Machine is not disabled.",
        ));
    }
    append_audit(
        &mut transaction,
        state.database.deployment_id(),
        "machine",
        Some(machine_id),
        None,
        "re_enroll",
    )
    .await
    .map_err(map_storage)?;
    transaction
        .commit()
        .await
        .map_err(|_| ApiError::ambiguous())?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Clone, Copy)]
enum OwnerExpectation {
    Exact {
        incarnation_id: Uuid,
        route: crate::relay::RouteIdentity,
    },
    NoValidOwner {
        route_revision: i64,
    },
}

impl OwnerExpectation {
    const fn route_revision(self) -> i64 {
        match self {
            Self::Exact { route, .. } => route.route_revision,
            Self::NoValidOwner { route_revision } => route_revision,
        }
    }
}

enum TransitionCompletion {
    Committed,
    Aborted(ApiError),
    Indeterminate(ApiError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransitionObservation {
    Committed,
    Aborted,
    Indeterminate,
}

async fn transition_active_machine(
    state: &Arc<ServerState>,
    machine_id: Uuid,
    lifecycle: &'static str,
    action: &'static str,
    admission: MutationAdmission,
) -> ApiResult<StatusCode> {
    let protected_state = Arc::clone(state);
    tokio::spawn(async move {
        let _admission = admission;
        transition_active_machine_protected(&protected_state, machine_id, lifecycle, action).await
    })
    .await
    .map_err(|_| ApiError::temporarily_unavailable())?
}

#[allow(clippy::too_many_lines)]
async fn transition_active_machine_protected(
    state: &Arc<ServerState>,
    machine_id: Uuid,
    lifecycle: &'static str,
    action: &'static str,
) -> ApiResult<StatusCode> {
    match crate::owner::resolve(state, machine_id)
        .await
        .map_err(|_| ApiError::temporarily_unavailable())?
    {
        crate::owner::OwnerRoute::Local { route } => {
            let Some(transition) = state
                .relays
                .begin_machine_transition(machine_id, route)
                .await
            else {
                return Err(ApiError::temporarily_unavailable());
            };
            let expected = OwnerExpectation::Exact {
                incarnation_id: state.lease.incarnation_id(),
                route,
            };
            let completion = settle_transition_commit(
                state,
                machine_id,
                lifecycle,
                action,
                expected,
                commit_active_machine_transition(state, machine_id, lifecycle, action, expected)
                    .await,
            )
            .await;
            match completion {
                TransitionCompletion::Committed => {
                    if let Err(error) = transition.finish(state, true).await {
                        tracing::error!(%machine_id, action, ?error, "local Machine transition completion failed");
                    }
                    Ok(StatusCode::NO_CONTENT)
                }
                TransitionCompletion::Aborted(error) => {
                    if let Err(finish_error) = transition.finish(state, false).await {
                        tracing::error!(%machine_id, action, ?finish_error, "local Machine transition release failed");
                    }
                    Err(error)
                }
                TransitionCompletion::Indeterminate(error) => {
                    tracing::error!(%machine_id, action, "Machine transition remains indeterminate; hard-fencing node");
                    transition.hard_fence();
                    Err(error)
                }
            }
        }
        crate::owner::OwnerRoute::Remote {
            route,
            incarnation_id,
            internal_wss_url,
        } => {
            let client = state
                .internal
                .as_ref()
                .ok_or_else(ApiError::temporarily_unavailable)?;
            let transition = client
                .prepare_invalidation(state, machine_id, route, incarnation_id, &internal_wss_url)
                .await
                .map_err(|_| ApiError::owner_unreachable())?;
            let expected = OwnerExpectation::Exact {
                incarnation_id,
                route,
            };
            let completion = settle_transition_commit(
                state,
                machine_id,
                lifecycle,
                action,
                expected,
                commit_active_machine_transition(state, machine_id, lifecycle, action, expected)
                    .await,
            )
            .await;
            match completion {
                TransitionCompletion::Committed => {
                    if let Err(error) = transition.finish(true).await {
                        tracing::error!(%machine_id, action, ?error, "remote Machine transition completion failed");
                    }
                    Ok(StatusCode::NO_CONTENT)
                }
                TransitionCompletion::Aborted(error) => {
                    if let Err(finish_error) = transition.finish(false).await {
                        tracing::error!(%machine_id, action, ?finish_error, "remote Machine transition release failed");
                    }
                    Err(error)
                }
                TransitionCompletion::Indeterminate(error) => {
                    tracing::error!(%machine_id, action, "remote Machine transition remains indeterminate; destination will hard-fence");
                    drop(transition);
                    Err(error)
                }
            }
        }
        crate::owner::OwnerRoute::NoValidOwner { route_revision } => {
            let expected = OwnerExpectation::NoValidOwner { route_revision };
            match settle_transition_commit(
                state,
                machine_id,
                lifecycle,
                action,
                expected,
                commit_active_machine_transition(state, machine_id, lifecycle, action, expected)
                    .await,
            )
            .await
            {
                TransitionCompletion::Committed => Ok(StatusCode::NO_CONTENT),
                TransitionCompletion::Aborted(error)
                | TransitionCompletion::Indeterminate(error) => Err(error),
            }
        }
    }
}

async fn settle_transition_commit(
    state: &ServerState,
    machine_id: Uuid,
    lifecycle: &'static str,
    action: &'static str,
    expected: OwnerExpectation,
    result: ApiResult<StatusCode>,
) -> TransitionCompletion {
    match result {
        Ok(_) => TransitionCompletion::Committed,
        Err(error) if !error.is_ambiguous() => TransitionCompletion::Aborted(error),
        Err(error) => {
            let observation =
                observe_machine_transition(state, machine_id, lifecycle, expected).await;
            tracing::warn!(%machine_id, action, ?observation, "observed ambiguous Machine transition commit");
            match observation {
                TransitionObservation::Committed => TransitionCompletion::Committed,
                TransitionObservation::Aborted => {
                    TransitionCompletion::Aborted(ApiError::temporarily_unavailable())
                }
                TransitionObservation::Indeterminate => TransitionCompletion::Indeterminate(error),
            }
        }
    }
}

async fn observe_machine_transition(
    state: &ServerState,
    machine_id: Uuid,
    lifecycle: &'static str,
    expected: OwnerExpectation,
) -> TransitionObservation {
    if state.lease.check().is_err() {
        return TransitionObservation::Indeterminate;
    }
    let route_revision = expected.route_revision();
    let Some(next_revision) = route_revision.checked_add(1) else {
        return TransitionObservation::Indeterminate;
    };
    let query = sqlx::query(
        "SELECT m.lifecycle, m.route_revision, o.route_revision AS owner_route_revision, o.connection_epoch, o.owner_incarnation_id, o.relay_connection_id, b.status AS binding_status, EXISTS(SELECT 1 FROM server_nodes n WHERE n.incarnation_id = o.owner_incarnation_id AND n.state IN ('serving', 'draining') AND n.config_epoch = $3 AND n.server_build_id = $4 AND n.relay_protocol_version = 1 AND n.lease_until > clock_timestamp()) AS owner_valid FROM machines m JOIN machine_owners o ON o.machine_id = m.id LEFT JOIN relay_bindings b ON b.machine_id = m.id AND b.route_revision = $2 WHERE m.id = $1",
    )
    .bind(machine_id)
    .bind(route_revision)
    .bind(state.config.config_epoch())
    .bind(build::BUILD_ID)
    .fetch_optional(state.database.critical());
    let Ok(Ok(Some(row))) = tokio::time::timeout(Duration::from_secs(6), query).await else {
        return TransitionObservation::Indeterminate;
    };
    if state.lease.check().is_err() {
        return TransitionObservation::Indeterminate;
    }
    let values = (
        row.try_get::<String, _>("lifecycle"),
        row.try_get::<i64, _>("route_revision"),
        row.try_get::<i64, _>("owner_route_revision"),
        row.try_get::<i64, _>("connection_epoch"),
        row.try_get::<Option<Uuid>, _>("owner_incarnation_id"),
        row.try_get::<Option<Uuid>, _>("relay_connection_id"),
        row.try_get::<Option<String>, _>("binding_status"),
        row.try_get::<bool, _>("owner_valid"),
    );
    let (
        Ok(current_lifecycle),
        Ok(machine_revision),
        Ok(owner_revision),
        Ok(connection_epoch),
        Ok(owner_incarnation_id),
        Ok(connection_id),
        Ok(binding_status),
        Ok(owner_valid),
    ) = values
    else {
        return TransitionObservation::Indeterminate;
    };
    if current_lifecycle == lifecycle
        && machine_revision == next_revision
        && owner_revision == next_revision
        && owner_incarnation_id.is_none()
        && connection_id.is_none()
        && binding_status.as_deref() == Some("revoked")
    {
        return TransitionObservation::Committed;
    }
    let aborted = current_lifecycle == "active"
        && machine_revision == route_revision
        && owner_revision == route_revision
        && binding_status.as_deref() == Some("active")
        && match expected {
            OwnerExpectation::Exact {
                incarnation_id,
                route,
            } => {
                connection_epoch == route.connection_epoch
                    && owner_incarnation_id == Some(incarnation_id)
                    && connection_id == Some(route.connection_id)
            }
            OwnerExpectation::NoValidOwner { .. } => !owner_valid,
        };
    if aborted {
        TransitionObservation::Aborted
    } else {
        TransitionObservation::Indeterminate
    }
}

#[allow(clippy::too_many_lines)]
async fn commit_active_machine_transition(
    state: &Arc<ServerState>,
    machine_id: Uuid,
    lifecycle: &'static str,
    action: &'static str,
    expected: OwnerExpectation,
) -> ApiResult<StatusCode> {
    let mut transaction = state
        .database
        .ordinary()
        .begin()
        .await
        .map_err(|error| map_write(&error))?;
    lock_current_deployment(&mut transaction, state).await?;
    let route_revision: i64 = sqlx::query_scalar(
        "SELECT route_revision FROM machines WHERE id = $1 AND lifecycle = 'active' FOR UPDATE",
    )
    .bind(machine_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_read)?
    .ok_or_else(|| ApiError::conflict("invalid_lifecycle", "Machine is not active."))?;
    if route_revision != expected.route_revision() {
        return Err(ApiError::temporarily_unavailable());
    }
    let owner = sqlx::query(
        "SELECT o.route_revision, o.connection_epoch, o.owner_incarnation_id, o.relay_connection_id, EXISTS(SELECT 1 FROM server_nodes n WHERE n.incarnation_id = o.owner_incarnation_id AND n.state IN ('serving', 'draining') AND n.config_epoch = $2 AND n.server_build_id = $3 AND n.relay_protocol_version = 1 AND n.lease_until > clock_timestamp()) AS owner_valid FROM machine_owners o WHERE o.machine_id = $1 FOR UPDATE",
    )
    .bind(machine_id)
    .bind(state.config.config_epoch())
    .bind(build::BUILD_ID)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_read)?
    .ok_or_else(ApiError::internal)?;
    let owner_revision: i64 = owner
        .try_get("route_revision")
        .map_err(|_| ApiError::internal())?;
    let connection_epoch: i64 = owner
        .try_get("connection_epoch")
        .map_err(|_| ApiError::internal())?;
    let owner_incarnation_id: Option<Uuid> = owner
        .try_get("owner_incarnation_id")
        .map_err(|_| ApiError::internal())?;
    let connection_id: Option<Uuid> = owner
        .try_get("relay_connection_id")
        .map_err(|_| ApiError::internal())?;
    let owner_valid: bool = owner
        .try_get("owner_valid")
        .map_err(|_| ApiError::internal())?;
    let owner_matches = owner_revision == route_revision
        && match expected {
            OwnerExpectation::Exact {
                incarnation_id,
                route,
            } => {
                owner_incarnation_id == Some(incarnation_id)
                    && connection_id == Some(route.connection_id)
                    && connection_epoch == route.connection_epoch
            }
            OwnerExpectation::NoValidOwner { .. } => !owner_valid,
        };
    if !owner_matches {
        return Err(ApiError::temporarily_unavailable());
    }
    let next_revision = route_revision
        .checked_add(1)
        .ok_or_else(ApiError::internal)?;
    let revoked = sqlx::query(
        "UPDATE relay_bindings SET status = 'revoked', revoked_at = clock_timestamp() WHERE machine_id = $1 AND status = 'active' AND route_revision = $2",
    )
    .bind(machine_id)
    .bind(route_revision)
    .execute(&mut *transaction)
    .await
    .map_err(|error| map_write(&error))?
    .rows_affected();
    if revoked != 1 {
        return Err(ApiError::internal());
    }
    sqlx::query(
        "UPDATE relay_verification_attempts SET status = 'failed', completed_at = clock_timestamp() WHERE machine_id = $1 AND status = 'verifying'",
    )
    .bind(machine_id)
    .execute(&mut *transaction)
    .await
    .map_err(|error| map_write(&error))?;
    sqlx::query(
        "UPDATE relay_enrollments SET status = 'cancelled', cancelled_at = clock_timestamp() WHERE machine_id = $1 AND status = 'issued'",
    )
    .bind(machine_id)
    .execute(&mut *transaction)
    .await
    .map_err(|error| map_write(&error))?;
    let owner_changed = sqlx::query(
        "UPDATE machine_owners SET owner_incarnation_id = NULL, relay_connection_id = NULL, claimed_at = NULL, route_revision = $1 WHERE machine_id = $2 AND route_revision = $3",
    )
    .bind(next_revision)
    .bind(machine_id)
    .bind(route_revision)
    .execute(&mut *transaction)
    .await
    .map_err(|error| map_write(&error))?
    .rows_affected();
    let machine_changed = sqlx::query(
        "UPDATE machines SET lifecycle = $1, route_revision = $2, updated_at = clock_timestamp() WHERE id = $3 AND lifecycle = 'active' AND route_revision = $4",
    )
    .bind(lifecycle)
    .bind(next_revision)
    .bind(machine_id)
    .bind(route_revision)
    .execute(&mut *transaction)
    .await
    .map_err(|error| map_write(&error))?
    .rows_affected();
    if owner_changed != 1 || machine_changed != 1 {
        return Err(ApiError::internal());
    }
    append_audit(
        &mut transaction,
        state.database.deployment_id(),
        "machine",
        Some(machine_id),
        None,
        action,
    )
    .await
    .map_err(map_storage)?;
    transaction
        .commit()
        .await
        .map_err(|_| ApiError::ambiguous())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn lock_current_deployment(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: &ServerState,
) -> ApiResult<()> {
    let config_proof = state
        .config
        .configuration_proof(state.database.deployment_id())
        .map(Vec::from);
    let locked: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM deployment WHERE singleton = true AND id = $1 AND config_epoch = $2 AND server_build_id = $3 AND relay_protocol_version = 1 AND profile = $4 AND config_proof IS NOT DISTINCT FROM $5 FOR UPDATE",
    )
    .bind(state.database.deployment_id())
    .bind(state.config.config_epoch())
    .bind(build::BUILD_ID)
    .bind(state.config.profile_database_value())
    .bind(config_proof)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_read)?;
    if locked.is_none() || state.lease.check().is_err() {
        return Err(ApiError::temporarily_unavailable());
    }
    let node_valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM server_nodes WHERE incarnation_id = $1 AND state = 'serving' AND config_epoch = $2 AND server_build_id = $3 AND relay_protocol_version = 1 AND lease_until > clock_timestamp() FOR UPDATE)",
    )
    .bind(state.lease.incarnation_id())
    .bind(state.config.config_epoch())
    .bind(build::BUILD_ID)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_read)?;
    if !node_valid || state.lease.check().is_err() {
        return Err(ApiError::temporarily_unavailable());
    }
    Ok(())
}

async fn enforce_credential_limit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: &ServerState,
) -> ApiResult<()> {
    let credential_count: i64 =
        sqlx::query_scalar("SELECT count(*)::bigint FROM ssh_credentials WHERE deployment_id = $1")
            .bind(state.database.deployment_id())
            .fetch_one(&mut **transaction)
            .await
            .map_err(map_read)?;
    if credential_count >= 256 {
        return Err(ApiError::conflict(
            "credential_limit",
            "Deployment SSH credential limit reached.",
        ));
    }
    Ok(())
}

#[derive(Serialize)]
struct EnrollmentTokenResponse {
    enrollment_token: String,
    enrollment_expires_in: i64,
}

async fn issue_enrollment_token(
    State(state): State<Arc<ServerState>>,
    Path(machine_id): Path<Uuid>,
) -> ApiResult<Json<EnrollmentTokenResponse>> {
    relay::recover_expired_attempts(&state, Some(machine_id))
        .await
        .map_err(|_| ApiError::temporarily_unavailable())?;
    let (token, digest) = enrollment_token();
    let mut transaction = state
        .database
        .ordinary()
        .begin()
        .await
        .map_err(|error| map_write(&error))?;
    lock_current_deployment(&mut transaction, &state).await?;
    let lifecycle: Option<String> =
        sqlx::query_scalar("SELECT lifecycle FROM machines WHERE id = $1 FOR UPDATE")
            .bind(machine_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(map_read)?;
    if lifecycle.as_deref() != Some("pending") {
        return Err(ApiError::conflict(
            "invalid_lifecycle",
            "Machine is not pending.",
        ));
    }
    sqlx::query("UPDATE relay_enrollments SET status = 'cancelled', cancelled_at = clock_timestamp() WHERE machine_id = $1 AND status = 'issued'").bind(machine_id).execute(&mut *transaction).await.map_err(|error| map_write(&error))?;
    insert_enrollment(&mut transaction, machine_id, digest).await?;
    append_audit(
        &mut transaction,
        state.database.deployment_id(),
        "enrollment",
        Some(machine_id),
        None,
        "issue_token",
    )
    .await
    .map_err(map_storage)?;
    transaction
        .commit()
        .await
        .map_err(|_| ApiError::ambiguous())?;
    Ok(Json(EnrollmentTokenResponse {
        enrollment_token: token,
        enrollment_expires_in: TOKEN_TTL_SECONDS,
    }))
}

async fn cancel_enrollment_token(
    State(state): State<Arc<ServerState>>,
    Path(machine_id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let mut transaction = state
        .database
        .ordinary()
        .begin()
        .await
        .map_err(|error| map_write(&error))?;
    lock_current_deployment(&mut transaction, &state).await?;
    sqlx::query("SELECT id FROM machines WHERE id = $1 FOR UPDATE")
        .bind(machine_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_read)?
        .ok_or_else(ApiError::not_found)?;
    let changed = sqlx::query("UPDATE relay_enrollments SET status = 'cancelled', cancelled_at = clock_timestamp() WHERE machine_id = $1 AND status = 'issued'")
        .bind(machine_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| map_write(&error))?
        .rows_affected();
    if changed == 0 {
        return Err(ApiError::not_found());
    }
    append_audit(
        &mut transaction,
        state.database.deployment_id(),
        "enrollment",
        Some(machine_id),
        None,
        "cancel_token",
    )
    .await
    .map_err(map_storage)?;
    transaction
        .commit()
        .await
        .map_err(|_| ApiError::ambiguous())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn insert_enrollment(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    machine_id: Uuid,
    digest: [u8; 32],
) -> ApiResult<()> {
    sqlx::query("INSERT INTO relay_enrollments (id, machine_id, token_digest, token_expires_at, status) VALUES ($1, $2, $3, clock_timestamp() + $4 * interval '1 second', 'issued')")
        .bind(Uuid::new_v4()).bind(machine_id).bind(digest.as_slice()).bind(TOKEN_TTL_SECONDS)
        .execute(&mut **transaction).await.map_err(|error| map_write(&error))?;
    Ok(())
}

fn enrollment_token() -> (String, [u8; 32]) {
    let mut random = [0_u8; 32];
    rand_core::OsRng.fill_bytes(&mut random);
    let token = format!("{ENROLLMENT_PREFIX}{}", URL_SAFE_NO_PAD.encode(random));
    let mut hasher = Sha256::new();
    hasher.update(b"owlmux:relay-enrollment-token:v1\0");
    hasher.update(token.as_bytes());
    (token, hasher.finalize().into())
}

fn machine_from_row(row: &sqlx::postgres::PgRow, connected: bool) -> ApiResult<MachineSummary> {
    let lifecycle: String = row.try_get("lifecycle").map_err(|_| ApiError::internal())?;
    let reachability = if lifecycle == "active" {
        if connected {
            "reachable"
        } else {
            "temporarily_unavailable"
        }
    } else {
        "unknown"
    };
    Ok(MachineSummary {
        machine_id: row.try_get("id").map_err(|_| ApiError::internal())?,
        ssh_credential_id: row
            .try_get("ssh_credential_id")
            .map_err(|_| ApiError::internal())?,
        alias: row.try_get("alias").map_err(|_| ApiError::internal())?,
        lifecycle,
        reachability,
    })
}

fn validate_name(name: &str) -> ApiResult<()> {
    if name.is_empty()
        || name.len() > 64
        || name.trim() != name
        || name.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request(
            "invalid_name",
            "Name must be 1 to 64 safe characters.",
        ));
    }
    Ok(())
}
fn validate_machine(input: &CreateMachineInput) -> ApiResult<()> {
    validate_name(&input.alias)?;
    if !ssh::is_target_account(&input.target_account) {
        return Err(ApiError::bad_request(
            "invalid_target_account",
            "Target account is invalid.",
        ));
    }
    if !input.tmux_path.starts_with('/')
        || input.tmux_path.len() > 256
        || input.tmux_path.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request(
            "invalid_tmux_path",
            "tmux path must be an absolute safe path.",
        ));
    }
    if input.tmux_socket_identity.is_empty()
        || input.tmux_socket_identity.len() > 128
        || input.tmux_socket_identity.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request(
            "invalid_tmux_socket",
            "tmux socket identity is invalid.",
        ));
    }
    if input.host_identity.len() > 2048 || !ssh::is_ed25519_host_identity(&input.host_identity) {
        return Err(ApiError::bad_request(
            "invalid_host_identity",
            "Host identity is invalid.",
        ));
    }
    Ok(())
}

#[derive(Serialize)]
struct ErrorResponse {
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after: Option<u8>,
}
struct ApiError {
    status: StatusCode,
    body: ErrorResponse,
}
type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    fn is_ambiguous(&self) -> bool {
        self.body.code == "operation_ambiguous"
    }

    fn unauthenticated() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            body: ErrorResponse {
                code: "unauthenticated",
                message: "Authentication failed.",
                retry_after: None,
            },
        }
    }
    fn bad_request(code: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: ErrorResponse {
                code,
                message,
                retry_after: None,
            },
        }
    }
    fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            body: ErrorResponse {
                code: "not_found",
                message: "Resource not found.",
                retry_after: None,
            },
        }
    }
    fn conflict(code: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            body: ErrorResponse {
                code,
                message,
                retry_after: None,
            },
        }
    }
    fn temporarily_unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            body: ErrorResponse {
                code: "temporarily_unavailable",
                message: "Service is temporarily unavailable.",
                retry_after: Some(1),
            },
        }
    }
    fn owner_unreachable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            body: ErrorResponse {
                code: "owner_unreachable",
                message: "The Machine owner is unreachable.",
                retry_after: Some(1),
            },
        }
    }
    fn ambiguous() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            body: ErrorResponse {
                code: "operation_ambiguous",
                message: "Operation outcome is unknown; refresh before deciding what to do.",
                retry_after: None,
            },
        }
    }
    fn internal() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: ErrorResponse {
                code: "internal_error",
                message: "The operation failed.",
                retry_after: None,
            },
        }
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(self.body)).into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
        response
    }
}
fn map_read(_: sqlx::Error) -> ApiError {
    ApiError::temporarily_unavailable()
}
fn map_write(error: &sqlx::Error) -> ApiError {
    if error.as_database_error().is_some() {
        ApiError::conflict(
            "conflict",
            "The requested state conflicts with current state.",
        )
    } else {
        ApiError::temporarily_unavailable()
    }
}
fn map_storage(error: StorageError) -> ApiError {
    match error {
        StorageError::Conflict => ApiError::conflict(
            "conflict",
            "The requested state conflicts with current state.",
        ),
        StorageError::Ambiguous => ApiError::ambiguous(),
        _ => ApiError::temporarily_unavailable(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Semaphore;

    use super::MutationAdmission;

    #[test]
    fn protected_mutation_admission_outlives_request_copy() {
        let limit = Arc::new(Semaphore::new(1));
        let request_admission = MutationAdmission::new(
            limit
                .clone()
                .try_acquire_owned()
                .expect("test permit should be available"),
        );
        let protected_admission = request_admission.clone();

        drop(request_admission);
        assert_eq!(limit.available_permits(), 0);

        drop(protected_admission);
        assert_eq!(limit.available_permits(), 1);
    }
}
