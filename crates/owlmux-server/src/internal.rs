use std::{sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::{SinkExt as _, StreamExt as _};
use rustls::{
    ClientConfig, RootCertStore, ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject as _},
};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
    time::timeout,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        client::IntoClientRequest as _,
        handshake::server::{ErrorResponse, Request, Response},
        protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
    },
};
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

use crate::{
    attachment::{AttachmentSocket, AttachmentWireError},
    build,
    clock::BootClock,
    cluster::{ConnectionClass, OwnerAuthContext, random_32},
    config::{ClusterConfig, Config},
    generated::contracts::{
        INTERNAL_AUTH_TIMEOUT_SECONDS, INTERNAL_DIAL_TIMEOUT_SECONDS, INTERNAL_MAX_CONNECTIONS,
        INTERNAL_MAX_CONTROL_CONNECTIONS, INTERNAL_MAX_FRAME_BYTES, INTERNAL_WRITE_TIMEOUT_SECONDS,
    },
    owner::OwnerError,
    relay::RouteIdentity,
    service::ServerState,
};

const INTERNAL_PATH: &str = "/internal/v1/owner";
const AUTH_TIMEOUT: Duration = Duration::from_secs(INTERNAL_AUTH_TIMEOUT_SECONDS);
const DIAL_TIMEOUT: Duration = Duration::from_secs(INTERNAL_DIAL_TIMEOUT_SECONDS);
const WRITE_TIMEOUT: Duration = Duration::from_secs(INTERNAL_WRITE_TIMEOUT_SECONDS);
const MAX_FRAME_BYTES: usize = INTERNAL_MAX_FRAME_BYTES;
const MAX_CONNECTIONS: usize = INTERNAL_MAX_CONNECTIONS;
const MAX_CONTROL_CONNECTIONS: usize = INTERNAL_MAX_CONTROL_CONNECTIONS;

pub struct InternalRuntime {
    listener: TcpListener,
    acceptor: TlsAcceptor,
    client: InternalClient,
}

#[derive(Clone)]
pub struct InternalClient {
    connector: TlsConnector,
}

pub(crate) struct RemoteTransition {
    socket: Option<ClientSocket>,
    _permit: OwnedSemaphorePermit,
}

#[derive(Clone)]
pub(crate) struct InternalLimits {
    attachments: Arc<Semaphore>,
    controls: Arc<Semaphore>,
}

impl InternalLimits {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            attachments: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
            controls: Arc::new(Semaphore::new(MAX_CONTROL_CONNECTIONS)),
        }
    }

    fn try_attachment(&self) -> Result<OwnedSemaphorePermit, tokio::sync::TryAcquireError> {
        self.attachments.clone().try_acquire_owned()
    }

    fn try_control(&self) -> Result<OwnedSemaphorePermit, tokio::sync::TryAcquireError> {
        self.controls.clone().try_acquire_owned()
    }
}

impl InternalRuntime {
    /// Bind and validate the clustered internal TLS endpoint before node registration.
    ///
    /// # Errors
    ///
    /// Returns a sanitized startup error for unreadable TLS material, invalid trust, or bind failure.
    pub async fn bind(config: &Config) -> Result<Option<Self>, InternalError> {
        let Some(cluster) = config.cluster() else {
            return Ok(None);
        };
        let server_config = load_server_config(cluster).await?;
        let client_config = load_client_config(cluster).await?;
        validate_local_identity(cluster, &server_config, &client_config).await?;
        let listener = TcpListener::bind(cluster.address())
            .await
            .map_err(|_| InternalError::Configuration)?;
        Ok(Some(Self {
            listener,
            acceptor: TlsAcceptor::from(Arc::new(server_config)),
            client: InternalClient {
                connector: TlsConnector::from(Arc::new(client_config)),
            },
        }))
    }

    #[must_use]
    pub fn client(&self) -> InternalClient {
        self.client.clone()
    }

