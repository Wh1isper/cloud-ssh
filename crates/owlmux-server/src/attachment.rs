use std::{collections::HashSet, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    Router,
    extract::{
        ConnectInfo, Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse as _, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use tokio::{
    sync::OwnedSemaphorePermit,
    time::{Instant, sleep, timeout},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zeroize::Zeroize as _;

use crate::{
    generated::contracts::{ATTACHMENT_MAX_FRAME_BYTES, ATTACHMENT_MAX_TERMINAL_CHUNK_BYTES},
    service::{ServerState, SourcePermit},
    ssh, tmux,
};
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
const LOCAL_ROUTE_WAIT: Duration = Duration::from_secs(5);
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

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
        let _ = send_error(&mut socket, "unauthenticated", "Authentication failed.").await;
        return;
    }
    let Ok(_connection_permit) = state
        .attachment_connection_limit
        .clone()
        .try_acquire_owned()
    else {
        let _ = send_error(
            &mut socket,
            "overloaded",
            "Attachment capacity is exhausted.",
        )
        .await;
        return;
    };
    loop {
        let selection = match chooser(&mut socket, &state, machine_id).await {
            Ok(Some(selection)) => selection,
            Ok(None) => return,
            Err(AttachmentError::RouteChanged) => continue,
            Err(error) => {
                tracing::warn!(?error, %machine_id, "attachment chooser failed");
                let _ = send_error(&mut socket, error.code(), error.message()).await;
                return;
            }
        };
        match workspace(&mut socket, &state, machine_id, selection).await {
            Ok(WorkspaceExit::Chooser) | Err(AttachmentError::RouteChanged) => {}
            Ok(WorkspaceExit::Detach) => return,
            Err(error) => {
                tracing::warn!(?error, %machine_id, "attachment workspace failed");
                let _ = send_error(&mut socket, error.code(), error.message()).await;
                return;
            }
        }
    }
}

async fn authenticate(socket: &mut WebSocket, state: &ServerState) -> Result<(), ()> {
    let frame = receive(socket, AUTH_TIMEOUT).await.map_err(|_| ())?;
    let ClientFrame::AuthApiKey { mut api_key } = frame else {
        return Err(());
    };
    let valid = state.config.api_key().verify(&api_key);
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
}

async fn chooser(
    socket: &mut WebSocket,
    state: &Arc<ServerState>,
    machine_id: Uuid,
) -> Result<Option<Selection>, AttachmentError> {
    send(
        socket,
        &ServerFrame::Phase {
            phase: Phase::Connecting,
        },
    )
    .await?;
    let relay_stream = open_relay_stream(state, machine_id).await?;
    let route = relay_stream.route;
    let route_closed = relay_stream.closed;
    let probe = tokio::select! {
        () = route_closed.cancelled() => return Err(AttachmentError::RouteChanged),
        probe = ssh::run_tmux_probe(state, machine_id, relay_stream.stream) => {
            probe.map_err(|_| AttachmentError::Ssh)?
        }
    };
    let probe = tmux::parse_probe(&probe).map_err(map_tmux)?;
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
            selection_epoch: probe.selection_epoch,
            tmux_client_version: probe.tmux_client_version,
            tmux_server_version: probe.tmux_server_version,
            sessions: probe.sessions.clone(),
        },
    )
    .await?;
    loop {
        let frame = tokio::select! {
            () = route_closed.cancelled() => return Err(AttachmentError::RouteChanged),
            frame = receive(socket, Duration::from_mins(5)) => frame?,
        };
        match frame {
            ClientFrame::SessionSelect {
                selection_epoch,
                session_id,
                session_created,
            } => {
                if selection_epoch != probe.selection_epoch
                    || !probe.sessions.iter().any(|session| {
                        session.session_id == session_id
                            && session.session_created == session_created
                    })
                {
                    send_error(socket, "stale_selection", "Session selection is stale.").await?;
                    continue;
                }
                return Ok(Some(Selection {
                    session_id,
                    session_created,
                    route,
                    route_closed: route_closed.clone(),
                }));
            }
            ClientFrame::WorkspaceDetach => return Ok(None),
            _ => return Err(AttachmentError::Protocol),
        }
    }
}

async fn workspace(
    socket: &mut WebSocket,
    state: &Arc<ServerState>,
    machine_id: Uuid,
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
        ) => control.map_err(|_| AttachmentError::Ssh)?,
    };
    let mut control = tokio::select! {
        () = route_closed.cancelled() => return Err(AttachmentError::RouteChanged),
        control = tmux::ControlAdapter::start(control) => control.map_err(map_tmux)?,
    };
    let projection = tokio::select! {
        () = route_closed.cancelled() => return Err(AttachmentError::RouteChanged),
        projection = install_projection(
            socket,
            &mut control,
            &selection.session_id,
            selection.session_created,
        ) => projection?,
    };
    let Some((mut workspace_epoch, mut visible_panes)) = projection else {
        return Ok(WorkspaceExit::Chooser);
    };
    loop {
        tokio::select! {
            () = route_closed.cancelled() => return Err(AttachmentError::RouteChanged),
            client = receive(socket, Duration::from_mins(5)) => match client? {
                ClientFrame::WorkspaceReturnToChooser => return Ok(WorkspaceExit::Chooser),
                ClientFrame::WorkspaceDetach => return Ok(WorkspaceExit::Detach),
                _ => return Err(AttachmentError::Protocol),
            },
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
                        if visible_panes.contains(&pane_id) {
                            send(socket, &ServerFrame::Output {
                                workspace_epoch,
                                pane_id,
                                data_base64: URL_SAFE_NO_PAD.encode(data),
                            }).await?;
                        }
                    }
                    tmux::ControlEvent::Refresh => {
                        let projection = tokio::select! {
                            () = route_closed.cancelled() => {
                                return Err(AttachmentError::RouteChanged);
                            }
                            projection = install_projection(
                                socket,
                                &mut control,
                                &selection.session_id,
                                selection.session_created,
                            ) => projection?,
                        };
                        let Some((next_epoch, next_panes)) = projection else {
                            return Ok(WorkspaceExit::Chooser);
                        };
                        workspace_epoch = next_epoch;
                        visible_panes = next_panes;
                    }
                    tmux::ControlEvent::Exit => return Ok(WorkspaceExit::Chooser),
                }
            }
        }
    }
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

