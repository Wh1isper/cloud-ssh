use std::{
    collections::HashSet, future::Future, net::SocketAddr, pin::Pin, sync::Arc, time::Duration,
};

use axum::{
    Router,
    extract::{
        ConnectInfo, Path, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse as _, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt as _;
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use tokio::{
    sync::{OwnedMutexGuard, OwnedSemaphorePermit, mpsc, oneshot},
    time::{Instant, sleep, timeout},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zeroize::{Zeroize as _, Zeroizing};

use crate::{
    generated::contracts::{
        ATTACHMENT_CLOSE_NORMAL, ATTACHMENT_CLOSE_POLICY_VIOLATION,
        ATTACHMENT_CLOSE_PROTOCOL_ERROR, ATTACHMENT_CLOSE_TEMPORARILY_UNAVAILABLE,
        ATTACHMENT_MAX_DIMENSION, ATTACHMENT_MAX_FRAME_BYTES, ATTACHMENT_MAX_GRID_CELLS,
        ATTACHMENT_MAX_INPUT_BYTES, ATTACHMENT_MAX_TERMINAL_CHUNK_BYTES,
    },
    service::{ServerState, SourcePermit},
    ssh,
    storage::record_audit,
    tmux,
    writer::{ClientSize, ControlDirective, ControlOutcome, WriterScope, WriterView},
};

const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
const LOCAL_ROUTE_WAIT: Duration = Duration::from_secs(5);
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const WRITER_TRANSITION_TIMEOUT: Duration = Duration::from_secs(5);
const PROJECTION_INSTALL_TIMEOUT: Duration = Duration::from_secs(30);
type PendingDispatch = Pin<Box<dyn Future<Output = OwnedMutexGuard<()>> + Send>>;

pub(crate) trait AttachmentSocket {
    async fn receive_text(&mut self) -> Result<String, AttachmentWireError>;
    async fn send_text(&mut self, value: String) -> Result<(), AttachmentWireError>;
    async fn close(&mut self, code: u16, reason: &'static str);
    async fn audit_operation(&mut self, _operation: Operation, _outcome: PublicOutcome) {}
}

#[derive(Clone, Copy)]
pub(crate) enum AttachmentWireError {
    Closed,
    Protocol,
    Unavailable,
}

struct RouteAttachmentSocket<'a, S> {
    inner: &'a mut S,
    fence: crate::relay::RouteFence,
    state: &'a ServerState,
    machine_id: Uuid,
}

impl<S: AttachmentSocket> AttachmentSocket for RouteAttachmentSocket<'_, S> {
    async fn receive_text(&mut self) -> Result<String, AttachmentWireError> {
        self.inner.receive_text().await
    }

    async fn send_text(&mut self, value: String) -> Result<(), AttachmentWireError> {
        self.fence
            .dispatch(self.inner.send_text(value))
            .await
            .map_err(|_| AttachmentWireError::Unavailable)?
    }

    async fn close(&mut self, code: u16, reason: &'static str) {
        self.inner.close(code, reason).await;
    }

    async fn audit_operation(&mut self, operation: Operation, outcome: PublicOutcome) {
        if let Some(action) = operation.audit_action() {
            audit_event(self.state, self.machine_id, action, outcome.audit_outcome()).await;
        }
    }
}

impl AttachmentSocket for WebSocket {
    async fn receive_text(&mut self) -> Result<String, AttachmentWireError> {
        let message = self
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
        self.send(Message::Text(value.into()))
            .await
            .map_err(|_| AttachmentWireError::Unavailable)
    }

    async fn close(&mut self, code: u16, reason: &'static str) {
        let _ = self
            .send(Message::Close(Some(CloseFrame {
                code,
                reason: reason.into(),
            })))
            .await;
    }
}

pub fn router(state: Arc<ServerState>) -> Router<Arc<ServerState>> {
    Router::new()
        .route("/machines/{machine_id}", get(upgrade))
        .with_state(state)
}

async fn upgrade(
    State(state): State<Arc<ServerState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(machine_id): Path<Uuid>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let valid_origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| origin == state.config.public_origin());
    if !valid_origin {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(source_permit) = state.source_admission.try_acquire(peer.ip()) else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let Ok(attempt_permit) = state.preauth_attempt_limit.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    upgrade
        .max_message_size(ATTACHMENT_MAX_FRAME_BYTES)
        .max_frame_size(ATTACHMENT_MAX_FRAME_BYTES)
        .on_upgrade(move |socket| async move {
            attachment(socket, state, machine_id, attempt_permit, source_permit).await;
        })
}

async fn attachment(
    socket: WebSocket,
    state: Arc<ServerState>,
    machine_id: Uuid,
    attempt_permit: OwnedSemaphorePermit,
    source_permit: SourcePermit,
) {
    let fence = state.lease.fence_token();
    tokio::select! {
        () = fence.cancelled() => {}
        () = attachment_session(
            socket,
            state,
            machine_id,
            attempt_permit,
            source_permit,
        ) => {}
    }
}

async fn attachment_session(
    mut socket: WebSocket,
    state: Arc<ServerState>,
    machine_id: Uuid,
    attempt_permit: OwnedSemaphorePermit,
    source_permit: SourcePermit,
) {
    let authenticated = authenticate(&mut socket, &state).await;
    drop(source_permit);
    drop(attempt_permit);
    if authenticated.is_err() {
        let _ = send_fatal_error(&mut socket, "unauthenticated", "Authentication failed.").await;
        return;
    }
    match crate::owner::resolve(&state, machine_id).await {
        Ok(crate::owner::OwnerRoute::Local { route }) => {
            audit_event(&state, machine_id, "attachment_route_local", "success").await;
            run_owner_attachment(&mut socket, &state, machine_id, route).await;
        }
        Ok(crate::owner::OwnerRoute::Remote {
            route,
            incarnation_id,
            internal_wss_url,
        }) => {
            audit_event(&state, machine_id, "attachment_route_remote", "success").await;
            if crate::internal::proxy_attachment(
                socket,
                &state,
                machine_id,
                route,
                incarnation_id,
                &internal_wss_url,
            )
            .await
            .is_err()
            {
                tracing::warn!(%machine_id, "remote Machine owner is unreachable");
            }
        }
        Ok(crate::owner::OwnerRoute::NoValidOwner { .. }) => {
            audit_event(&state, machine_id, "attachment_route", "rejected").await;
            let _ = send_fatal_error(
                &mut socket,
                "temporarily_unavailable",
                "The Machine is temporarily unavailable.",
            )
            .await;
        }
        Err(error) => {
            tracing::warn!(?error, %machine_id, "Machine owner resolution failed");
            let _ = send_fatal_error(
                &mut socket,
                "temporarily_unavailable",
                "The Machine is temporarily unavailable.",
            )
            .await;
        }
    }
}

pub(crate) async fn run_owner_attachment(
    socket: &mut impl AttachmentSocket,
    state: &Arc<ServerState>,
    machine_id: Uuid,
    expected_route: crate::relay::RouteIdentity,
) {
    let Some(route_fence) = state.relays.route_fence(machine_id, expected_route).await else {
        let _ = send_fatal_error(
            socket,
            "temporarily_unavailable",
            "The Machine route changed.",
        )
        .await;
        return;
    };
    let Ok(_connection_permit) = state
        .attachment_connection_limit
        .clone()
        .try_acquire_owned()
    else {
        let _ = send_fatal_error(socket, "overloaded", "Attachment capacity is exhausted.").await;
        return;
    };
    let attachment_id = Uuid::new_v4();
    audit_event(state, machine_id, "attachment_start", "success").await;
    let mut routed_socket = RouteAttachmentSocket {
        inner: socket,
        fence: route_fence,
        state,
        machine_id,
    };
    authenticated_session(
        &mut routed_socket,
        state,
        machine_id,
        attachment_id,
        expected_route,
    )
    .await;
    state.writers.release_attachment(attachment_id).await;
    audit_event(state, machine_id, "attachment_end", "success").await;
    let _ = timeout(
        CLIENT_WRITE_TIMEOUT,
        routed_socket.close(ATTACHMENT_CLOSE_NORMAL, "attachment_closed"),
    )
    .await;
}

async fn authenticated_session(
    socket: &mut impl AttachmentSocket,
    state: &Arc<ServerState>,
    machine_id: Uuid,
    attachment_id: Uuid,
    expected_route: crate::relay::RouteIdentity,
) {
    loop {
        let selection =
            match chooser(socket, state, machine_id, attachment_id, expected_route).await {
                Ok(ChooserExit::Selection(selection)) => selection,
                Ok(ChooserExit::Refresh) => continue,
                Ok(ChooserExit::Detach)
                | Err(AttachmentError::ClientClosed | AttachmentError::RouteChanged) => return,
                Err(error) => {
                    tracing::warn!(?error, %machine_id, "attachment chooser failed");
                    let _ = send_fatal_error(socket, error.code(), error.message()).await;
                    return;
                }
            };
        let scope = Arc::clone(&selection.scope);
        let result = workspace(socket, state, machine_id, attachment_id, selection).await;
        let _dispatch = scope.dispatch().await;
        scope.lock().await.clear_control(attachment_id);
        match result {
            Ok(WorkspaceExit::Chooser) => {}
            Ok(WorkspaceExit::Detach)
            | Err(AttachmentError::ClientClosed | AttachmentError::RouteChanged) => return,
            Err(error) => {
                tracing::warn!(?error, %machine_id, "attachment workspace failed");
                let _ = send_fatal_error(socket, error.code(), error.message()).await;
                return;
            }
        }
    }
}

async fn authenticate(socket: &mut WebSocket, state: &ServerState) -> Result<(), ()> {
    let AuthFrame::ApiKey { mut api_key } = receive_auth(socket).await.map_err(|_| ())?;
    let valid_length = (56..=64).contains(&api_key.as_str().len());
    let valid = valid_length && state.config.api_key().verify(api_key.as_str());
    api_key.zeroize();
    if !valid || state.lease.check().is_err() {
        return Err(());
    }
    Ok(())
}

#[derive(Clone)]
struct Selection {
    session_id: String,
    session_created: i64,
    route: crate::relay::RouteIdentity,
    route_closed: CancellationToken,
    scope: Arc<WriterScope>,
}

#[derive(Clone)]
enum ChooserExit {
    Selection(Selection),
    Refresh,
    Detach,
}

#[allow(clippy::too_many_lines)]
async fn chooser(
    socket: &mut impl AttachmentSocket,
    state: &Arc<ServerState>,
    machine_id: Uuid,
    attachment_id: Uuid,
    expected_route: crate::relay::RouteIdentity,
) -> Result<ChooserExit, AttachmentError> {
    send(
        socket,
        &ServerFrame::Phase {
            phase: Phase::Connecting,
        },
    )
    .await?;
    let relay_stream = open_relay_stream(state, machine_id).await?;
    let route = relay_stream.route;
    if route != expected_route {
        return Err(AttachmentError::RouteChanged);
    }
    let route_closed = relay_stream.closed;
    let probe = tokio::select! {
        () = route_closed.cancelled() => return Err(AttachmentError::RouteChanged),
        probe = ssh::run_tmux_probe(state, machine_id, relay_stream.stream) => {
            audit_event(
                state,
                machine_id,
                "ssh_tmux_probe",
                if probe.is_ok() { "success" } else { "rejected" },
            )
            .await;
            probe.map_err(|_| AttachmentError::Ssh)?
        }
    };
    let probe = tmux::parse_probe(&probe).map_err(map_tmux)?;
    let scope = state.writers.scope(machine_id, route).await;
    let mut writer_changed = scope.subscribe();
    send(
        socket,
        &ServerFrame::Phase {
            phase: Phase::Selecting,
        },
    )
    .await?;
    send(
        socket,
        &ServerFrame::SessionList {
            machine_connection_epoch: route.connection_epoch.to_string(),
            selection_epoch: probe.selection_epoch,
            tmux_client_version: probe.tmux_client_version,
            tmux_server_version: probe.tmux_server_version,
            sessions: probe.sessions.clone(),
        },
    )
    .await?;
    send_writer_state(socket, &scope, attachment_id, probe.selection_epoch).await?;
    loop {
        let frame = tokio::select! {
            () = route_closed.cancelled() => return Err(AttachmentError::RouteChanged),
            changed = writer_changed.changed() => {
                changed.map_err(|_| AttachmentError::Unavailable)?;
                send_writer_state(socket, &scope, attachment_id, probe.selection_epoch).await?;
                continue;
            }
            frame = receive(socket) => frame?,
        };
        match frame {
            ClientFrame::SessionSelect {
                machine_connection_epoch,
                selection_epoch,
                session_id,
                session_created,
            } => {
                if !matches_connection_epoch(&machine_connection_epoch, route.connection_epoch)
                    || selection_epoch != probe.selection_epoch
                    || !probe.sessions.iter().any(|session| {
                        session.session_id == session_id
                            && session.session_created == session_created
                    })
                {
                    send_workspace_error(socket, "stale_selection", "Session selection is stale.")
                        .await?;
                    continue;
                }
                return Ok(ChooserExit::Selection(Selection {
                    session_id,
                    session_created,
                    route,
                    route_closed: route_closed.clone(),
                    scope,
                }));
            }
            ClientFrame::WriterClaim {
                request_id,
                machine_connection_epoch,
                attachment_epoch,
                columns,
                rows,
            } => {
                let request = WriterRequest {
                    request_id,
                    operation: Operation::WriterClaim,
                    machine_connection_epoch,
                    attachment_epoch,
                    size: ClientSize { columns, rows },
                };
                change_writer_without_control(
                    socket,
                    state,
                    machine_id,
                    attachment_id,
                    &scope,
                    probe.selection_epoch,
                    request,
                    false,
                )
                .await?;
            }
            ClientFrame::WriterTakeover {
                request_id,
                machine_connection_epoch,
                attachment_epoch,
                columns,
                rows,
            } => {
                let request = WriterRequest {
                    request_id,
                    operation: Operation::WriterTakeover,
                    machine_connection_epoch,
                    attachment_epoch,
                    size: ClientSize { columns, rows },
                };
                change_writer_without_control(
                    socket,
                    state,
                    machine_id,
                    attachment_id,
                    &scope,
                    probe.selection_epoch,
                    request,
                    true,
                )
                .await?;
            }
            ClientFrame::SessionRefresh {
                request_id,
                machine_connection_epoch,
                selection_epoch,
            } => {
                if !matches_connection_epoch(&machine_connection_epoch, route.connection_epoch)
                    || selection_epoch != probe.selection_epoch
                {
                    send_stale_operation(socket, request_id, Operation::SessionRefresh).await?;
                    continue;
                }
                send_operation_result(
                    socket,
                    request_id,
                    Operation::SessionRefresh,
                    PublicOutcome::Succeeded,
                    "chooser_refreshed",
                    "Refreshing the target session chooser.",
                )
                .await?;
                return Ok(ChooserExit::Refresh);
            }
            ClientFrame::SessionCreate {
                request_id,
                machine_connection_epoch,
                selection_epoch,
                name,
            } => {
                if !matches_connection_epoch(&machine_connection_epoch, route.connection_epoch)
                    || selection_epoch != probe.selection_epoch
                {
                    send_operation_result(
                        socket,
                        request_id,
                        Operation::SessionCreate,
                        PublicOutcome::Failed,
                        "stale_epoch",
                        "The session chooser changed before dispatch.",
                    )
                    .await?;
                    continue;
                }
                let _dispatch = scope.dispatch().await;
                let mut writer = scope.lock().await;
                if !writer.is_current(attachment_id) {
                    send_operation_result(
                        socket,
                        request_id,
                        Operation::SessionCreate,
                        PublicOutcome::Failed,
                        "writer_required",
                        "Claim writer access before creating a session.",
                    )
                    .await?;
                    continue;
                }
                if !valid_session_name(&name) || !current_route(state, machine_id, &scope).await {
                    send_operation_result(
                        socket,
                        request_id,
                        Operation::SessionCreate,
                        PublicOutcome::Failed,
                        "invalid_operation",
                        "The session could not be created before dispatch.",
                    )
                    .await?;
                    continue;
                }
                let stream = match state.relays.open_stream(machine_id).await {
                    Ok(stream) if stream.route == route => stream,
                    _ => {
                        send_operation_result(
                            socket,
                            request_id,
                            Operation::SessionCreate,
                            PublicOutcome::Failed,
                            "target_unavailable",
                            "The target became unavailable before dispatch.",
                        )
                        .await?;
                        continue;
                    }
                };
                let outcome = ssh::create_tmux_session(state, machine_id, &name, stream.stream)
                    .await
                    .map_err(|_| AttachmentError::Ssh)?;
                if !current_route(state, machine_id, &scope).await {
                    writer.clear_if_current(attachment_id);
                    return Err(AttachmentError::RouteChanged);
                }
                let (public, code, message) = match outcome {
                    ssh::CreateSessionOutcome::Succeeded => (
                        PublicOutcome::Succeeded,
                        "session_created",
                        "The target session was created. Refreshing the chooser.",
                    ),
                    ssh::CreateSessionOutcome::Failed => (
                        PublicOutcome::Failed,
                        "session_create_failed",
                        "The target rejected session creation. Refreshing the chooser.",
                    ),
                    ssh::CreateSessionOutcome::Ambiguous => {
                        writer.clear();
                        (
                            PublicOutcome::Ambiguous,
                            "operation_ambiguous",
                            "Session creation may have occurred. It will not be retried.",
                        )
                    }
                };
                send_operation_result(
                    socket,
                    request_id,
                    Operation::SessionCreate,
                    public,
                    code,
                    message,
                )
                .await?;
                return Ok(ChooserExit::Refresh);
            }
            ClientFrame::WorkspaceDetach => return Ok(ChooserExit::Detach),
            _ => return Err(AttachmentError::Protocol),
        }
    }
}

#[derive(Clone)]
struct WriterRequest {
    request_id: Uuid,
    operation: Operation,
    machine_connection_epoch: String,
    attachment_epoch: Uuid,
    size: ClientSize,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn change_writer_without_control(
    socket: &mut impl AttachmentSocket,
    state: &Arc<ServerState>,
    machine_id: Uuid,
    attachment_id: Uuid,
    scope: &Arc<WriterScope>,
    expected_epoch: Uuid,
    request: WriterRequest,
    takeover: bool,
) -> Result<(), AttachmentError> {
    if request.attachment_epoch != expected_epoch
        || !matches_connection_epoch(
            &request.machine_connection_epoch,
            scope.route().connection_epoch,
        )
    {
        return send_operation_result(
            socket,
            request.request_id,
            request.operation,
            PublicOutcome::Failed,
            "stale_epoch",
            "Writer state changed before dispatch.",
        )
        .await;
    }
    if !valid_client_size(request.size) {
        return send_operation_result(
            socket,
            request.request_id,
            request.operation,
            PublicOutcome::Failed,
            "invalid_operation",
            "The requested Browser size is outside the supported bounds.",
        )
        .await;
    }
    let _dispatch = scope.dispatch().await;
    let mut writer = scope.lock().await;
    if !current_route(state, machine_id, scope).await {
        return Err(AttachmentError::RouteChanged);
    }
    if writer.is_current(attachment_id) {
        writer.update_size(attachment_id, request.size);
        return send_operation_result(
            socket,
            request.request_id,
            request.operation,
            PublicOutcome::Succeeded,
            "writer_current",
            "This attachment is the current writer.",
        )
        .await;
    }
    if !writer.is_available() && !takeover {
        return send_operation_result(
            socket,
            request.request_id,
            request.operation,
            PublicOutcome::Failed,
            "writer_busy",
            "Another OwlMux Browser attachment is the current writer.",
        )
        .await;
    }
    let old_control = writer.current_control();
    if let Some(control) = old_control.as_ref() {
        let outcome = demote_writer(control).await;
        if !current_route(state, machine_id, scope).await {
            writer.clear();
            let _ = control.send(ControlDirective::Close).await;
            return Err(AttachmentError::RouteChanged);
        }
        if outcome != ControlOutcome::Succeeded {
            writer.clear();
            let _ = control.send(ControlDirective::Close).await;
            let public = if outcome == ControlOutcome::Ambiguous {
                PublicOutcome::Ambiguous
            } else {
                PublicOutcome::Failed
            };
            return send_operation_result(
                socket,
                request.request_id,
                request.operation,
                public,
                if public == PublicOutcome::Ambiguous {
                    "operation_ambiguous"
                } else {
                    "writer_transition_failed"
                },
                "Writer transition failed closed; no writer was retained.",
            )
            .await;
        }
    }
    writer.set_current(attachment_id, request.size, None);
    send_operation_result(
        socket,
        request.request_id,
        request.operation,
        PublicOutcome::Succeeded,
        if takeover {
            "writer_taken_over"
        } else {
            "writer_claimed"
        },
        "This attachment is now the current writer.",
    )
    .await
}

#[allow(clippy::too_many_lines)]
async fn workspace(
    socket: &mut impl AttachmentSocket,
    state: &Arc<ServerState>,
    machine_id: Uuid,
    attachment_id: Uuid,
    selection: Selection,
) -> Result<WorkspaceExit, AttachmentError> {
    let relay_stream = open_relay_stream(state, machine_id).await?;
    if relay_stream.route != selection.route || selection.route_closed.is_cancelled() {
        return Err(AttachmentError::RouteChanged);
    }
    let route_closed = relay_stream.closed;
    let control = tokio::select! {
        () = route_closed.cancelled() => return Err(AttachmentError::RouteChanged),
        control = ssh::start_tmux_control(
            state,
            machine_id,
            &selection.session_id,
            selection.session_created,
            relay_stream.stream,
        ) => {
            audit_event(
                state,
                machine_id,
                "ssh_tmux_control",
                if control.is_ok() { "success" } else { "rejected" },
            )
            .await;
            control.map_err(|_| AttachmentError::Ssh)?
        },
    };
    let mut control = tokio::select! {
        () = route_closed.cancelled() => return Err(AttachmentError::RouteChanged),
        control = tmux::ControlAdapter::start(control) => {
            control.map_err(map_tmux)?
        },
    };
    let (control_tx, mut control_rx) = mpsc::channel(4);
    let projection = {
        let _dispatch = selection.scope.dispatch().await;
        if !current_route(state, machine_id, &selection.scope).await {
            return Err(AttachmentError::RouteChanged);
        }
        let mut writer = selection.scope.lock().await;
        if writer.is_current(attachment_id) {
            let outcome = control.set_writer(true).await;
            if outcome != tmux::MutationOutcome::Succeeded {
                writer.clear_if_current(attachment_id);
                return Ok(WorkspaceExit::Chooser);
            }
            writer.set_control(attachment_id, control_tx.clone());
            if let Some(size) = writer.current_size(attachment_id) {
                let outcome = control
                    .resize(size.columns, size.rows)
                    .await
                    .map_err(map_tmux)?;
                if outcome != tmux::MutationOutcome::Succeeded {
                    writer.clear_if_current(attachment_id);
                    return Ok(WorkspaceExit::Chooser);
                }
            }
        }
        let projection = tokio::select! {
            () = route_closed.cancelled() => return Err(AttachmentError::RouteChanged),
            projection = install_projection(
                socket,
                &mut control,
                selection.route.connection_epoch,
                &selection.session_id,
                selection.session_created,
            ) => projection?,
        };
        if !current_route(state, machine_id, &selection.scope).await {
            writer.clear_if_current(attachment_id);
            return Err(AttachmentError::RouteChanged);
        }
        projection
    };
    let Some(mut workspace) = projection else {
        return Ok(WorkspaceExit::Chooser);
    };
    let mut writer_changed = selection.scope.subscribe();
    let mut projection_dirty = false;
    let mut pending_dispatch: Option<PendingDispatch> = None;
    send_writer_state(socket, &selection.scope, attachment_id, workspace.epoch).await?;
    loop {
        tokio::select! {
            () = route_closed.cancelled() => return Err(AttachmentError::RouteChanged),
            changed = writer_changed.changed() => {
                changed.map_err(|_| AttachmentError::Unavailable)?;
                send_writer_state(
                    socket,
                    &selection.scope,
                    attachment_id,
                    workspace.epoch,
                ).await?;
            }
            directive = control_rx.recv() => {
                let Some(directive) = directive else {
                    return Ok(WorkspaceExit::Chooser);
                };
                match directive {
                    ControlDirective::Demote { response } => {
                        let outcome = to_control_outcome(control.set_writer(false).await);
                        let _ = response.send(outcome);
                        if outcome != ControlOutcome::Succeeded {
                            return Ok(WorkspaceExit::Chooser);
                        }
                    }
                    ControlDirective::Close => return Ok(WorkspaceExit::Chooser),
                }
            }
            client = receive(socket) => {
                match handle_workspace_frame(
                    socket,
                    state,
                    machine_id,
                    attachment_id,
                    &selection,
                    &control_tx,
                    &mut control,
                    &mut workspace,
                    client?,
                ).await? {
                    WorkspaceFrameExit::Continue => {}
                    WorkspaceFrameExit::Chooser => return Ok(WorkspaceExit::Chooser),
                    WorkspaceFrameExit::Detach => return Ok(WorkspaceExit::Detach),
                }
            },
            _dispatch = async {
                pending_dispatch
                    .as_mut()
                    .expect("pending dispatch branch is guarded")
                    .await
            }, if pending_dispatch.is_some() => {
                pending_dispatch = None;
                if !projection_dirty {
                    continue;
                }
                projection_dirty = false;
                if !current_route(state, machine_id, &selection.scope).await {
                    return Err(AttachmentError::RouteChanged);
                }
                let projection = install_projection(
                    socket,
                    &mut control,
                    selection.route.connection_epoch,
                    &selection.session_id,
                    selection.session_created,
                ).await?;
                let Some(next) = projection else {
                    return Ok(WorkspaceExit::Chooser);
                };
                if !current_route(state, machine_id, &selection.scope).await {
                    return Err(AttachmentError::RouteChanged);
                }
                workspace = next;
            }
            event = control.next_event() => {
                let event = match event {
                    Ok(event) => event,
                    Err(tmux::TmuxError::Ssh(ssh::SshError::Unavailable)) => {
                        return Ok(WorkspaceExit::Chooser);
                    }
                    Err(error) => return Err(map_tmux(error)),
                };
                match event {
                    tmux::ControlEvent::Output { pane_id, data } => {
                        if workspace.visible_panes.contains(&pane_id) {
                            send(socket, &ServerFrame::Output {
                                workspace_epoch: workspace.epoch,
                                pane_id,
                                data_base64: URL_SAFE_NO_PAD.encode(data),
                            }).await?;
                        }
                    }
                    tmux::ControlEvent::Refresh => {
                        projection_dirty = true;
                        if pending_dispatch.is_none() {
                            pending_dispatch = Some(Box::pin(
                                Arc::clone(&selection.scope).dispatch_owned(),
                            ));
                        }
                    }
                    tmux::ControlEvent::Exit => return Ok(WorkspaceExit::Chooser),
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn handle_workspace_frame(
    socket: &mut impl AttachmentSocket,
    state: &Arc<ServerState>,
    machine_id: Uuid,
    attachment_id: Uuid,
    selection: &Selection,
    control_tx: &mpsc::Sender<ControlDirective>,
    control: &mut tmux::ControlAdapter,
    workspace: &mut WorkspaceState,
    frame: ClientFrame,
) -> Result<WorkspaceFrameExit, AttachmentError> {
    match frame {
        ClientFrame::WorkspaceReturnToChooser {
            machine_connection_epoch,
            workspace_epoch,
        } => {
            if workspace.matches(&machine_connection_epoch, workspace_epoch) {
                Ok(WorkspaceFrameExit::Chooser)
            } else {
                send_workspace_error(socket, "stale_epoch", "Workspace state is stale.").await?;
                Ok(WorkspaceFrameExit::Continue)
            }
        }
        ClientFrame::WorkspaceDetach => Ok(WorkspaceFrameExit::Detach),
        ClientFrame::WriterClaim {
            request_id,
            machine_connection_epoch,
            attachment_epoch,
            columns,
            rows,
        } => {
            let request = WriterRequest {
                request_id,
                operation: Operation::WriterClaim,
                machine_connection_epoch,
                attachment_epoch,
                size: ClientSize { columns, rows },
            };
            change_writer_with_control(
                socket,
                state,
                machine_id,
                attachment_id,
                selection,
                control_tx,
                control,
                workspace,
                request,
                false,
            )
            .await
        }
        ClientFrame::WriterTakeover {
            request_id,
            machine_connection_epoch,
            attachment_epoch,
            columns,
            rows,
        } => {
            let request = WriterRequest {
                request_id,
                operation: Operation::WriterTakeover,
                machine_connection_epoch,
                attachment_epoch,
                size: ClientSize { columns, rows },
            };
            change_writer_with_control(
                socket,
                state,
                machine_id,
                attachment_id,
                selection,
                control_tx,
                control,
                workspace,
                request,
                true,
            )
            .await
        }
        ClientFrame::PaneInput {
            request_id,
            machine_connection_epoch,
            workspace_epoch,
            pane_id,
            data_base64,
        } => {
            if !workspace.matches(&machine_connection_epoch, workspace_epoch)
                || workspace.active_pane != pane_id
            {
                send_operation_result(
                    socket,
                    request_id,
                    Operation::PaneInput,
                    PublicOutcome::Failed,
                    "stale_epoch",
                    "Pane input was rejected before dispatch.",
                )
                .await?;
                return Ok(WorkspaceFrameExit::Continue);
            }
            let data = decode_input(&data_base64).ok_or(AttachmentError::Protocol)?;
            let Some(_dispatch) =
                try_dispatch_operation(socket, &selection.scope, request_id, Operation::PaneInput)
                    .await?
            else {
                return Ok(WorkspaceFrameExit::Continue);
            };
            let mut writer = selection.scope.lock().await;
            if !writer.is_current(attachment_id) {
                send_writer_required(socket, request_id, Operation::PaneInput).await?;
                return Ok(WorkspaceFrameExit::Continue);
            }
            if !current_route(state, machine_id, &selection.scope).await {
                return Err(AttachmentError::RouteChanged);
            }
            let outcome = control
                .send_literal(&pane_id, &data)
                .await
                .map_err(map_tmux)?;
            if !current_route(state, machine_id, &selection.scope).await {
                writer.clear_if_current(attachment_id);
                return Err(AttachmentError::RouteChanged);
            }
            mutation_result(
                socket,
                &mut writer,
                attachment_id,
                request_id,
                Operation::PaneInput,
                outcome,
            )
            .await
        }
        ClientFrame::ClientResize {
            request_id,
            machine_connection_epoch,
            workspace_epoch,
            columns,
            rows,
        } => {
            if !workspace.matches(&machine_connection_epoch, workspace_epoch) {
                send_stale_operation(socket, request_id, Operation::ClientResize).await?;
                return Ok(WorkspaceFrameExit::Continue);
            }
            if !valid_client_size(ClientSize { columns, rows }) {
                send_operation_result(
                    socket,
                    request_id,
                    Operation::ClientResize,
                    PublicOutcome::Failed,
                    "invalid_operation",
                    "The requested Browser size is outside the supported bounds.",
                )
                .await?;
                return Ok(WorkspaceFrameExit::Continue);
            }
            let Some(_dispatch) = try_dispatch_operation(
                socket,
                &selection.scope,
                request_id,
                Operation::ClientResize,
            )
            .await?
            else {
                return Ok(WorkspaceFrameExit::Continue);
            };
            let mut writer = selection.scope.lock().await;
            if !writer.is_current(attachment_id) {
                send_writer_required(socket, request_id, Operation::ClientResize).await?;
                return Ok(WorkspaceFrameExit::Continue);
            }
            if !current_route(state, machine_id, &selection.scope).await {
                return Err(AttachmentError::RouteChanged);
            }
            let outcome = control.resize(columns, rows).await.map_err(map_tmux)?;
            if !current_route(state, machine_id, &selection.scope).await {
                writer.clear_if_current(attachment_id);
                return Err(AttachmentError::RouteChanged);
            }
            if outcome != tmux::MutationOutcome::Succeeded {
                return mutation_result(
                    socket,
                    &mut writer,
                    attachment_id,
                    request_id,
                    Operation::ClientResize,
                    outcome,
                )
                .await;
            }
            writer.update_size(attachment_id, ClientSize { columns, rows });
            send_operation_result(
                socket,
                request_id,
                Operation::ClientResize,
                PublicOutcome::Succeeded,
                "resize_applied",
                "The target accepted the Browser client size.",
            )
            .await?;
            refresh_after_mutation(socket, state, machine_id, control, workspace, selection).await
        }
        ClientFrame::WindowSelect {
            request_id,
            machine_connection_epoch,
            workspace_epoch,
            window_id,
        } => {
            if !workspace.matches(&machine_connection_epoch, workspace_epoch)
                || !workspace.windows.contains(&window_id)
            {
                send_stale_operation(socket, request_id, Operation::WindowSelect).await?;
                return Ok(WorkspaceFrameExit::Continue);
            }
            let Some(_dispatch) = try_dispatch_operation(
                socket,
                &selection.scope,
                request_id,
                Operation::WindowSelect,
            )
            .await?
            else {
                return Ok(WorkspaceFrameExit::Continue);
            };
            let mut writer = selection.scope.lock().await;
            if !writer.is_current(attachment_id) {
                send_writer_required(socket, request_id, Operation::WindowSelect).await?;
                return Ok(WorkspaceFrameExit::Continue);
            }
            if !current_route(state, machine_id, &selection.scope).await {
                return Err(AttachmentError::RouteChanged);
            }
            let outcome = control.select_window(&window_id).await.map_err(map_tmux)?;
            if !current_route(state, machine_id, &selection.scope).await {
                writer.clear_if_current(attachment_id);
                return Err(AttachmentError::RouteChanged);
            }
            if outcome != tmux::MutationOutcome::Succeeded {
                return mutation_result(
                    socket,
                    &mut writer,
                    attachment_id,
                    request_id,
                    Operation::WindowSelect,
                    outcome,
                )
                .await;
            }
            send_operation_result(
                socket,
                request_id,
                Operation::WindowSelect,
                PublicOutcome::Succeeded,
                "window_selected",
                "The target selected the observed window.",
            )
            .await?;
            refresh_after_mutation(socket, state, machine_id, control, workspace, selection).await
        }
        ClientFrame::PaneSelect {
            request_id,
            machine_connection_epoch,
            workspace_epoch,
            pane_id,
        } => {
            if !workspace.matches(&machine_connection_epoch, workspace_epoch)
                || !workspace.visible_panes.contains(&pane_id)
            {
                send_stale_operation(socket, request_id, Operation::PaneSelect).await?;
                return Ok(WorkspaceFrameExit::Continue);
            }
            let Some(_dispatch) =
                try_dispatch_operation(socket, &selection.scope, request_id, Operation::PaneSelect)
                    .await?
            else {
                return Ok(WorkspaceFrameExit::Continue);
            };
            let mut writer = selection.scope.lock().await;
            if !writer.is_current(attachment_id) {
                send_writer_required(socket, request_id, Operation::PaneSelect).await?;
                return Ok(WorkspaceFrameExit::Continue);
            }
            if !current_route(state, machine_id, &selection.scope).await {
                return Err(AttachmentError::RouteChanged);
            }
            let outcome = control.select_pane(&pane_id).await.map_err(map_tmux)?;
            if !current_route(state, machine_id, &selection.scope).await {
                writer.clear_if_current(attachment_id);
                return Err(AttachmentError::RouteChanged);
            }
            if outcome != tmux::MutationOutcome::Succeeded {
                return mutation_result(
                    socket,
                    &mut writer,
                    attachment_id,
                    request_id,
                    Operation::PaneSelect,
                    outcome,
                )
                .await;
            }
            send_operation_result(
                socket,
                request_id,
                Operation::PaneSelect,
                PublicOutcome::Succeeded,
                "pane_selected",
                "The target selected the observed pane.",
            )
            .await?;
            refresh_after_mutation(socket, state, machine_id, control, workspace, selection).await
        }
        ClientFrame::WorkspaceRefresh {
            request_id,
            machine_connection_epoch,
            workspace_epoch,
        } => {
            if !workspace.matches(&machine_connection_epoch, workspace_epoch) {
                send_stale_operation(socket, request_id, Operation::WorkspaceRefresh).await?;
                return Ok(WorkspaceFrameExit::Continue);
            }
            let Some(_dispatch) = try_dispatch_operation(
                socket,
                &selection.scope,
                request_id,
                Operation::WorkspaceRefresh,
            )
            .await?
            else {
                return Ok(WorkspaceFrameExit::Continue);
            };
            let writer = selection.scope.lock().await;
            if !writer.is_current(attachment_id) {
                send_writer_required(socket, request_id, Operation::WorkspaceRefresh).await?;
                return Ok(WorkspaceFrameExit::Continue);
            }
            if !current_route(state, machine_id, &selection.scope).await {
                return Err(AttachmentError::RouteChanged);
            }
            let projection = install_projection(
                socket,
                control,
                selection.route.connection_epoch,
                &selection.session_id,
                selection.session_created,
            )
            .await?;
            let Some(next) = projection else {
                return Ok(WorkspaceFrameExit::Chooser);
            };
            if !current_route(state, machine_id, &selection.scope).await {
                return Err(AttachmentError::RouteChanged);
            }
            *workspace = next;
            send_operation_result(
                socket,
                request_id,
                Operation::WorkspaceRefresh,
                PublicOutcome::Succeeded,
                "projection_refreshed",
                "A fresh target projection was installed.",
            )
            .await?;
            Ok(WorkspaceFrameExit::Continue)
        }
        _ => Err(AttachmentError::Protocol),
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn change_writer_with_control(
    socket: &mut impl AttachmentSocket,
    state: &Arc<ServerState>,
    machine_id: Uuid,
    attachment_id: Uuid,
    selection: &Selection,
    control_tx: &mpsc::Sender<ControlDirective>,
    control: &mut tmux::ControlAdapter,
    workspace: &mut WorkspaceState,
    request: WriterRequest,
    takeover: bool,
) -> Result<WorkspaceFrameExit, AttachmentError> {
    if request.attachment_epoch != workspace.epoch
        || !matches_connection_epoch(
            &request.machine_connection_epoch,
            selection.route.connection_epoch,
        )
    {
        send_stale_operation(socket, request.request_id, request.operation).await?;
        return Ok(WorkspaceFrameExit::Continue);
    }
    if !valid_client_size(request.size) {
        send_operation_result(
            socket,
            request.request_id,
            request.operation,
            PublicOutcome::Failed,
            "invalid_operation",
            "The requested Browser size is outside the supported bounds.",
        )
        .await?;
        return Ok(WorkspaceFrameExit::Continue);
    }
    let Some(_dispatch) = try_dispatch_operation(
        socket,
        &selection.scope,
        request.request_id,
        request.operation,
    )
    .await?
    else {
        return Ok(WorkspaceFrameExit::Continue);
    };
    let mut writer = selection.scope.lock().await;
    if !current_route(state, machine_id, &selection.scope).await {
        return Err(AttachmentError::RouteChanged);
    }
    if !writer.is_available() && !writer.is_current(attachment_id) && !takeover {
        send_operation_result(
            socket,
            request.request_id,
            request.operation,
            PublicOutcome::Failed,
            "writer_busy",
            "Another OwlMux Browser attachment is the current writer.",
        )
        .await?;
        return Ok(WorkspaceFrameExit::Continue);
    }
    let old_control = if writer.is_current(attachment_id) {
        None
    } else {
        writer.current_control()
    };
    if let Some(old) = old_control.as_ref() {
        let outcome = demote_writer(old).await;
        if !current_route(state, machine_id, &selection.scope).await {
            writer.clear();
            let _ = old.send(ControlDirective::Close).await;
            return Err(AttachmentError::RouteChanged);
        }
        if outcome != ControlOutcome::Succeeded {
            writer.clear();
            let _ = old.send(ControlDirective::Close).await;
            send_operation_result(
                socket,
                request.request_id,
                request.operation,
                if outcome == ControlOutcome::Ambiguous {
                    PublicOutcome::Ambiguous
                } else {
                    PublicOutcome::Failed
                },
                if outcome == ControlOutcome::Ambiguous {
                    "operation_ambiguous"
                } else {
                    "writer_transition_failed"
                },
                "Writer transition failed closed; no writer was retained.",
            )
            .await?;
            return Ok(WorkspaceFrameExit::Chooser);
        }
    }
    if writer.is_current(attachment_id) {
        writer.update_size(attachment_id, request.size);
        writer.set_control(attachment_id, control_tx.clone());
    } else {
        let outcome = control.set_writer(true).await;
        if !current_route(state, machine_id, &selection.scope).await {
            writer.clear();
            if let Some(old) = old_control {
                let _ = old.send(ControlDirective::Close).await;
            }
            return Err(AttachmentError::RouteChanged);
        }
        if outcome != tmux::MutationOutcome::Succeeded {
            writer.clear();
            if let Some(old) = old_control {
                let _ = old.send(ControlDirective::Close).await;
            }
            send_operation_result(
                socket,
                request.request_id,
                request.operation,
                if outcome == tmux::MutationOutcome::Ambiguous {
                    PublicOutcome::Ambiguous
                } else {
                    PublicOutcome::Failed
                },
                if outcome == tmux::MutationOutcome::Ambiguous {
                    "operation_ambiguous"
                } else {
                    "writer_transition_failed"
                },
                "Writer transition failed closed; no writer was retained.",
            )
            .await?;
            return Ok(WorkspaceFrameExit::Chooser);
        }
        writer.set_current(attachment_id, request.size, Some(control_tx.clone()));
    }
    let resize = control
        .resize(request.size.columns, request.size.rows)
        .await
        .map_err(map_tmux)?;
    if !current_route(state, machine_id, &selection.scope).await {
        writer.clear_if_current(attachment_id);
        if let Some(old) = old_control {
            let _ = old.send(ControlDirective::Close).await;
        }
        return Err(AttachmentError::RouteChanged);
    }
    if resize != tmux::MutationOutcome::Succeeded {
        writer.clear_if_current(attachment_id);
        if let Some(old) = old_control {
            let _ = old.send(ControlDirective::Close).await;
        }
        send_operation_result(
            socket,
            request.request_id,
            request.operation,
            if resize == tmux::MutationOutcome::Ambiguous {
                PublicOutcome::Ambiguous
            } else {
                PublicOutcome::Failed
            },
            if resize == tmux::MutationOutcome::Ambiguous {
                "operation_ambiguous"
            } else {
                "writer_transition_failed"
            },
            "Writer resize failed closed; no writer was retained.",
        )
        .await?;
        return Ok(WorkspaceFrameExit::Chooser);
    }
    let projection = install_projection(
        socket,
        control,
        selection.route.connection_epoch,
        &selection.session_id,
        selection.session_created,
    )
    .await?;
    let Some(next) = projection else {
        writer.clear_if_current(attachment_id);
        return Ok(WorkspaceFrameExit::Chooser);
    };
    if !current_route(state, machine_id, &selection.scope).await {
        writer.clear_if_current(attachment_id);
        return Err(AttachmentError::RouteChanged);
    }
    *workspace = next;
    send_operation_result(
        socket,
        request.request_id,
        request.operation,
        PublicOutcome::Succeeded,
        if takeover {
            "writer_taken_over"
        } else {
            "writer_claimed"
        },
        "Writer access is active after a fresh target projection.",
    )
    .await?;
    Ok(WorkspaceFrameExit::Continue)
}

async fn try_dispatch_operation<'a>(
    socket: &mut impl AttachmentSocket,
    scope: &'a WriterScope,
    request_id: Uuid,
    operation: Operation,
) -> Result<Option<tokio::sync::MutexGuard<'a, ()>>, AttachmentError> {
    let Some(dispatch) = scope.try_dispatch() else {
        send_operation_result(
            socket,
            request_id,
            operation,
            PublicOutcome::Failed,
            "writer_busy",
            "Another target operation is already being dispatched.",
        )
        .await?;
        return Ok(None);
    };
    Ok(Some(dispatch))
}

async fn mutation_result(
    socket: &mut impl AttachmentSocket,
    writer: &mut crate::writer::WriterGuard<'_>,
    attachment_id: Uuid,
    request_id: Uuid,
    operation: Operation,
    outcome: tmux::MutationOutcome,
) -> Result<WorkspaceFrameExit, AttachmentError> {
    match outcome {
        tmux::MutationOutcome::Succeeded => {
            send_operation_result(
                socket,
                request_id,
                operation,
                PublicOutcome::Succeeded,
                "operation_succeeded",
                "The target confirmed the operation.",
            )
            .await?;
            Ok(WorkspaceFrameExit::Continue)
        }
        tmux::MutationOutcome::Failed => {
            send_operation_result(
                socket,
                request_id,
                operation,
                PublicOutcome::Failed,
                "target_rejected",
                "The target rejected the operation without applying it.",
            )
            .await?;
            Ok(WorkspaceFrameExit::Continue)
        }
        tmux::MutationOutcome::Ambiguous => {
            writer.clear_if_current(attachment_id);
            send_operation_result(
                socket,
                request_id,
                operation,
                PublicOutcome::Ambiguous,
                "operation_ambiguous",
                "The operation may have occurred. It will not be retried.",
            )
            .await?;
            Ok(WorkspaceFrameExit::Chooser)
        }
    }
}

async fn refresh_after_mutation(
    socket: &mut impl AttachmentSocket,
    state: &ServerState,
    machine_id: Uuid,
    control: &mut tmux::ControlAdapter,
    workspace: &mut WorkspaceState,
    selection: &Selection,
) -> Result<WorkspaceFrameExit, AttachmentError> {
    let projection = install_projection(
        socket,
        control,
        selection.route.connection_epoch,
        &selection.session_id,
        selection.session_created,
    )
    .await?;
    let Some(next) = projection else {
        return Ok(WorkspaceFrameExit::Chooser);
    };
    if !current_route(state, machine_id, &selection.scope).await {
        return Err(AttachmentError::RouteChanged);
    }
    *workspace = next;
    Ok(WorkspaceFrameExit::Continue)
}

async fn open_relay_stream(
    state: &ServerState,
    machine_id: Uuid,
) -> Result<crate::relay::RelayStream, AttachmentError> {
    let deadline = Instant::now() + LOCAL_ROUTE_WAIT;
    loop {
        if let Ok(stream) = state.relays.open_stream(machine_id).await {
            return Ok(stream);
        }
        if Instant::now() >= deadline || state.lease.check().is_err() {
            return Err(AttachmentError::Unavailable);
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn current_route(state: &ServerState, machine_id: Uuid, scope: &WriterScope) -> bool {
    state.lease.check().is_ok()
        && state
            .relays
            .is_current_route(machine_id, scope.route())
            .await
}

struct WorkspaceState {
    epoch: Uuid,
    connection_epoch: i64,
    windows: HashSet<String>,
    visible_panes: HashSet<String>,
    active_pane: String,
}

impl WorkspaceState {
    fn matches(&self, connection_epoch: &str, workspace_epoch: Uuid) -> bool {
        workspace_epoch == self.epoch
            && matches_connection_epoch(connection_epoch, self.connection_epoch)
    }
}

async fn install_projection(
    socket: &mut impl AttachmentSocket,
    control: &mut tmux::ControlAdapter,
    connection_epoch: i64,
    session_id: &str,
    session_created: i64,
) -> Result<Option<WorkspaceState>, AttachmentError> {
    timeout(
        PROJECTION_INSTALL_TIMEOUT,
        install_projection_inner(
            socket,
            control,
            connection_epoch,
            session_id,
            session_created,
        ),
    )
    .await
    .map_err(|_| AttachmentError::Unavailable)?
}

async fn install_projection_inner(
    socket: &mut impl AttachmentSocket,
    control: &mut tmux::ControlAdapter,
    connection_epoch: i64,
    session_id: &str,
    session_created: i64,
) -> Result<Option<WorkspaceState>, AttachmentError> {
    send(
        socket,
        &ServerFrame::Phase {
            phase: Phase::Connecting,
        },
    )
    .await?;
    let projection = match control.hydrate(session_id, session_created).await {
        Ok(projection) => projection,
        Err(
            tmux::TmuxError::Changed
            | tmux::TmuxError::Target
            | tmux::TmuxError::Ssh(ssh::SshError::Unavailable),
        ) => return Ok(None),
        Err(error) => return Err(map_tmux(error)),
    };
    let workspace_epoch = Uuid::new_v4();
    let active_pane = projection
        .panes
        .iter()
        .find(|pane| pane.active)
        .map(|pane| pane.pane_id.clone())
        .ok_or(AttachmentError::Protocol)?;
    let visible_panes = projection
        .panes
        .iter()
        .map(|pane| pane.pane_id.clone())
        .collect::<HashSet<_>>();
    let windows = projection
        .windows
        .iter()
        .map(|window| window.window_id.clone())
        .collect::<HashSet<_>>();
    send_with_control(
        socket,
        control,
        &ServerFrame::Projection {
            machine_connection_epoch: connection_epoch.to_string(),
            workspace_epoch,
            session_id: session_id.to_owned(),
            session_created,
            windows: projection.windows,
            window: projection.window,
            panes: projection.panes.clone(),
        },
    )
    .await?;
    for snapshot in projection.snapshots {
        let chunks = snapshot
            .content
            .chunks(ATTACHMENT_MAX_TERMINAL_CHUNK_BYTES)
            .collect::<Vec<_>>();
        if chunks.is_empty() {
            send_with_control(
                socket,
                control,
                &ServerFrame::PaneSnapshot {
                    workspace_epoch,
                    pane_id: snapshot.pane_id,
                    chunk_index: 0,
                    final_chunk: true,
                    data_base64: String::new(),
                },
            )
            .await?;
            continue;
        }
        let final_index = chunks.len() - 1;
        for (chunk_index, chunk) in chunks.into_iter().enumerate() {
            send_with_control(
                socket,
                control,
                &ServerFrame::PaneSnapshot {
                    workspace_epoch,
                    pane_id: snapshot.pane_id.clone(),
                    chunk_index,
                    final_chunk: chunk_index == final_index,
                    data_base64: URL_SAFE_NO_PAD.encode(chunk),
                },
            )
            .await?;
        }
    }
    send_with_control(
        socket,
        control,
        &ServerFrame::Phase {
            phase: Phase::Ready,
        },
    )
    .await?;
    Ok(Some(WorkspaceState {
        epoch: workspace_epoch,
        connection_epoch,
        windows,
        visible_panes,
        active_pane,
    }))
}

async fn send_writer_state(
    socket: &mut impl AttachmentSocket,
    scope: &WriterScope,
    attachment_id: Uuid,
    attachment_epoch: Uuid,
) -> Result<(), AttachmentError> {
    let WriterView {
        is_writer,
        writer_available,
    } = scope.view(attachment_id).await;
    send(
        socket,
        &ServerFrame::WriterState {
            machine_connection_epoch: scope.route().connection_epoch.to_string(),
            attachment_epoch,
            role: if is_writer {
                WriterRole::Writer
            } else {
                WriterRole::Observer
            },
            writer_available,
        },
    )
    .await
}

async fn demote_writer(control: &mpsc::Sender<ControlDirective>) -> ControlOutcome {
    let (response_tx, response_rx) = oneshot::channel();
    let sent = timeout(
        WRITER_TRANSITION_TIMEOUT,
        control.send(ControlDirective::Demote {
            response: response_tx,
        }),
    )
    .await;
    if !matches!(sent, Ok(Ok(()))) {
        return ControlOutcome::Ambiguous;
    }
    timeout(WRITER_TRANSITION_TIMEOUT, response_rx)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(ControlOutcome::Ambiguous)
}

fn to_control_outcome(outcome: tmux::MutationOutcome) -> ControlOutcome {
    match outcome {
        tmux::MutationOutcome::Succeeded => ControlOutcome::Succeeded,
        tmux::MutationOutcome::Failed => ControlOutcome::Failed,
        tmux::MutationOutcome::Ambiguous => ControlOutcome::Ambiguous,
    }
}

fn matches_connection_epoch(value: &str, expected: i64) -> bool {
    expected > 0 && value.parse::<i64>() == Ok(expected) && value == expected.to_string()
}

fn valid_client_size(size: ClientSize) -> bool {
    size.columns > 0
        && size.columns <= ATTACHMENT_MAX_DIMENSION
        && size.rows > 0
        && size.rows <= ATTACHMENT_MAX_DIMENSION
        && u64::from(size.columns) * u64::from(size.rows) <= ATTACHMENT_MAX_GRID_CELLS
}

fn valid_session_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn decode_input(value: &str) -> Option<Vec<u8>> {
    if value.is_empty() || value.len() > 1366 {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(value).ok()?;
    if bytes.is_empty()
        || bytes.len() > ATTACHMENT_MAX_INPUT_BYTES
        || URL_SAFE_NO_PAD.encode(&bytes) != value
    {
        return None;
    }
    Some(bytes)
}

struct SecretString(Zeroizing<String>);

impl SecretString {
    fn as_str(&self) -> &str {
        self.0.as_str()
    }

    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(|value| Self(Zeroizing::new(value)))
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum AuthFrame {
    #[serde(rename = "auth.api_key")]
    ApiKey { api_key: SecretString },
}

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum ClientFrame {
    #[serde(rename = "writer.claim")]
    WriterClaim {
        request_id: Uuid,
        machine_connection_epoch: String,
        attachment_epoch: Uuid,
        columns: u32,
        rows: u32,
    },
    #[serde(rename = "writer.takeover")]
    WriterTakeover {
        request_id: Uuid,
        machine_connection_epoch: String,
        attachment_epoch: Uuid,
        columns: u32,
        rows: u32,
    },
    #[serde(rename = "session.select")]
    SessionSelect {
        machine_connection_epoch: String,
        selection_epoch: Uuid,
        session_id: String,
        session_created: i64,
    },
    #[serde(rename = "session.refresh")]
    SessionRefresh {
        request_id: Uuid,
        machine_connection_epoch: String,
        selection_epoch: Uuid,
    },
    #[serde(rename = "session.create")]
    SessionCreate {
        request_id: Uuid,
        machine_connection_epoch: String,
        selection_epoch: Uuid,
        name: String,
    },
    #[serde(rename = "workspace.return_to_chooser")]
    WorkspaceReturnToChooser {
        machine_connection_epoch: String,
        workspace_epoch: Uuid,
    },
    #[serde(rename = "window.select")]
    WindowSelect {
        request_id: Uuid,
        machine_connection_epoch: String,
        workspace_epoch: Uuid,
        window_id: String,
    },
    #[serde(rename = "pane.select")]
    PaneSelect {
        request_id: Uuid,
        machine_connection_epoch: String,
        workspace_epoch: Uuid,
        pane_id: String,
    },
    #[serde(rename = "pane.input")]
    PaneInput {
        request_id: Uuid,
        machine_connection_epoch: String,
        workspace_epoch: Uuid,
        pane_id: String,
        data_base64: String,
    },
    #[serde(rename = "client.resize")]
    ClientResize {
        request_id: Uuid,
        machine_connection_epoch: String,
        workspace_epoch: Uuid,
        columns: u32,
        rows: u32,
    },
    #[serde(rename = "workspace.refresh")]
    WorkspaceRefresh {
        request_id: Uuid,
        machine_connection_epoch: String,
        workspace_epoch: Uuid,
    },
    #[serde(rename = "workspace.detach")]
    WorkspaceDetach,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum ServerFrame {
    #[serde(rename = "workspace.phase")]
    Phase { phase: Phase },
    #[serde(rename = "session.list")]
    SessionList {
        machine_connection_epoch: String,
        selection_epoch: Uuid,
        tmux_client_version: String,
        tmux_server_version: Option<String>,
        sessions: Vec<tmux::SessionSummary>,
    },
    #[serde(rename = "writer.state")]
    WriterState {
        machine_connection_epoch: String,
        attachment_epoch: Uuid,
        role: WriterRole,
        writer_available: bool,
    },
    #[serde(rename = "operation.result")]
    OperationResult {
        request_id: Uuid,
        operation: Operation,
        outcome: PublicOutcome,
        code: &'static str,
        message: &'static str,
    },
    #[serde(rename = "workspace.projection")]
    Projection {
        machine_connection_epoch: String,
        workspace_epoch: Uuid,
        session_id: String,
        session_created: i64,
        windows: Vec<tmux::WindowSummary>,
        window: tmux::WindowProjection,
        panes: Vec<tmux::PaneProjection>,
    },
    #[serde(rename = "workspace.pane_snapshot")]
    PaneSnapshot {
        workspace_epoch: Uuid,
        pane_id: String,
        chunk_index: usize,
        #[serde(rename = "final")]
        final_chunk: bool,
        data_base64: String,
    },
    #[serde(rename = "workspace.output")]
    Output {
        workspace_epoch: Uuid,
        pane_id: String,
        data_base64: String,
    },
    #[serde(rename = "workspace.error")]
    Error {
        code: &'static str,
        message: &'static str,
    },
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Connecting,
    Selecting,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum WriterRole {
    Observer,
    Writer,
}

#[derive(Clone, Copy, Serialize)]
pub(crate) enum Operation {
    #[serde(rename = "writer.claim")]
    WriterClaim,
    #[serde(rename = "writer.takeover")]
    WriterTakeover,
    #[serde(rename = "session.refresh")]
    SessionRefresh,
    #[serde(rename = "session.create")]
    SessionCreate,
    #[serde(rename = "window.select")]
    WindowSelect,
    #[serde(rename = "pane.select")]
    PaneSelect,
    #[serde(rename = "pane.input")]
    PaneInput,
    #[serde(rename = "client.resize")]
    ClientResize,
    #[serde(rename = "workspace.refresh")]
    WorkspaceRefresh,
}

impl Operation {
    const fn audit_action(self) -> Option<&'static str> {
        match self {
            Self::WriterClaim => Some("writer_claim"),
            Self::WriterTakeover => Some("writer_takeover"),
            Self::SessionCreate => Some("tmux_session_create"),
            Self::WindowSelect => Some("tmux_window_select"),
            Self::PaneSelect => Some("tmux_pane_select"),
            Self::WorkspaceRefresh => Some("tmux_projection_refresh"),
            Self::SessionRefresh | Self::PaneInput | Self::ClientResize => None,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PublicOutcome {
    Succeeded,
    Failed,
    Ambiguous,
}

impl PublicOutcome {
    const fn audit_outcome(self) -> &'static str {
        match self {
            Self::Succeeded => "success",
            Self::Failed => "rejected",
            Self::Ambiguous => "ambiguous",
        }
    }
}

async fn audit_event(
    state: &ServerState,
    machine_id: Uuid,
    action: &'static str,
    outcome: &'static str,
) {
    let result = timeout(
        Duration::from_secs(1),
        record_audit(
            state.database.ordinary(),
            state.database.deployment_id(),
            "machine",
            Some(machine_id),
            None,
            action,
            outcome,
        ),
    )
    .await;
    if !matches!(result, Ok(Ok(()))) {
        tracing::warn!(%machine_id, action, outcome, "safe durable audit write failed");
    }
}

async fn receive_auth(socket: &mut WebSocket) -> Result<AuthFrame, AttachmentError> {
    let message = timeout(AUTH_TIMEOUT, socket.next())
        .await
        .map_err(|_| AttachmentError::Protocol)?
        .ok_or(AttachmentError::Unavailable)?
        .map_err(|_| AttachmentError::Protocol)?;
    decode_auth_frame(message)
}

fn decode_auth_frame(message: Message) -> Result<AuthFrame, AttachmentError> {
    let text = match message {
        Message::Text(text) => text,
        Message::Close(_) => return Err(AttachmentError::ClientClosed),
        _ => return Err(AttachmentError::Protocol),
    };
    let mut raw = text.as_str().as_bytes().to_vec();
    drop(text);
    let result = if raw.len() > ATTACHMENT_MAX_FRAME_BYTES {
        Err(AttachmentError::Protocol)
    } else {
        serde_json::from_slice(&raw).map_err(|_| AttachmentError::Protocol)
    };
    raw.zeroize();
    result
}

async fn receive(socket: &mut impl AttachmentSocket) -> Result<ClientFrame, AttachmentError> {
    let text = socket.receive_text().await.map_err(map_wire_error)?;
    decode_text_frame(&text)
}

fn decode_text_frame<T: DeserializeOwned>(text: &str) -> Result<T, AttachmentError> {
    if text.len() > ATTACHMENT_MAX_FRAME_BYTES {
        return Err(AttachmentError::Protocol);
    }
    serde_json::from_str(text).map_err(|_| AttachmentError::Protocol)
}

async fn send(
    socket: &mut impl AttachmentSocket,
    frame: &ServerFrame,
) -> Result<(), AttachmentError> {
    let encoded = serde_json::to_string(frame).map_err(|_| AttachmentError::Protocol)?;
    if encoded.len() > ATTACHMENT_MAX_FRAME_BYTES {
        return Err(AttachmentError::Protocol);
    }
    timeout(CLIENT_WRITE_TIMEOUT, socket.send_text(encoded))
        .await
        .map_err(|_| AttachmentError::Unavailable)?
        .map_err(map_wire_error)
}

async fn send_with_control(
    socket: &mut impl AttachmentSocket,
    control: &mut tmux::ControlAdapter,
    frame: &ServerFrame,
) -> Result<(), AttachmentError> {
    let encoded = serde_json::to_string(frame).map_err(|_| AttachmentError::Protocol)?;
    if encoded.len() > ATTACHMENT_MAX_FRAME_BYTES {
        return Err(AttachmentError::Protocol);
    }
    let write = timeout(CLIENT_WRITE_TIMEOUT, socket.send_text(encoded));
    tokio::pin!(write);
    loop {
        tokio::select! {
            result = &mut write => {
                return result
                    .map_err(|_| AttachmentError::Unavailable)?
                    .map_err(map_wire_error);
            }
            result = control.pump_event() => result.map_err(map_tmux)?,
        }
    }
}

fn map_wire_error(error: AttachmentWireError) -> AttachmentError {
    match error {
        AttachmentWireError::Closed => AttachmentError::ClientClosed,
        AttachmentWireError::Protocol => AttachmentError::Protocol,
        AttachmentWireError::Unavailable => AttachmentError::Unavailable,
    }
}

async fn send_operation_result(
    socket: &mut impl AttachmentSocket,
    request_id: Uuid,
    operation: Operation,
    outcome: PublicOutcome,
    code: &'static str,
    message: &'static str,
) -> Result<(), AttachmentError> {
    socket.audit_operation(operation, outcome).await;
    send(
        socket,
        &ServerFrame::OperationResult {
            request_id,
            operation,
            outcome,
            code,
            message,
        },
    )
    .await
}

async fn send_writer_required(
    socket: &mut impl AttachmentSocket,
    request_id: Uuid,
    operation: Operation,
) -> Result<(), AttachmentError> {
    send_operation_result(
        socket,
        request_id,
        operation,
        PublicOutcome::Failed,
        "writer_required",
        "Only the current OwlMux Browser writer may perform this operation.",
    )
    .await
}

async fn send_stale_operation(
    socket: &mut impl AttachmentSocket,
    request_id: Uuid,
    operation: Operation,
) -> Result<(), AttachmentError> {
    send_operation_result(
        socket,
        request_id,
        operation,
        PublicOutcome::Failed,
        "stale_epoch",
        "The workspace changed before dispatch.",
    )
    .await
}

async fn send_workspace_error(
    socket: &mut impl AttachmentSocket,
    code: &'static str,
    message: &'static str,
) -> Result<(), AttachmentError> {
    send(socket, &ServerFrame::Error { code, message }).await
}

async fn send_fatal_error(
    socket: &mut impl AttachmentSocket,
    code: &'static str,
    message: &'static str,
) -> Result<(), AttachmentError> {
    send(
        socket,
        &ServerFrame::Phase {
            phase: Phase::Failed,
        },
    )
    .await?;
    send_workspace_error(socket, code, message).await?;
    let close_code = match code {
        "protocol_error" => ATTACHMENT_CLOSE_PROTOCOL_ERROR,
        "unauthenticated" | "invalid_origin" | "tmux_incompatible" => {
            ATTACHMENT_CLOSE_POLICY_VIOLATION
        }
        _ => ATTACHMENT_CLOSE_TEMPORARILY_UNAVAILABLE,
    };
    timeout(CLIENT_WRITE_TIMEOUT, socket.close(close_code, code))
        .await
        .map_err(|_| AttachmentError::Unavailable)?;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum WorkspaceExit {
    Chooser,
    Detach,
}

#[derive(Clone, Copy, Debug)]
enum WorkspaceFrameExit {
    Continue,
    Chooser,
    Detach,
}

#[derive(Debug)]
enum AttachmentError {
    ClientClosed,
    Ssh,
    Tmux,
    TmuxIncompatible,
    Protocol,
    Unavailable,
    RouteChanged,
}

impl AttachmentError {
    const fn code(&self) -> &'static str {
        match self {
            Self::TmuxIncompatible => "tmux_incompatible",
            Self::Protocol => "protocol_error",
            Self::ClientClosed
            | Self::Ssh
            | Self::Tmux
            | Self::Unavailable
            | Self::RouteChanged => "target_unavailable",
        }
    }

    const fn message(&self) -> &'static str {
        match self {
            Self::TmuxIncompatible => "Target tmux is incompatible.",
            Self::Protocol => "Attachment protocol failed closed.",
            Self::ClientClosed
            | Self::Ssh
            | Self::Tmux
            | Self::Unavailable
            | Self::RouteChanged => "Target is unavailable.",
        }
    }
}

fn map_tmux(error: tmux::TmuxError) -> AttachmentError {
    tracing::warn!(?error, "tmux attachment operation failed");
    if error == tmux::TmuxError::Incompatible {
        AttachmentError::TmuxIncompatible
    } else {
        AttachmentError::Tmux
    }
}

impl std::fmt::Display for AttachmentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ClientClosed => "Browser attachment closed",
            Self::Ssh => "target SSH failed",
            Self::Tmux | Self::TmuxIncompatible => "target tmux failed",
            Self::Protocol => "attachment protocol failed closed",
            Self::Unavailable => "attachment is unavailable",
            Self::RouteChanged => "Machine route changed",
        })
    }
}

impl std::error::Error for AttachmentError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_frame_contract_rejects_unknown_fields() {
        let valid = include_str!("../fixtures/attachment/auth.json");
        assert!(matches!(
            decode_auth_frame(Message::Text(valid.to_owned().into())).expect("auth fixture"),
            AuthFrame::ApiKey { .. }
        ));
        assert!(serde_json::from_str::<ClientFrame>(valid).is_err());
        let invalid = valid
            .trim_end()
            .replacen('}', ",\"machine_id\":\"hidden\"}", 1);
        assert!(decode_auth_frame(Message::Text(invalid.into())).is_err());
    }

    #[test]
    fn pane_input_requires_canonical_bounded_base64url() {
        let bytes = vec![0xff; ATTACHMENT_MAX_INPUT_BYTES];
        let encoded = URL_SAFE_NO_PAD.encode(&bytes);
        assert_eq!(decode_input(&encoded), Some(bytes));
        assert!(decode_input("").is_none());
        assert!(decode_input("_x").is_none());
        assert!(decode_input(&"YQ".repeat(700)).is_none());
    }

    #[test]
    fn writer_sizes_are_bounded_before_claim() {
        assert!(valid_client_size(ClientSize {
            columns: 100,
            rows: 30,
        }));
        assert!(!valid_client_size(ClientSize {
            columns: 0,
            rows: 30,
        }));
        assert!(!valid_client_size(ClientSize {
            columns: ATTACHMENT_MAX_DIMENSION,
            rows: ATTACHMENT_MAX_DIMENSION,
        }));
    }

    #[test]
    fn connection_epochs_are_canonical_and_exact() {
        assert!(matches_connection_epoch("42", 42));
        assert!(!matches_connection_epoch("042", 42));
        assert!(!matches_connection_epoch("42", 43));
        assert!(!matches_connection_epoch("0", 0));
    }
}