    pub async fn serve(self, state: Arc<ServerState>, cancellation: CancellationToken) {
        let preauth_permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        let limits = InternalLimits::new();
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                () = cancellation.cancelled() => break,
                Some(result) = connections.join_next(), if !connections.is_empty() => {
                    if let Err(error) = result {
                        tracing::warn!(%error, "internal owner connection task failed");
                    }
                }
                accepted = self.listener.accept() => {
                    let Ok((stream, _peer)) = accepted else {
                        if !cancellation.is_cancelled() {
                            tracing::warn!("internal owner listener accept failed");
                        }
                        continue;
                    };
                    let Ok(preauth_permit) = Arc::clone(&preauth_permits).try_acquire_owned() else {
                        drop(stream);
                        continue;
                    };
                    let acceptor = self.acceptor.clone();
                    let connection_state = Arc::clone(&state);
                    let connection_limits = limits.clone();
                    connections.spawn(async move {
                        if let Err(error) = accept_owner_connection(
                            stream,
                            acceptor,
                            connection_state,
                            preauth_permit,
                            connection_limits,
                        ).await {
                            tracing::warn!(?error, "internal owner connection closed");
                        }
                    });
                }
            }
        }
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    }
}

pub(crate) async fn proxy_attachment(
    mut public: axum::extract::ws::WebSocket,
    state: &Arc<ServerState>,
    machine_id: Uuid,
    route: RouteIdentity,
    destination_incarnation_id: Uuid,
    internal_wss_url: &str,
) -> Result<(), OwnerError> {
    let Some(client) = &state.internal else {
        return Err(OwnerError::Invariant);
    };
    let Ok(_permit) = state.internal_limits.try_attachment() else {
        let _ = send_public_error(
            &mut public,
            "overloaded",
            "Internal routing capacity is exhausted.",
        )
        .await;
        return Err(OwnerError::Unreachable);
    };
    let internal = client
        .connect_authenticated(
            state,
            machine_id,
            route,
            destination_incarnation_id,
            internal_wss_url,
            ConnectionClass::Attachment,
        )
        .await;
    let mut internal = match internal {
        Ok(socket) => socket,
        Err(error) => {
            let _ = send_public_error(
                &mut public,
                "owner_unreachable",
                "The Machine owner is unreachable.",
            )
            .await;
            return Err(error);
        }
    };
    let fence = state.lease.fence_token();
    loop {
        tokio::select! {
            () = fence.cancelled() => return Err(OwnerError::Fenced),
            message = public.next() => {
                let Some(message) = message else { return Ok(()); };
                let message = message.map_err(|_| OwnerError::Unreachable)?;
                state.lease.check().map_err(|_| OwnerError::Fenced)?;
                match message {
                    axum::extract::ws::Message::Text(text) => {
                        if text.len() > MAX_FRAME_BYTES {
                            return Err(OwnerError::Unreachable);
                        }
                        timeout(WRITE_TIMEOUT, internal.send(Message::Text(text.to_string().into())))
                            .await.map_err(|_| OwnerError::Unreachable)?
                            .map_err(|_| OwnerError::Unreachable)?;
                    }
                    axum::extract::ws::Message::Close(frame) => {
                        send_internal_close(&mut internal, frame).await?;
                        return Ok(());
                    }
                    axum::extract::ws::Message::Ping(value) => {
                        timeout(WRITE_TIMEOUT, internal.send(Message::Ping(value.to_vec().into())))
                            .await.map_err(|_| OwnerError::Unreachable)?
                            .map_err(|_| OwnerError::Unreachable)?;
                    }
                    axum::extract::ws::Message::Pong(_) => {}
                    axum::extract::ws::Message::Binary(_) => return Err(OwnerError::Unreachable),
                }
            }
            message = internal.next() => {
                let Some(message) = message else { return Ok(()); };
                let message = message.map_err(|_| OwnerError::Unreachable)?;
                state.lease.check().map_err(|_| OwnerError::Fenced)?;
                match message {
                    Message::Text(text) => {
                        if text.len() > MAX_FRAME_BYTES {
                            return Err(OwnerError::Unreachable);
                        }
                        timeout(WRITE_TIMEOUT, public.send(axum::extract::ws::Message::Text(text.to_string().into())))
                            .await.map_err(|_| OwnerError::Unreachable)?
                            .map_err(|_| OwnerError::Unreachable)?;
                    }
                    Message::Close(frame) => {
                        send_public_close(&mut public, frame).await?;
                        return Ok(());
                    }
                    Message::Ping(value) => {
                        timeout(WRITE_TIMEOUT, public.send(axum::extract::ws::Message::Ping(value.to_vec().into())))
                            .await.map_err(|_| OwnerError::Unreachable)?
                            .map_err(|_| OwnerError::Unreachable)?;
                    }
                    Message::Pong(_) | Message::Frame(_) => {}
                    Message::Binary(_) => return Err(OwnerError::Unreachable),
                }
            }
        }
    }
}