async fn install_projection(
    socket: &mut WebSocket,
    control: &mut tmux::ControlAdapter,
    session_id: &str,
    session_created: i64,
) -> Result<Option<(Uuid, HashSet<String>)>, AttachmentError> {
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
    let visible_panes = projection
        .panes
        .iter()
        .map(|pane| pane.pane_id.clone())
        .collect::<HashSet<_>>();
    send_with_control(
        socket,
        control,
        &ServerFrame::Projection {
            workspace_epoch,
            session_id: session_id.to_owned(),
            session_created,
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
    Ok(Some((workspace_epoch, visible_panes)))
}

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum ClientFrame {
    #[serde(rename = "auth.api_key")]
    AuthApiKey { api_key: String },
    #[serde(rename = "session.select")]
    SessionSelect {
        selection_epoch: Uuid,
        session_id: String,
        session_created: i64,
    },
    #[serde(rename = "workspace.return_to_chooser")]
    WorkspaceReturnToChooser,
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
        selection_epoch: Uuid,
        tmux_client_version: String,
        tmux_server_version: Option<String>,
        sessions: Vec<tmux::SessionSummary>,
    },
    #[serde(rename = "workspace.projection")]
    Projection {
        workspace_epoch: Uuid,
        session_id: String,
        session_created: i64,
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

async fn receive(
    socket: &mut WebSocket,
    deadline: Duration,
) -> Result<ClientFrame, AttachmentError> {
    let message = timeout(deadline, socket.next())
        .await
        .map_err(|_| AttachmentError::Protocol)?
        .ok_or(AttachmentError::Unavailable)?
        .map_err(|_| AttachmentError::Protocol)?;
    let Message::Text(text) = message else {
        return Err(AttachmentError::Protocol);
    };
    if text.len() > ATTACHMENT_MAX_FRAME_BYTES {
        return Err(AttachmentError::Protocol);
    }
    serde_json::from_str(&text).map_err(|_| AttachmentError::Protocol)
}

async fn send(socket: &mut WebSocket, frame: &ServerFrame) -> Result<(), AttachmentError> {
    let encoded = serde_json::to_string(frame).map_err(|_| AttachmentError::Protocol)?;
    if encoded.len() > ATTACHMENT_MAX_FRAME_BYTES {
        return Err(AttachmentError::Protocol);
    }
    timeout(
        CLIENT_WRITE_TIMEOUT,
        socket.send(Message::Text(encoded.into())),
    )
    .await
    .map_err(|_| AttachmentError::Unavailable)?
    .map_err(|_| AttachmentError::Unavailable)
}

async fn send_with_control(
    socket: &mut WebSocket,
    control: &mut tmux::ControlAdapter,
    frame: &ServerFrame,
) -> Result<(), AttachmentError> {
    let encoded = serde_json::to_string(frame).map_err(|_| AttachmentError::Protocol)?;
    if encoded.len() > ATTACHMENT_MAX_FRAME_BYTES {
        return Err(AttachmentError::Protocol);
    }
    let write = timeout(
        CLIENT_WRITE_TIMEOUT,
        socket.send(Message::Text(encoded.into())),
    );
    tokio::pin!(write);
    loop {
        tokio::select! {
            result = &mut write => {
                return result
                    .map_err(|_| AttachmentError::Unavailable)?
                    .map_err(|_| AttachmentError::Unavailable);
            }
            result = control.pump_event() => result.map_err(map_tmux)?,
        }
    }
}

async fn send_error(
    socket: &mut WebSocket,
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
    send(socket, &ServerFrame::Error { code, message }).await
}

#[derive(Clone, Copy, Debug)]
enum WorkspaceExit {
    Chooser,
    Detach,
}

#[derive(Debug)]
enum AttachmentError {
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
            Self::Ssh | Self::Tmux | Self::Unavailable | Self::RouteChanged => "target_unavailable",
        }
    }
    const fn message(&self) -> &'static str {
        match self {
            Self::TmuxIncompatible => "Target tmux is incompatible.",
            Self::Protocol => "Attachment protocol failed closed.",
            Self::Ssh | Self::Tmux | Self::Unavailable | Self::RouteChanged => {
                "Target is unavailable."
            }
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
        let valid = include_str!("../../../contracts/attachment/v1/fixtures/auth.json");
        assert!(matches!(
            serde_json::from_str::<ClientFrame>(valid).expect("auth fixture"),
            ClientFrame::AuthApiKey { .. }
        ));
        let invalid = valid
            .trim_end()
            .replacen('}', ",\"machine_id\":\"hidden\"}", 1);
        assert!(serde_json::from_str::<ClientFrame>(&invalid).is_err());
    }
}
