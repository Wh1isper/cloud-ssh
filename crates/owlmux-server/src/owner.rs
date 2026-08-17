use sqlx::Row as _;
use uuid::Uuid;

use crate::{build, config::DeploymentProfile, relay::RouteIdentity, service::ServerState};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OwnerRoute {
    Local {
        route: RouteIdentity,
    },
    Remote {
        route: RouteIdentity,
        incarnation_id: Uuid,
        internal_wss_url: String,
    },
    NoValidOwner {
        route_revision: i64,
    },
}

/// Resolve one exact lease-valid Machine owner after external authentication.
pub(crate) async fn resolve(
    state: &ServerState,
    machine_id: Uuid,
) -> Result<OwnerRoute, OwnerError> {
    state.lease.check().map_err(|_| {
        state.observability.owner_resolution_failed();
        OwnerError::Fenced
    })?;
    let row = sqlx::query(
        "SELECT o.route_revision, o.connection_epoch, o.relay_connection_id, o.owner_incarnation_id, n.internal_wss_url FROM machines m JOIN machine_owners o ON o.machine_id = m.id JOIN relay_bindings b ON b.machine_id = m.id AND b.status = 'active' AND b.route_revision = m.route_revision JOIN server_nodes n ON n.incarnation_id = o.owner_incarnation_id WHERE m.id = $1 AND m.lifecycle = 'active' AND o.route_revision = m.route_revision AND o.connection_epoch > 0 AND n.state IN ('serving', 'draining') AND n.config_epoch = $2 AND n.server_build_id = $3 AND n.relay_protocol_version = 1 AND n.lease_until > clock_timestamp()",
    )
    .bind(machine_id)
    .bind(state.config.config_epoch())
    .bind(build::BUILD_ID)
    .fetch_optional(state.database.ordinary())
    .await
    .map_err(|_| {
        state.observability.owner_resolution_failed();
        OwnerError::Database
    })?;
    state.lease.check().map_err(|_| {
        state.observability.owner_resolution_failed();
        OwnerError::Fenced
    })?;

    let Some(row) = row else {
        let route_revision: Option<i64> = sqlx::query_scalar(
            "SELECT m.route_revision FROM machines m JOIN machine_owners o ON o.machine_id = m.id AND o.route_revision = m.route_revision JOIN relay_bindings b ON b.machine_id = m.id AND b.status = 'active' AND b.route_revision = m.route_revision WHERE m.id = $1 AND m.lifecycle = 'active' AND NOT EXISTS (SELECT 1 FROM server_nodes n WHERE n.incarnation_id = o.owner_incarnation_id AND n.state IN ('serving', 'draining') AND n.config_epoch = $2 AND n.server_build_id = $3 AND n.relay_protocol_version = 1 AND n.lease_until > clock_timestamp())",
        )
        .bind(machine_id)
        .bind(state.config.config_epoch())
        .bind(build::BUILD_ID)
        .fetch_optional(state.database.ordinary())
        .await
        .map_err(|_| OwnerError::Database)?;
        state.lease.check().map_err(|_| {
            state.observability.owner_resolution_failed();
            OwnerError::Fenced
        })?;
        return if let Some(route_revision) = route_revision {
            state.observability.owner_absent();
            Ok(OwnerRoute::NoValidOwner { route_revision })
        } else {
            state.observability.owner_resolution_failed();
            Err(OwnerError::Unavailable)
        };
    };

    let route = RouteIdentity {
        route_revision: row
            .try_get("route_revision")
            .map_err(|_| OwnerError::Invariant)?,
        connection_epoch: row
            .try_get("connection_epoch")
            .map_err(|_| OwnerError::Invariant)?,
        connection_id: row
            .try_get("relay_connection_id")
            .map_err(|_| OwnerError::Invariant)?,
    };
    let owner_incarnation_id: Uuid = row
        .try_get("owner_incarnation_id")
        .map_err(|_| OwnerError::Invariant)?;
    if owner_incarnation_id == state.lease.incarnation_id() {
        state.observability.owner_local();
        return Ok(OwnerRoute::Local { route });
    }
    if state.config.profile() != DeploymentProfile::Clustered {
        return Err(OwnerError::Invariant);
    }
    let internal_wss_url: Option<String> = row
        .try_get("internal_wss_url")
        .map_err(|_| OwnerError::Invariant)?;
    let internal_wss_url = internal_wss_url.ok_or(OwnerError::Invariant)?;
    state.observability.owner_remote();
    Ok(OwnerRoute::Remote {
        route,
        incarnation_id: owner_incarnation_id,
        internal_wss_url,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OwnerError {
    Unavailable,
    Database,
    Invariant,
    Fenced,
    Unreachable,
}

impl std::fmt::Display for OwnerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "Machine owner is unavailable",
            Self::Database => "Machine owner resolution failed",
            Self::Invariant => "Machine owner state is inconsistent",
            Self::Fenced => "Server incarnation is fenced",
            Self::Unreachable => "Machine owner is unreachable",
        })
    }
}

impl std::error::Error for OwnerError {}