async fn send_internal_close(
    socket: &mut ClientSocket,
    frame: Option<axum::extract::ws::CloseFrame>,
) -> Result<(), OwnerError> {
    let frame = frame.map(|frame| CloseFrame {
        code: CloseCode::from(frame.code),
        reason: frame.reason.to_string().into(),
    });
    timeout(WRITE_TIMEOUT, socket.send(Message::Close(frame)))
        .await
        .map_err(|_| OwnerError::Unreachable)?
        .map_err(|_| OwnerError::Unreachable)
}

async fn send_public_close(
    socket: &mut axum::extract::ws::WebSocket,
    frame: Option<CloseFrame>,
) -> Result<(), OwnerError> {
    let frame = frame.map(|frame| axum::extract::ws::CloseFrame {
        code: frame.code.into(),
        reason: frame.reason.to_string().into(),
    });
    timeout(
        WRITE_TIMEOUT,
        socket.send(axum::extract::ws::Message::Close(frame)),
    )
    .await
    .map_err(|_| OwnerError::Unreachable)?
    .map_err(|_| OwnerError::Unreachable)
}

fn acquire_class(
    limits: &InternalLimits,
    connection_class: ConnectionClass,
) -> Result<OwnedSemaphorePermit, OwnerError> {
    match connection_class {
        ConnectionClass::Attachment => limits.try_attachment(),
        ConnectionClass::Control => limits.try_control(),
    }
    .map_err(|_| OwnerError::Unavailable)
}

impl RemoteTransition {
    pub(crate) async fn finish(mut self, committed: bool) -> Result<(), OwnerError> {
        let mut socket = self.socket.take().ok_or(OwnerError::Unreachable)?;
        send_json(
            &mut socket,
            &InvalidationFinish {
                message_type: "machine.invalidation.finish".to_owned(),
                committed,
            },
        )
        .await?;
        let result: InvalidationResult = receive_json(&mut socket, AUTH_TIMEOUT).await?;
        let expected = if committed { "completed" } else { "aborted" };
        if result.message_type != "machine.invalidation.result" || result.outcome != expected {
            return Err(OwnerError::Unreachable);
        }
        Ok(())
    }
}

impl InternalClient {
    pub(crate) async fn prepare_invalidation(
        &self,
        state: &ServerState,
        machine_id: Uuid,
        route: RouteIdentity,
        destination_incarnation_id: Uuid,
        internal_wss_url: &str,
    ) -> Result<RemoteTransition, OwnerError> {
        let permit = state
            .internal_limits
            .try_control()
            .map_err(|_| OwnerError::Unreachable)?;
        let mut socket = self
            .connect_authenticated(
                state,
                machine_id,
                route,
                destination_incarnation_id,
                internal_wss_url,
                ConnectionClass::Control,
            )
            .await?;
        send_json(
            &mut socket,
            &InvalidationPrepare {
                message_type: "machine.invalidation.prepare".to_owned(),
            },
        )
        .await?;
        let ready: InvalidationReady = receive_json(&mut socket, AUTH_TIMEOUT).await?;
        if ready.message_type != "machine.invalidation.ready" {
            return Err(OwnerError::Unreachable);
        }
        Ok(RemoteTransition {
            socket: Some(socket),
            _permit: permit,
        })
    }

    async fn connect_authenticated(
        &self,
        state: &ServerState,
        machine_id: Uuid,
        route: RouteIdentity,
        destination_incarnation_id: Uuid,
        internal_wss_url: &str,
        connection_class: ConnectionClass,
    ) -> Result<ClientSocket, OwnerError> {
        state.lease.check().map_err(|_| OwnerError::Fenced)?;
        let url = validate_internal_url(internal_wss_url)?;
        let host = url.host_str().ok_or(OwnerError::Invariant)?.to_owned();
        let port = url.port_or_known_default().ok_or(OwnerError::Invariant)?;
        let tcp = timeout(DIAL_TIMEOUT, TcpStream::connect((host.as_str(), port)))
            .await
            .map_err(|_| OwnerError::Unreachable)?
            .map_err(|_| OwnerError::Unreachable)?;
        let server_name = ServerName::try_from(host).map_err(|_| OwnerError::Invariant)?;
        let tls = timeout(DIAL_TIMEOUT, self.connector.connect(server_name, tcp))
            .await
            .map_err(|_| OwnerError::Unreachable)?
            .map_err(|_| OwnerError::Unreachable)?;
        let request = internal_wss_url
            .into_client_request()
            .map_err(|_| OwnerError::Invariant)?;
        let (mut socket, _) = timeout(
            DIAL_TIMEOUT,
            tokio_tungstenite::client_async_with_config(request, tls, Some(websocket_config())),
        )
        .await
        .map_err(|_| OwnerError::Unreachable)?
        .map_err(|_| OwnerError::Unreachable)?;
        let challenge: Challenge = receive_json(&mut socket, AUTH_TIMEOUT).await?;
        if challenge.protocol != "owner.v1"
            || challenge.destination_incarnation_id != destination_incarnation_id
        {
            return Err(OwnerError::Unreachable);
        }
        let challenge_bytes = decode_32(&challenge.nonce)?;
        let source_nonce = random_32();
        let trace_id = Uuid::new_v4();
        let context = OwnerAuthContext {
            deployment_id: state.database.deployment_id(),
            config_epoch: state.config.config_epoch(),
            source_incarnation_id: state.lease.incarnation_id(),
            destination_incarnation_id,
            machine_id,
            route_revision: route.route_revision,
            connection_epoch: route.connection_epoch,
            connection_class,
            challenge: challenge_bytes,
            source_nonce,
            trace_id,
        };
        let cluster = state.config.cluster().ok_or(OwnerError::Invariant)?;
        let response = cluster.key().owner_response(&context);
        let authenticate = Authenticate {
            message_type: "owner.authenticate".to_owned(),
            source_incarnation_id: state.lease.incarnation_id(),
            destination_incarnation_id,
            machine_id,
            route_revision: route.route_revision.to_string(),
            connection_epoch: route.connection_epoch.to_string(),
            connection_class: class_label(connection_class).to_owned(),
            source_nonce: URL_SAFE_NO_PAD.encode(source_nonce),
            trace_id,
            response: URL_SAFE_NO_PAD.encode(response),
        };
        send_json(&mut socket, &authenticate).await?;
        let accepted: Accepted = receive_json(&mut socket, AUTH_TIMEOUT).await?;
        if accepted.message_type != "owner.accepted" {
            return Err(OwnerError::Unreachable);
        }
        state.lease.check().map_err(|_| OwnerError::Fenced)?;
        Ok(socket)
    }
}

// Tungstenite fixes the callback error to an unboxed HTTP response.
#[allow(clippy::result_large_err)]
fn validate_upgrade(request: &Request, response: Response) -> Result<Response, ErrorResponse> {
    if request.uri().path() == INTERNAL_PATH && request.uri().query().is_none() {
        Ok(response)
    } else {
        let mut error = ErrorResponse::new(Some("not found".to_owned()));
        *error.status_mut() = axum::http::StatusCode::NOT_FOUND;
        Err(error)
    }
}

async fn accept_owner_connection(
    stream: TcpStream,
    acceptor: TlsAcceptor,
    state: Arc<ServerState>,
    preauth_permit: OwnedSemaphorePermit,
    limits: InternalLimits,
) -> Result<(), OwnerError> {
    state.lease.check().map_err(|_| OwnerError::Fenced)?;
    let tls = timeout(AUTH_TIMEOUT, acceptor.accept(stream))
        .await
        .map_err(|_| OwnerError::Unreachable)?
        .map_err(|_| OwnerError::Unreachable)?;
    let mut socket = timeout(
        AUTH_TIMEOUT,
        tokio_tungstenite::accept_hdr_async_with_config(
            tls,
            validate_upgrade,
            Some(websocket_config()),
        ),
    )
    .await
    .map_err(|_| OwnerError::Unreachable)?
    .map_err(|_| OwnerError::Unreachable)?;
    let auth_clock = BootClock::default();
    let auth_started = auth_clock.now().map_err(|_| OwnerError::Fenced)?;
    let challenge_bytes = random_32();
    let challenge = Challenge {
        message_type: "owner.challenge".to_owned(),
        protocol: "owner.v1".to_owned(),
        destination_incarnation_id: state.lease.incarnation_id(),
        nonce: URL_SAFE_NO_PAD.encode(challenge_bytes),
    };
    send_json(&mut socket, &challenge).await?;
    let authenticate: Authenticate = receive_json(&mut socket, AUTH_TIMEOUT).await?;
    let route_revision = parse_epoch(&authenticate.route_revision)?;
    let connection_epoch = parse_epoch(&authenticate.connection_epoch)?;
    let connection_class = parse_class(&authenticate.connection_class)?;
    let source_nonce = decode_32(&authenticate.source_nonce)?;
    let response = decode_32(&authenticate.response)?;
    let context = OwnerAuthContext {
        deployment_id: state.database.deployment_id(),
        config_epoch: state.config.config_epoch(),
        source_incarnation_id: authenticate.source_incarnation_id,
        destination_incarnation_id: authenticate.destination_incarnation_id,
        machine_id: authenticate.machine_id,
        route_revision,
        connection_epoch,
        connection_class,
        challenge: challenge_bytes,
        source_nonce,
        trace_id: authenticate.trace_id,
    };
    let cluster = state.config.cluster().ok_or(OwnerError::Invariant)?;
    if authenticate.message_type != "owner.authenticate"
        || authenticate.destination_incarnation_id != state.lease.incarnation_id()
        || !cluster.key().verify_owner_response(&context, &response)
    {
        return Err(OwnerError::Unreachable);
    }
    validate_context(&state, &context).await?;
    let auth_finished = auth_clock.now().map_err(|_| OwnerError::Fenced)?;
    if auth_finished
        .checked_sub(auth_started)
        .is_none_or(|elapsed| elapsed >= AUTH_TIMEOUT)
    {
        return Err(OwnerError::Unreachable);
    }
    let route = resolve_current_route(&state, &context).await?;
    let class_permit = acquire_class(&limits, connection_class)?;
    drop(preauth_permit);
    send_json(
        &mut socket,
        &Accepted {
            message_type: "owner.accepted".to_owned(),
        },
    )
    .await?;
    match connection_class {
        ConnectionClass::Attachment => {
            let _class_permit = class_permit;
            let mut adapter = InternalAttachmentSocket { socket };
            crate::attachment::run_owner_attachment(
                &mut adapter,
                &state,
                context.machine_id,
                route,
            )
            .await;
            Ok(())
        }
        ConnectionClass::Control => {
            let _class_permit = class_permit;
            handle_invalidation(socket, state, context.machine_id, route).await
        }
    }
}

async fn handle_invalidation<S>(
    mut socket: WebSocketStream<S>,
    state: Arc<ServerState>,
    machine_id: Uuid,
    route: RouteIdentity,
) -> Result<(), OwnerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let prepare: InvalidationPrepare = receive_json(&mut socket, AUTH_TIMEOUT).await?;
    if prepare.message_type != "machine.invalidation.prepare" {
        return Err(OwnerError::Unreachable);
    }
    let Some(transition) = state
        .relays
        .begin_machine_transition(machine_id, route)
        .await
    else {
        return Err(OwnerError::Unavailable);
    };
    if send_json(
        &mut socket,
        &InvalidationReady {
            message_type: "machine.invalidation.ready".to_owned(),
        },
    )
    .await
    .is_err()
    {
        transition.hard_fence();
        return Err(OwnerError::Unreachable);
    }
    let finish: InvalidationFinish = match receive_json(&mut socket, Duration::from_secs(15)).await
    {
        Ok(finish) => finish,
        Err(error) => {
            transition.hard_fence();
            return Err(error);
        }
    };
    if finish.message_type != "machine.invalidation.finish" {
        transition.hard_fence();
        return Err(OwnerError::Unreachable);
    }
    transition
        .finish(&state, finish.committed)
        .await
        .map_err(|_| OwnerError::Fenced)?;
    send_json(
        &mut socket,
        &InvalidationResult {
            message_type: "machine.invalidation.result".to_owned(),
            outcome: if finish.committed {
                "completed".to_owned()
            } else {
                "aborted".to_owned()
            },
        },
    )
    .await
}

async fn validate_context(
    state: &ServerState,
    context: &OwnerAuthContext,
) -> Result<(), OwnerError> {
    state.lease.check().map_err(|_| OwnerError::Fenced)?;
    let proof = state
        .config
        .configuration_proof(state.database.deployment_id())
        .map(Vec::from);
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM deployment d JOIN server_nodes source ON source.incarnation_id = $1 JOIN server_nodes destination ON destination.incarnation_id = $2 JOIN machines m ON m.id = $3 JOIN machine_owners o ON o.machine_id = m.id JOIN relay_bindings b ON b.machine_id = m.id AND b.status = 'active' WHERE d.singleton = true AND d.id = $4 AND d.config_epoch = $5 AND d.server_build_id = $6 AND d.relay_protocol_version = 1 AND d.profile = 'clustered' AND d.config_proof IS NOT DISTINCT FROM $7 AND source.state = 'serving' AND source.config_epoch = d.config_epoch AND source.server_build_id = d.server_build_id AND source.relay_protocol_version = 1 AND source.lease_until > clock_timestamp() AND destination.state = 'serving' AND destination.config_epoch = d.config_epoch AND destination.server_build_id = d.server_build_id AND destination.relay_protocol_version = 1 AND destination.lease_until > clock_timestamp() AND m.lifecycle = 'active' AND m.route_revision = $8 AND o.owner_incarnation_id = destination.incarnation_id AND o.route_revision = $8 AND o.connection_epoch = $9 AND b.route_revision = $8)",
    )
    .bind(context.source_incarnation_id)
    .bind(context.destination_incarnation_id)
    .bind(context.machine_id)
    .bind(state.database.deployment_id())
    .bind(state.config.config_epoch())
    .bind(build::BUILD_ID)
    .bind(proof)
    .bind(context.route_revision)
    .bind(context.connection_epoch)
    .fetch_one(state.database.critical())
    .await
    .map_err(|_| OwnerError::Database)?;
    if !valid || state.lease.check().is_err() {
        return Err(OwnerError::Unavailable);
    }
    Ok(())
}

async fn resolve_current_route(
    state: &ServerState,
    context: &OwnerAuthContext,
) -> Result<RouteIdentity, OwnerError> {
    let route = RouteIdentity {
        route_revision: context.route_revision,
        connection_epoch: context.connection_epoch,
        connection_id: resolve_connection_id(state, context).await?,
    };
    if !state
        .relays
        .is_current_route(context.machine_id, route)
        .await
    {
        return Err(OwnerError::Unavailable);
    }
    Ok(route)
}

async fn resolve_connection_id(
    state: &ServerState,
    context: &OwnerAuthContext,
) -> Result<Uuid, OwnerError> {
    sqlx::query_scalar(
        "SELECT relay_connection_id FROM machine_owners WHERE machine_id = $1 AND owner_incarnation_id = $2 AND route_revision = $3 AND connection_epoch = $4",
    )
    .bind(context.machine_id)
    .bind(context.destination_incarnation_id)
    .bind(context.route_revision)
    .bind(context.connection_epoch)
    .fetch_optional(state.database.critical())
    .await
    .map_err(|_| OwnerError::Database)?
    .ok_or(OwnerError::Unavailable)
}

type ClientTlsStream = tokio_rustls::client::TlsStream<TcpStream>;
type ClientSocket = WebSocketStream<ClientTlsStream>;

struct InternalAttachmentSocket<S> {
    socket: WebSocketStream<S>,
}

impl<S> AttachmentSocket for InternalAttachmentSocket<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    async fn receive_text(&mut self) -> Result<String, AttachmentWireError> {
        let message = self
            .socket
            .next()
            .await
            .ok_or(AttachmentWireError::Closed)?
            .map_err(|_| AttachmentWireError::Closed)?;
        match message {
            Message::Text(text) => Ok(text.to_string()),
            Message::Close(_) => Err(AttachmentWireError::Closed),
            _ => Err(AttachmentWireError::Protocol),
        }
    }

    async fn send_text(&mut self, value: String) -> Result<(), AttachmentWireError> {
        self.socket
            .send(Message::Text(value.into()))
            .await
            .map_err(|_| AttachmentWireError::Unavailable)
    }

    async fn close(&mut self, code: u16, reason: &'static str) {
        let _ = self
            .socket
            .send(Message::Close(Some(CloseFrame {
                code: CloseCode::from(code),
                reason: reason.into(),
            })))
            .await;
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Challenge {
    #[serde(rename = "type")]
    message_type: String,
    protocol: String,
    destination_incarnation_id: Uuid,
    #[serde(rename = "challenge")]
    nonce: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Authenticate {
    #[serde(rename = "type")]
    message_type: String,
    source_incarnation_id: Uuid,
    destination_incarnation_id: Uuid,
    machine_id: Uuid,
    route_revision: String,
    connection_epoch: String,
    connection_class: String,
    source_nonce: String,
    trace_id: Uuid,
    response: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvalidationPrepare {
    #[serde(rename = "type")]
    message_type: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvalidationReady {
    #[serde(rename = "type")]
    message_type: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvalidationFinish {
    #[serde(rename = "type")]
    message_type: String,
    committed: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvalidationResult {
    #[serde(rename = "type")]
    message_type: String,
    outcome: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Accepted {
    #[serde(rename = "type")]
    message_type: String,
}

async fn send_json<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    value: &impl Serialize,
) -> Result<(), OwnerError> {
    let encoded = serde_json::to_string(value).map_err(|_| OwnerError::Invariant)?;
    if encoded.len() > MAX_FRAME_BYTES {
        return Err(OwnerError::Invariant);
    }
    timeout(WRITE_TIMEOUT, socket.send(Message::Text(encoded.into())))
        .await
        .map_err(|_| OwnerError::Unreachable)?
        .map_err(|_| OwnerError::Unreachable)
}

async fn receive_json<T: for<'de> Deserialize<'de>, S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    deadline: Duration,
) -> Result<T, OwnerError> {
    let message = timeout(deadline, socket.next())
        .await
        .map_err(|_| OwnerError::Unreachable)?
        .ok_or(OwnerError::Unreachable)?
        .map_err(|_| OwnerError::Unreachable)?;
    let Message::Text(text) = message else {
        return Err(OwnerError::Unreachable);
    };
    if text.len() > MAX_FRAME_BYTES {
        return Err(OwnerError::Unreachable);
    }
    serde_json::from_str(&text).map_err(|_| OwnerError::Unreachable)
}

fn parse_epoch(value: &str) -> Result<i64, OwnerError> {
    let parsed = value.parse::<i64>().map_err(|_| OwnerError::Unreachable)?;
    if parsed <= 0 || value != parsed.to_string() {
        return Err(OwnerError::Unreachable);
    }
    Ok(parsed)
}

fn decode_32(value: &str) -> Result<[u8; 32], OwnerError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| OwnerError::Unreachable)?;
    if decoded.len() != 32 || URL_SAFE_NO_PAD.encode(&decoded) != value {
        return Err(OwnerError::Unreachable);
    }
    decoded.try_into().map_err(|_| OwnerError::Unreachable)
}

fn validate_internal_url(value: &str) -> Result<Url, OwnerError> {
    let url = Url::parse(value).map_err(|_| OwnerError::Invariant)?;
    if url.scheme() != "wss"
        || url.host_str().is_none()
        || url.path() != INTERNAL_PATH
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(OwnerError::Invariant);
    }
    Ok(url)
}

fn websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES))
        .accept_unmasked_frames(false)
}

async fn validate_local_identity(
    cluster: &ClusterConfig,
    server_config: &ServerConfig,
    client_config: &ClientConfig,
) -> Result<(), InternalError> {
    let url = Url::parse(cluster.advertised_url()).map_err(|_| InternalError::Configuration)?;
    let host = url.host_str().ok_or(InternalError::Configuration)?;
    let server_name =
        ServerName::try_from(host.to_owned()).map_err(|_| InternalError::Configuration)?;
    let acceptor = TlsAcceptor::from(Arc::new(server_config.clone()));
    let connector = TlsConnector::from(Arc::new(client_config.clone()));
    let (client_io, server_io) = tokio::io::duplex(65_536);
    let handshake = async move {
        let (accepted, connected) = tokio::join!(
            acceptor.accept(server_io),
            connector.connect(server_name, client_io)
        );
        accepted.map_err(|_| InternalError::Configuration)?;
        connected.map_err(|_| InternalError::Configuration)?;
        Ok(())
    };
    timeout(AUTH_TIMEOUT, handshake)
        .await
        .map_err(|_| InternalError::Configuration)?
}

async fn load_server_config(cluster: &ClusterConfig) -> Result<ServerConfig, InternalError> {
    let certificate = tokio::fs::read(cluster.tls_certificate())
        .await
        .map_err(|_| InternalError::Configuration)?;
    let private_key = tokio::fs::read(cluster.tls_private_key())
        .await
        .map_err(|_| InternalError::Configuration)?;
    let certificates = parse_certificates(&certificate)?;
    let key =
        PrivateKeyDer::from_pem_slice(&private_key).map_err(|_| InternalError::Configuration)?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| InternalError::Configuration)?
        .with_no_client_auth()
        .with_single_cert(certificates, key)
        .map_err(|_| InternalError::Configuration)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

async fn load_client_config(cluster: &ClusterConfig) -> Result<ClientConfig, InternalError> {
    let ca = tokio::fs::read(cluster.tls_ca())
        .await
        .map_err(|_| InternalError::Configuration)?;
    let mut roots = RootCertStore::empty();
    for certificate in parse_certificates(&ca)? {
        roots
            .add(certificate)
            .map_err(|_| InternalError::Configuration)?;
    }
    if roots.is_empty() {
        return Err(InternalError::Configuration);
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| InternalError::Configuration)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

fn parse_certificates(value: &[u8]) -> Result<Vec<CertificateDer<'static>>, InternalError> {
    let certificates = CertificateDer::pem_slice_iter(value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| InternalError::Configuration)?;
    if certificates.is_empty() {
        return Err(InternalError::Configuration);
    }
    Ok(certificates)
}

async fn send_public_error(
    socket: &mut axum::extract::ws::WebSocket,
    code: &'static str,
    message: &'static str,
) -> Result<(), OwnerError> {
    for value in [
        serde_json::json!({ "type": "workspace.phase", "phase": "failed" }),
        serde_json::json!({ "type": "workspace.error", "code": code, "message": message }),
    ] {
        let encoded = serde_json::to_string(&value).map_err(|_| OwnerError::Invariant)?;
        timeout(
            WRITE_TIMEOUT,
            socket.send(axum::extract::ws::Message::Text(encoded.into())),
        )
        .await
        .map_err(|_| OwnerError::Unreachable)?
        .map_err(|_| OwnerError::Unreachable)?;
    }
    Ok(())
}

const fn class_label(value: ConnectionClass) -> &'static str {
    match value {
        ConnectionClass::Attachment => "attachment",
        ConnectionClass::Control => "control",
    }
}

fn parse_class(value: &str) -> Result<ConnectionClass, OwnerError> {
    match value {
        "attachment" => Ok(ConnectionClass::Attachment),
        "control" => Ok(ConnectionClass::Control),
        _ => Err(OwnerError::Unreachable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_capacity_remains_available_when_attachments_are_full() {
        let limits = InternalLimits::new();
        let attachments = (0..MAX_CONNECTIONS)
            .map(|_| limits.try_attachment().expect("attachment capacity"))
            .collect::<Vec<_>>();
        assert!(limits.try_attachment().is_err());
        let control = limits.try_control().expect("reserved control capacity");
        drop((attachments, control));
    }
}

#[derive(Clone, Copy, Debug)]
pub enum InternalError {
    Configuration,
}

impl std::fmt::Display for InternalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("internal TLS/WSS configuration failed")
    }
}

impl std::error::Error for InternalError {}
