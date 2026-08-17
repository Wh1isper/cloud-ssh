pub mod protocol;

use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};

use axum::{
    Router,
    extract::{
        ConnectInfo, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::{IntoResponse as _, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use futures_util::StreamExt as _;
use rand_core::RngCore as _;
use sha2::{Digest as _, Sha256};
use sqlx::Row as _;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _, DuplexStream, ReadHalf, WriteHalf},
    sync::{Mutex, OwnedSemaphorePermit, RwLock, mpsc, oneshot},
    time::{Instant, interval, sleep_until, timeout},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    build,
    deployment::NodeLease,
    service::{ServerState, SourcePermit},
    ssh,
    storage::{append_audit, record_audit},
};
use protocol::{
    ChallengePurpose, ClientFrame, CloseReason, MAX_DATA_BYTES, MAX_FRAME_BYTES, MAX_STREAMS,
    ServerFrame, VERSION, signature_message,
};

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
const OWNER_CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);
const ENROLLMENT_ATTEMPT_SECONDS: i64 = 60;
const HEARTBEAT: Duration = Duration::from_secs(15);
const ENROLLMENT_PREFIX: &str = "owlmux_enroll_v1_";

type RelayResult<T> = Result<T, RelayError>;

pub fn router(state: Arc<ServerState>) -> Router<Arc<ServerState>> {
    Router::new()
        .route("/enroll", get(enroll_upgrade))
        .route("/tunnel", get(tunnel_upgrade))
        .with_state(state)
}

async fn enroll_upgrade(
    State(state): State<Arc<ServerState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Some(source_permit) = state.source_admission.try_acquire(peer.ip()) else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let Ok(attempt_permit) = state.preauth_attempt_limit.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok(permit) = state.relay_connection_limit.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    upgrade
        .max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            let fence = state.lease.fence_token();
            tokio::select! {
                result = enrollment(socket, state, attempt_permit, source_permit) => {
                    if let Err(error) = result {
                        tracing::warn!(reason = error.code(), "Relay enrollment closed");
                    }
                }
                () = fence.cancelled() => {}
            }
        })
}

async fn tunnel_upgrade(
    State(state): State<Arc<ServerState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Some(source_permit) = state.source_admission.try_acquire(peer.ip()) else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let Ok(attempt_permit) = state.preauth_attempt_limit.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok(permit) = state.relay_connection_limit.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    upgrade
        .max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            let fence = state.lease.fence_token();
            tokio::select! {
                result = tunnel(socket, state, attempt_permit, source_permit) => {
                    if let Err(error) = result {
                        tracing::warn!(reason = error.code(), "Relay tunnel closed");
                    }
                }
                () = fence.cancelled() => {}
            }
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnerExitState {
    Transitioning(RouteIdentity),
    Releasing(RouteIdentity),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnerReleaseDecision {
    Defer,
    Release,
}

#[derive(Default)]
struct RegistryState {
    tunnels: HashMap<Uuid, Arc<TunnelHandle>>,
    pending_claims: HashMap<(Uuid, Uuid), CancellationToken>,
    owner_exits: HashMap<Uuid, OwnerExitState>,
}

impl RegistryState {
    fn begin_transition(&mut self, machine_id: Uuid, route: RouteIdentity) -> bool {
        if self.owner_exits.contains_key(&machine_id) {
            return false;
        }
        self.owner_exits
            .insert(machine_id, OwnerExitState::Transitioning(route));
        true
    }

    fn begin_owner_release(
        &mut self,
        machine_id: Uuid,
        route: RouteIdentity,
    ) -> Option<OwnerReleaseDecision> {
        match self.owner_exits.get(&machine_id) {
            Some(OwnerExitState::Transitioning(expected)) if *expected == route => {
                Some(OwnerReleaseDecision::Defer)
            }
            Some(_) => None,
            None => {
                self.owner_exits
                    .insert(machine_id, OwnerExitState::Releasing(route));
                Some(OwnerReleaseDecision::Release)
            }
        }
    }

    fn finish_owner_release(&mut self, machine_id: Uuid, route: RouteIdentity) {
        if self.owner_exits.get(&machine_id) == Some(&OwnerExitState::Releasing(route)) {
            self.owner_exits.remove(&machine_id);
        }
    }
}

#[derive(Clone)]
pub struct RelayRegistry {
    state: Arc<RwLock<RegistryState>>,
    lease: Arc<NodeLease>,
}

pub(crate) struct MachineTransition {
    registry: RelayRegistry,
    machine_id: Uuid,
    route: RouteIdentity,
    armed: bool,
}

impl MachineTransition {
    pub(crate) async fn finish(
        mut self,
        state: &Arc<ServerState>,
        committed: bool,
    ) -> RelayResult<()> {
        if !committed
            && release_owner(
                state,
                self.machine_id,
                self.route.connection_id,
                self.route.connection_epoch,
            )
            .await
            .is_err()
        {
            self.armed = false;
            return Err(RelayError::Fenced);
        }
        self.registry
            .finish_machine_transition(self.machine_id, self.route)
            .await;
        self.armed = false;
        Ok(())
    }

    pub(crate) fn hard_fence(mut self) {
        self.registry.lease.hard_fence();
        self.armed = false;
    }
}

impl Drop for MachineTransition {
    fn drop(&mut self) {
        if self.armed {
            tracing::error!(machine_id = %self.machine_id, "Machine transition was cancelled; hard-fencing node");
            self.registry.lease.hard_fence();
        }
    }
}

impl RelayRegistry {
    #[must_use]
    pub fn new(lease: Arc<NodeLease>) -> Self {
        Self {
            state: Arc::new(RwLock::new(RegistryState::default())),
            lease,
        }
    }

    /// Open one bounded logical stream through the current owner-local Relay tunnel.
    ///
    /// # Errors
    ///
    /// Returns unavailable if there is no current local tunnel or its dispatch barrier is closed.
    pub async fn open_stream(&self, machine_id: Uuid) -> RelayResult<RelayStream> {
        self.lease.check().map_err(|_| RelayError::Fenced)?;
        let handle = self
            .state
            .read()
            .await
            .tunnels
            .get(&machine_id)
            .cloned()
            .ok_or(RelayError::Unavailable)?;
        handle.open_stream().await
    }

    pub async fn close_all(&self) {
        let handles = {
            let mut state = self.state.write().await;
            state
                .tunnels
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };
        for handle in handles {
            handle.close_dispatch().await;
        }
    }

    pub(crate) async fn begin_machine_transition(
        &self,
        machine_id: Uuid,
        expected_route: RouteIdentity,
    ) -> Option<MachineTransition> {
        let (transition, handle, pending_claims) = {
            let mut state = self.state.write().await;
            let handle = state.tunnels.get(&machine_id)?.clone();
            if handle.route != expected_route || !state.begin_transition(machine_id, expected_route)
            {
                return None;
            }
            let transition = MachineTransition {
                registry: self.clone(),
                machine_id,
                route: expected_route,
                armed: true,
            };
            state.tunnels.remove(&machine_id);
            let pending_claims = state
                .pending_claims
                .iter()
                .filter(|((pending_machine_id, _), _)| *pending_machine_id == machine_id)
                .map(|(_, completion)| completion.clone())
                .collect::<Vec<_>>();
            (transition, handle, pending_claims)
        };
        let cleanup = async {
            handle.close_dispatch().await;
            handle.cleanup_complete.cancelled().await;
            for completion in pending_claims {
                completion.cancelled().await;
            }
        };
        if timeout(OWNER_CLEANUP_TIMEOUT, cleanup).await.is_err() || self.lease.check().is_err() {
            tracing::error!(%machine_id, "Machine transition cleanup did not complete with valid authority");
            return None;
        }
        Some(transition)
    }

    async fn finish_machine_transition(&self, machine_id: Uuid, route: RouteIdentity) {
        let handle = {
            let mut state = self.state.write().await;
            let handle = state.tunnels.remove(&machine_id);
            if state.owner_exits.get(&machine_id) == Some(&OwnerExitState::Transitioning(route)) {
                state.owner_exits.remove(&machine_id);
            }
            handle
        };
        if let Some(handle) = handle {
            handle.close_dispatch().await;
        }
    }

    async fn begin_owner_release(
        &self,
        machine_id: Uuid,
        route: RouteIdentity,
    ) -> RelayResult<OwnerReleaseDecision> {
        self.state
            .write()
            .await
            .begin_owner_release(machine_id, route)
            .ok_or(RelayError::Fenced)
    }

    async fn finish_owner_release(
        &self,
        machine_id: Uuid,
        connection_id: Uuid,
        route: RouteIdentity,
    ) {
        let mut state = self.state.write().await;
        if state
            .tunnels
            .get(&machine_id)
            .is_some_and(|handle| handle.route == route && route.connection_id == connection_id)
        {
            state.tunnels.remove(&machine_id);
        }
        state.finish_owner_release(machine_id, route);
    }

    pub async fn is_connected(&self, machine_id: Uuid) -> bool {
        self.state
            .read()
            .await
            .tunnels
            .get(&machine_id)
            .is_some_and(|handle| !handle.barrier.is_cancelled())
    }

    pub(crate) async fn route_fence(
        &self,
        machine_id: Uuid,
        route: RouteIdentity,
    ) -> Option<RouteFence> {
        self.lease.check().ok()?;
        let state = self.state.read().await;
        let handle = state.tunnels.get(&machine_id)?;
        if handle.route != route || handle.barrier.is_cancelled() {
            return None;
        }
        Some(handle.route_fence())
    }

    pub async fn is_current_route(&self, machine_id: Uuid, route: RouteIdentity) -> bool {
        self.lease.is_ready()
            && self
                .state
                .read()
                .await
                .tunnels
                .get(&machine_id)
                .is_some_and(|handle| handle.route == route && !handle.barrier.is_cancelled())
    }

    async fn reserve_claim(&self, machine_id: Uuid, connection_id: Uuid) -> RelayResult<()> {
        if !self.lease.is_ready() {
            return Err(RelayError::Fenced);
        }
        let mut state = self.state.write().await;
        if !self.lease.is_ready() {
            return Err(RelayError::Fenced);
        }
        let identity = (machine_id, connection_id);
        if state.owner_exits.contains_key(&machine_id)
            || state.tunnels.contains_key(&machine_id)
            || state.pending_claims.contains_key(&identity)
        {
            return Err(RelayError::OwnerConflict);
        }
        state
            .pending_claims
            .insert(identity, CancellationToken::new());
        Ok(())
    }

    async fn install_reserved(
        &self,
        machine_id: Uuid,
        connection_id: Uuid,
        handle: Arc<TunnelHandle>,
    ) -> RelayResult<()> {
        if !self.lease.is_ready() {
            return Err(RelayError::Fenced);
        }
        let mut state = self.state.write().await;
        if !self.lease.is_ready() {
            return Err(RelayError::Fenced);
        }
        let identity = (machine_id, connection_id);
        if state.owner_exits.contains_key(&machine_id)
            || state.tunnels.contains_key(&machine_id)
            || !state.pending_claims.contains_key(&identity)
        {
            return Err(RelayError::OwnerConflict);
        }
        state.tunnels.insert(machine_id, handle);
        if let Some(completion) = state.pending_claims.remove(&identity) {
            completion.cancel();
        }
        Ok(())
    }

    async fn finish_pending(&self, machine_id: Uuid, connection_id: Uuid) {
        let completion = self
            .state
            .write()
            .await
            .pending_claims
            .remove(&(machine_id, connection_id));
        if let Some(completion) = completion {
            completion.cancel();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RouteIdentity {
    pub route_revision: i64,
    pub connection_epoch: i64,
    pub connection_id: Uuid,
}

#[derive(Clone)]
pub(crate) struct RouteFence {
    dispatch_open: Arc<Mutex<bool>>,
    barrier: CancellationToken,
    lease: Arc<NodeLease>,
}

impl RouteFence {
    pub(crate) async fn dispatch<T>(&self, work: impl Future<Output = T>) -> RelayResult<T> {
        let dispatch_open = self.dispatch_open.lock().await;
        self.lease.check().map_err(|_| RelayError::Fenced)?;
        if !*dispatch_open || self.barrier.is_cancelled() {
            return Err(RelayError::Unavailable);
        }
        Ok(work.await)
    }
}

pub struct RelayStream {
    pub route: RouteIdentity,
    pub closed: CancellationToken,
    pub stream: DuplexStream,
}

struct TunnelHandle {
    route: RouteIdentity,
    command_tx: mpsc::Sender<TunnelCommand>,
    dispatch_open: Arc<Mutex<bool>>,
    barrier: CancellationToken,
    cleanup_complete: CancellationToken,
    lease: Arc<NodeLease>,
}

impl TunnelHandle {
    fn new(
        route: RouteIdentity,
        lease: Arc<NodeLease>,
    ) -> (Arc<Self>, mpsc::Receiver<TunnelCommand>) {
        let (command_tx, command_rx) = mpsc::channel(32);
        let handle = Arc::new(Self {
            route,
            command_tx,
            dispatch_open: Arc::new(Mutex::new(true)),
            barrier: CancellationToken::new(),
            cleanup_complete: CancellationToken::new(),
            lease,
        });
        (handle, command_rx)
    }

    async fn close_dispatch(&self) {
        let mut dispatch_open = self.dispatch_open.lock().await;
        *dispatch_open = false;
        self.barrier.cancel();
    }

    fn route_fence(&self) -> RouteFence {
        RouteFence {
            dispatch_open: Arc::clone(&self.dispatch_open),
            barrier: self.barrier.clone(),
            lease: Arc::clone(&self.lease),
        }
    }

    async fn open_stream(&self) -> RelayResult<RelayStream> {
        self.lease.check().map_err(|_| RelayError::Fenced)?;
        if self.barrier.is_cancelled() {
            return Err(RelayError::Unavailable);
        }
        let (response_tx, response_rx) = oneshot::channel();
        tokio::select! {
            biased;
            () = self.barrier.cancelled() => return Err(RelayError::Unavailable),
            result = timeout(
                SEND_TIMEOUT,
                self.command_tx.send(TunnelCommand::Open {
                    response: response_tx,
                }),
            ) => result
                .map_err(|_| RelayError::Unavailable)?
                .map_err(|_| RelayError::Unavailable)?,
        }
        let stream = tokio::select! {
            biased;
            () = self.barrier.cancelled() => return Err(RelayError::Unavailable),
            result = timeout(SEND_TIMEOUT, response_rx) => result
                .map_err(|_| RelayError::Unavailable)?
                .map_err(|_| RelayError::Unavailable)??,
        };
        self.lease.check().map_err(|_| RelayError::Fenced)?;
        if self.barrier.is_cancelled() {
            return Err(RelayError::Unavailable);
        }
        Ok(RelayStream {
            route: self.route,
            closed: self.barrier.clone(),
            stream,
        })
    }
}

enum TunnelCommand {
    Open {
        response: oneshot::Sender<RelayResult<DuplexStream>>,
    },
}

struct AcceptedEnrollment {
    deployment_id: Uuid,
    machine_id: Uuid,
    attempt_id: Uuid,
    route_revision: i64,
    target_account: String,
}

async fn enrollment(
    mut socket: WebSocket,
    state: Arc<ServerState>,
    attempt_permit: OwnedSemaphorePermit,
    source_permit: SourcePermit,
) -> RelayResult<()> {
    state.lease.check().map_err(|_| RelayError::Fenced)?;
    let ClientFrame::Token { token } = receive_frame(&mut socket, FIRST_FRAME_TIMEOUT).await?
    else {
        return send_error(&mut socket, RelayError::Protocol).await;
    };
    let token = Zeroizing::new(token);
    let accepted = accept_token(&state, &token).await?;
    drop(source_permit);
    drop(attempt_permit);
    let result = continue_enrollment(socket, &state, &accepted).await;
    if result.is_err() {
        fail_attempt(&state, &accepted).await;
    }
    result
}

async fn continue_enrollment(
    mut socket: WebSocket,
    state: &Arc<ServerState>,
    accepted: &AcceptedEnrollment,
) -> RelayResult<()> {
    send_frame(
        &mut socket,
        &ServerFrame::Accepted {
            deployment_id: accepted.deployment_id,
            machine_id: accepted.machine_id,
            attempt_id: accepted.attempt_id,
            route_revision: accepted.route_revision,
        },
    )
    .await?;

    let (relay_id, public_key) = match receive_frame(&mut socket, FIRST_FRAME_TIMEOUT).await? {
        ClientFrame::Setup {
            protocol,
            relay_id,
            public_key,
            endpoint,
            observed_account,
        } if protocol == VERSION
            && endpoint == "127.0.0.1:22"
            && observed_account == accepted.target_account =>
        {
            let key = decode_32(&public_key)?;
            (relay_id, key)
        }
        _ => return send_error(&mut socket, RelayError::InvalidState).await,
    };

    let credential = enrollment_credential(state, accepted).await?;
    send_frame(&mut socket, &credential).await?;
    if !matches!(
        receive_frame(&mut socket, FIRST_FRAME_TIMEOUT).await?,
        ClientFrame::Ready
    ) {
        return send_error(&mut socket, RelayError::Protocol).await;
    }

    let nonce = random_32();
    send_frame(
        &mut socket,
        &ServerFrame::Challenge {
            purpose: ChallengePurpose::Enrollment,
            nonce: URL_SAFE_NO_PAD.encode(nonce),
        },
    )
    .await?;
    let signature = match receive_frame(&mut socket, FIRST_FRAME_TIMEOUT).await? {
        ClientFrame::Signature { signature } => decode_signature(&signature)?,
        _ => return send_error(&mut socket, RelayError::Protocol).await,
    };
    verify_signature(
        public_key,
        signature,
        &signature_message(
            ChallengePurpose::Enrollment,
            accepted.deployment_id,
            accepted.machine_id,
            relay_id,
            None,
            accepted.route_revision,
            &nonce,
        ),
    )?;

    send_frame(&mut socket, &ServerFrame::OpenStream { stream_id: 1 }).await?;
    if !matches!(
        receive_frame(&mut socket, FIRST_FRAME_TIMEOUT).await?,
        ClientFrame::StreamOpened { stream_id: 1 }
    ) {
        return send_error(&mut socket, RelayError::ProofFailed).await;
    }
    let (ssh_stream, transport_stream) = tokio::io::duplex(64 * 1024);
    let proof = run_provisional_stream(
        &mut socket,
        transport_stream,
        ssh::verify_access(state, accepted.machine_id, ssh_stream),
    )
    .await;
    let _ = timeout(
        Duration::from_secs(1),
        record_audit(
            state.database.ordinary(),
            state.database.deployment_id(),
            "machine",
            Some(accepted.machine_id),
            None,
            "ssh_verify_access",
            if proof.is_ok() { "success" } else { "rejected" },
        ),
    )
    .await;
    proof?;
    activate(state, accepted, relay_id, public_key).await?;
    send_frame(
        &mut socket,
        &ServerFrame::Activated {
            route_revision: accepted.route_revision,
        },
    )
    .await?;
    finish_enrollment(&mut socket).await;
    Ok(())
}

async fn enrollment_credential(
    state: &ServerState,
    accepted: &AcceptedEnrollment,
) -> RelayResult<ServerFrame> {
    let row = sqlx::query(
        "SELECT c.id, c.name, c.public_key, c.public_fingerprint_sha256 FROM machines m JOIN ssh_credentials c ON c.id = m.ssh_credential_id WHERE m.id = $1 AND m.lifecycle = 'verifying' AND m.route_revision = $2 AND c.status = 'active'",
    )
    .bind(accepted.machine_id)
    .bind(accepted.route_revision)
    .fetch_optional(state.database.ordinary())
    .await
    .map_err(|_| RelayError::Unavailable)?
    .ok_or(RelayError::InvalidState)?;
    state.lease.check().map_err(|_| RelayError::Fenced)?;
    Ok(ServerFrame::Credential {
        credential_id: row.try_get("id").map_err(|_| RelayError::Unavailable)?,
        name: row.try_get("name").map_err(|_| RelayError::Unavailable)?,
        public_key: row
            .try_get("public_key")
            .map_err(|_| RelayError::Unavailable)?,
        public_fingerprint_sha256: row
            .try_get("public_fingerprint_sha256")
            .map_err(|_| RelayError::Unavailable)?,
    })
}

async fn finish_enrollment(socket: &mut WebSocket) {
    let _ = timeout(FIRST_FRAME_TIMEOUT, async {
        loop {
            match socket.recv().await {
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    let _ = timeout(SEND_TIMEOUT, socket.send(Message::Close(None))).await;
}

async fn run_provisional_stream<F>(
    socket: &mut WebSocket,
    transport: DuplexStream,
    proof: F,
) -> RelayResult<()>
where
    F: std::future::Future<Output = Result<(), ssh::SshError>>,
{
    let (mut reader, mut writer) = tokio::io::split(transport);
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<ServerFrame>(16);
    let reader_task = tokio::spawn(async move {
        relay_reader(1, &mut reader, outbound_tx).await;
    });
    tokio::pin!(proof);
    let result = loop {
        tokio::select! {
            proof_result = &mut proof => break proof_result.map_err(|_| RelayError::ProofFailed),
            Some(frame) = outbound_rx.recv() => send_frame(socket, &frame).await?,
            frame = receive_frame(socket, Duration::from_secs(20)) => match frame? {
                ClientFrame::StreamData { stream_id: 1, data } => write_data(&mut writer, &data).await?,
                ClientFrame::StreamHalfClosed { stream_id: 1 } | ClientFrame::StreamClosed { stream_id: 1, .. } => {
                    writer.shutdown().await.map_err(|_| RelayError::Unavailable)?;
                }
                _ => break Err(RelayError::Protocol),
            }
        }
    };
    reader_task.abort();
    result
}

async fn tunnel(
    mut socket: WebSocket,
    state: Arc<ServerState>,
    attempt_permit: OwnedSemaphorePermit,
    source_permit: SourcePermit,
) -> RelayResult<()> {
    state.lease.check().map_err(|_| RelayError::Fenced)?;
    let hello = match receive_frame(&mut socket, FIRST_FRAME_TIMEOUT).await? {
        ClientFrame::TunnelHello {
            protocol,
            deployment_id,
            machine_id,
            relay_id,
            connection_id,
            route_revision,
        } if protocol == VERSION => (
            deployment_id,
            machine_id,
            relay_id,
            connection_id,
            route_revision,
        ),
        _ => return send_error(&mut socket, RelayError::Protocol).await,
    };
    let public_key = binding_key(&state, hello.1, hello.2, hello.4).await?;
    if hello.0 != state.database.deployment_id() {
        return send_error(&mut socket, RelayError::InvalidState).await;
    }
    let nonce = random_32();
    send_frame(
        &mut socket,
        &ServerFrame::Challenge {
            purpose: ChallengePurpose::Tunnel,
            nonce: URL_SAFE_NO_PAD.encode(nonce),
        },
    )
    .await?;
    let signature = match receive_frame(&mut socket, FIRST_FRAME_TIMEOUT).await? {
        ClientFrame::Signature { signature } => decode_signature(&signature)?,
        _ => return send_error(&mut socket, RelayError::Protocol).await,
    };
    verify_signature(
        public_key,
        signature,
        &signature_message(
            ChallengePurpose::Tunnel,
            hello.0,
            hello.1,
            hello.2,
            Some(hello.3),
            hello.4,
            &nonce,
        ),
    )?;
    drop((source_permit, attempt_permit));
    let connection_epoch = claim_reserved_owner(&state, hello.1, hello.3, hello.4).await?;
    let (handle, command_rx) = TunnelHandle::new(
        RouteIdentity {
            route_revision: hello.4,
            connection_epoch,
            connection_id: hello.3,
        },
        state.lease.clone(),
    );
    if let Err(error) = state
        .relays
        .install_reserved(hello.1, hello.3, handle.clone())
        .await
    {
        handle.close_dispatch().await;
        let release_result = release_owner(&state, hello.1, hello.3, connection_epoch).await;
        state.relays.finish_pending(hello.1, hello.3).await;
        release_result?;
        return Err(error);
    }
    let result = match send_frame(
        &mut socket,
        &ServerFrame::TunnelEstablished { connection_epoch },
    )
    .await
    {
        Ok(()) => {
            run_tunnel(
                &mut socket,
                command_rx,
                handle.dispatch_open.clone(),
                handle.barrier.clone(),
                state.lease.clone(),
            )
            .await
        }
        Err(error) => Err(error),
    };
    handle.close_dispatch().await;
    complete_tunnel_owner_cleanup(&state, hello.1, &handle).await?;
    result
}

async fn complete_tunnel_owner_cleanup(
    state: &Arc<ServerState>,
    machine_id: Uuid,
    handle: &TunnelHandle,
) -> RelayResult<()> {
    let decision = state
        .relays
        .begin_owner_release(machine_id, handle.route)
        .await?;
    let release_result = match decision {
        OwnerReleaseDecision::Defer => Ok(()),
        OwnerReleaseDecision::Release => {
            release_owner(
                state,
                machine_id,
                handle.route.connection_id,
                handle.route.connection_epoch,
            )
            .await
        }
    };
    handle.cleanup_complete.cancel();
    match decision {
        OwnerReleaseDecision::Defer => {}
        OwnerReleaseDecision::Release => {
            state
                .relays
                .finish_owner_release(machine_id, handle.route.connection_id, handle.route)
                .await;
        }
    }
    release_result
}

async fn claim_reserved_owner(
    state: &Arc<ServerState>,
    machine_id: Uuid,
    connection_id: Uuid,
    route_revision: i64,
) -> RelayResult<i64> {
    state
        .relays
        .reserve_claim(machine_id, connection_id)
        .await?;
    match claim_owner(state, machine_id, connection_id, route_revision).await {
        Ok(connection_epoch) => Ok(connection_epoch),
        Err(error) => {
            state.relays.finish_pending(machine_id, connection_id).await;
            Err(error)
        }
    }
}

async fn run_tunnel(
    socket: &mut WebSocket,
    mut commands: mpsc::Receiver<TunnelCommand>,
    dispatch_open: Arc<Mutex<bool>>,
    barrier: CancellationToken,
    lease: Arc<NodeLease>,
) -> RelayResult<()> {
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<ServerFrame>(64);
    let mut writers = HashMap::<u32, WriteHalf<DuplexStream>>::new();
    let mut next_stream_id = 1_u32;
    let mut heartbeat = interval(HEARTBEAT);
    let mut ping_nonce = 0_u64;
    let mut outstanding_pings = VecDeque::with_capacity(3);
    let mut inbound_deadline = Instant::now() + HEARTBEAT * 3;
    let fence = lease.fence_token();
    loop {
        lease.check().map_err(|_| RelayError::Fenced)?;
        tokio::select! {
            biased;
            () = barrier.cancelled() => return Ok(()),
            () = fence.cancelled() => return Err(RelayError::Fenced),
            Some(command) = commands.recv() => match command {
                TunnelCommand::Open { response } => {
                    let dispatch_open = dispatch_open.lock().await;
                    lease.check().map_err(|_| RelayError::Fenced)?;
                    if !*dispatch_open || barrier.is_cancelled() {
                        let _ = response.send(Err(RelayError::Unavailable));
                        continue;
                    }
                    if writers.len() >= MAX_STREAMS {
                        let _ = response.send(Err(RelayError::Overloaded));
                        continue;
                    }
                    let stream_id = next_stream_id;
                    next_stream_id = next_stream_id.checked_add(1).ok_or(RelayError::Protocol)?;
                    let (caller, transport) = tokio::io::duplex(64 * 1024);
                    let (mut reader, writer) = tokio::io::split(transport);
                    writers.insert(stream_id, writer);
                    let tx = outbound_tx.clone();
                    tokio::spawn(async move { relay_reader(stream_id, &mut reader, tx).await; });
                    send_frame(socket, &ServerFrame::OpenStream { stream_id }).await?;
                    let _ = response.send(Ok(caller));
                }
            },
            Some(frame) = outbound_rx.recv() => {
                send_route_frame(socket, &frame, &dispatch_open, &barrier, &lease).await?;
            }
            () = sleep_until(inbound_deadline) => return Err(RelayError::Unavailable),
            message = socket.next() => {
                let message = message
                    .ok_or(RelayError::Unavailable)?
                    .map_err(|_| RelayError::Protocol)?;
                let frame = decode_frame(message)?;
                inbound_deadline = Instant::now() + HEARTBEAT * 3;
                match frame {
                    ClientFrame::StreamOpened { .. } => {}
                    ClientFrame::Pong { nonce } => {
                        let Some(position) = outstanding_pings.iter().position(|sent| *sent == nonce) else {
                            return Err(RelayError::Protocol);
                        };
                        outstanding_pings.remove(position);
                    }
                    ClientFrame::StreamData { stream_id, data } => {
                        lease.check().map_err(|_| RelayError::Fenced)?;
                        let writer = writers.get_mut(&stream_id).ok_or(RelayError::Protocol)?;
                        write_data(writer, &data).await?;
                    }
                    ClientFrame::StreamHalfClosed { stream_id } | ClientFrame::StreamClosed { stream_id, .. } => {
                        if let Some(mut writer) = writers.remove(&stream_id) {
                            let _ = writer.shutdown().await;
                        }
                    }
                    _ => return Err(RelayError::Protocol),
                }
            },
            _ = heartbeat.tick() => {
                if outstanding_pings.len() >= 3 {
                    return Err(RelayError::Unavailable);
                }
                ping_nonce = ping_nonce.checked_add(1).ok_or(RelayError::Protocol)?;
                send_route_frame(
                    socket,
                    &ServerFrame::Ping { nonce: ping_nonce },
                    &dispatch_open,
                    &barrier,
                    &lease,
                )
                .await?;
                outstanding_pings.push_back(ping_nonce);
            }
        }
    }
}

async fn relay_reader(
    stream_id: u32,
    reader: &mut ReadHalf<DuplexStream>,
    outbound: mpsc::Sender<ServerFrame>,
) {
    let mut buffer = vec![0_u8; MAX_DATA_BYTES];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => {
                let _ = outbound
                    .send(ServerFrame::StreamHalfClosed { stream_id })
                    .await;
                return;
            }
            Ok(size) => {
                if outbound
                    .send(ServerFrame::StreamData {
                        stream_id,
                        data: URL_SAFE_NO_PAD.encode(&buffer[..size]),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(_) => {
                let _ = outbound
                    .send(ServerFrame::StreamClosed {
                        stream_id,
                        reason: CloseReason::Shutdown,
                    })
                    .await;
                return;
            }
        }
    }
}

async fn accept_token(state: &Arc<ServerState>, token: &str) -> RelayResult<AcceptedEnrollment> {
    if !token.starts_with(ENROLLMENT_PREFIX) || token.len() > 80 {
        return Err(RelayError::InvalidToken);
    }
    let mut hasher = Sha256::new();
    hasher.update(b"owlmux:relay-enrollment-token:v1\0");
    hasher.update(token.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let mut transaction = state
        .database
        .ordinary()
        .begin()
        .await
        .map_err(|_| RelayError::Unavailable)?;
    lock_deployment(&mut transaction, state).await?;
    let row = sqlx::query(
        "SELECT e.id AS enrollment_id, m.id AS machine_id, m.route_revision, m.target_account FROM relay_enrollments e JOIN machines m ON m.id = e.machine_id WHERE e.token_digest = $1 AND e.status = 'issued' AND e.token_expires_at > clock_timestamp() AND m.lifecycle = 'pending' FOR UPDATE OF e, m",
    )
    .bind(digest.as_slice())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| RelayError::Unavailable)?
    .ok_or(RelayError::InvalidToken)?;
    validate_node(&mut transaction, state).await?;
    let enrollment_id: Uuid = row
        .try_get("enrollment_id")
        .map_err(|_| RelayError::Unavailable)?;
    let machine_id: Uuid = row
        .try_get("machine_id")
        .map_err(|_| RelayError::Unavailable)?;
    let route_revision: i64 = row
        .try_get("route_revision")
        .map_err(|_| RelayError::Unavailable)?;
    let target_account: String = row
        .try_get("target_account")
        .map_err(|_| RelayError::Unavailable)?;
    let attempt_id = Uuid::new_v4();
    sqlx::query("UPDATE relay_enrollments SET status = 'consumed', consumed_at = clock_timestamp() WHERE id = $1")
        .bind(enrollment_id).execute(&mut *transaction).await.map_err(|_| RelayError::Unavailable)?;
    sqlx::query(
        "UPDATE machines SET lifecycle = 'verifying', updated_at = clock_timestamp() WHERE id = $1",
    )
    .bind(machine_id)
    .execute(&mut *transaction)
    .await
    .map_err(|_| RelayError::Unavailable)?;
    sqlx::query("INSERT INTO relay_verification_attempts (id, machine_id, enrollment_id, executing_incarnation_id, route_revision, status, deadline) VALUES ($1,$2,$3,$4,$5,'verifying',clock_timestamp() + $6 * interval '1 second')")
        .bind(attempt_id).bind(machine_id).bind(enrollment_id).bind(state.lease.incarnation_id()).bind(route_revision).bind(ENROLLMENT_ATTEMPT_SECONDS)
        .execute(&mut *transaction).await.map_err(|_| RelayError::Unavailable)?;
    append_audit(
        &mut transaction,
        state.database.deployment_id(),
        "enrollment",
        Some(machine_id),
        None,
        "accept_token",
    )
    .await
    .map_err(|_| RelayError::Unavailable)?;
    transaction
        .commit()
        .await
        .map_err(|_| RelayError::Ambiguous)?;
    Ok(AcceptedEnrollment {
        deployment_id: state.database.deployment_id(),
        machine_id,
        attempt_id,
        route_revision,
        target_account,
    })
}

async fn activate(
    state: &Arc<ServerState>,
    accepted: &AcceptedEnrollment,
    relay_id: Uuid,
    public_key: [u8; 32],
) -> RelayResult<()> {
    let mut transaction = state
        .database
        .ordinary()
        .begin()
        .await
        .map_err(|_| RelayError::Unavailable)?;
    lock_deployment(&mut transaction, state).await?;
    validate_node(&mut transaction, state).await?;
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM relay_verification_attempts a JOIN machines m ON m.id = a.machine_id JOIN ssh_credentials c ON c.id = m.ssh_credential_id WHERE a.id = $1 AND a.machine_id = $2 AND a.status = 'verifying' AND a.deadline > clock_timestamp() AND a.executing_incarnation_id = $3 AND a.route_revision = m.route_revision AND m.lifecycle = 'verifying' AND c.status = 'active' FOR UPDATE OF a, m, c)",
    )
    .bind(accepted.attempt_id).bind(accepted.machine_id).bind(state.lease.incarnation_id())
    .fetch_one(&mut *transaction).await.map_err(|_| RelayError::Unavailable)?;
    if !valid {
        return Err(RelayError::InvalidState);
    }
    sqlx::query("INSERT INTO relay_bindings (id, machine_id, relay_id, relay_public_key, route_revision, status) VALUES ($1,$2,$3,$4,$5,'active')")
        .bind(Uuid::new_v4()).bind(accepted.machine_id).bind(relay_id).bind(public_key.as_slice()).bind(accepted.route_revision)
        .execute(&mut *transaction).await.map_err(|error| if error.as_database_error().is_some() { RelayError::Conflict } else { RelayError::Unavailable })?;
    sqlx::query("UPDATE relay_verification_attempts SET status = 'activated', completed_at = clock_timestamp() WHERE id = $1")
        .bind(accepted.attempt_id).execute(&mut *transaction).await.map_err(|_| RelayError::Unavailable)?;
    append_audit(
        &mut transaction,
        state.database.deployment_id(),
        "relay_binding",
        Some(accepted.machine_id),
        None,
        "activate",
    )
    .await
    .map_err(|_| RelayError::Unavailable)?;
    transaction
        .commit()
        .await
        .map_err(|_| RelayError::Ambiguous)
}

/// Recover a bounded batch of expired pre-activation verification attempts.
///
/// # Errors
///
/// Returns a bounded authority or database error without reopening an activated binding.
pub async fn recover_expired_attempts(
    state: &Arc<ServerState>,
    machine_id: Option<Uuid>,
) -> RelayResult<()> {
    let mut transaction = state
        .database
        .ordinary()
        .begin()
        .await
        .map_err(|_| RelayError::Unavailable)?;
    lock_deployment(&mut transaction, state).await?;
    validate_node(&mut transaction, state).await?;
    let rows = if let Some(machine_id) = machine_id {
        sqlx::query("SELECT id, machine_id, route_revision FROM relay_verification_attempts WHERE machine_id = $1 AND status = 'verifying' AND deadline <= clock_timestamp() ORDER BY deadline LIMIT 128 FOR UPDATE")
            .bind(machine_id)
            .fetch_all(&mut *transaction)
            .await
    } else {
        sqlx::query("SELECT id, machine_id, route_revision FROM relay_verification_attempts WHERE status = 'verifying' AND deadline <= clock_timestamp() ORDER BY deadline LIMIT 128 FOR UPDATE")
            .fetch_all(&mut *transaction)
            .await
    }
    .map_err(|_| RelayError::Unavailable)?;
    for row in rows {
        let attempt_id: Uuid = row.try_get("id").map_err(|_| RelayError::Unavailable)?;
        let machine_id: Uuid = row
            .try_get("machine_id")
            .map_err(|_| RelayError::Unavailable)?;
        let route_revision: i64 = row
            .try_get("route_revision")
            .map_err(|_| RelayError::Unavailable)?;
        let changed = sqlx::query("UPDATE relay_verification_attempts SET status = 'failed', completed_at = clock_timestamp() WHERE id = $1 AND status = 'verifying'")
            .bind(attempt_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RelayError::Unavailable)?
            .rows_affected();
        if changed == 1 {
            sqlx::query("UPDATE machines SET lifecycle = 'pending', updated_at = clock_timestamp() WHERE id = $1 AND lifecycle = 'verifying' AND route_revision = $2 AND NOT EXISTS (SELECT 1 FROM relay_bindings WHERE machine_id = $1 AND route_revision = $2 AND status = 'active')")
                .bind(machine_id)
                .bind(route_revision)
                .execute(&mut *transaction)
                .await
                .map_err(|_| RelayError::Unavailable)?;
            append_audit(
                &mut transaction,
                state.database.deployment_id(),
                "enrollment",
                Some(machine_id),
                None,
                "recover_expired_attempt",
            )
            .await
            .map_err(|_| RelayError::Unavailable)?;
        }
    }
    transaction
        .commit()
        .await
        .map_err(|_| RelayError::Ambiguous)
}

async fn fail_attempt(state: &Arc<ServerState>, accepted: &AcceptedEnrollment) {
    let Ok(mut transaction) = state.database.ordinary().begin().await else {
        return;
    };
    if lock_deployment(&mut transaction, state).await.is_err()
        || validate_node(&mut transaction, state).await.is_err()
    {
        return;
    }
    let changed = sqlx::query("UPDATE relay_verification_attempts SET status = 'failed', completed_at = clock_timestamp() WHERE id = $1 AND status = 'verifying'")
        .bind(accepted.attempt_id).execute(&mut *transaction).await.map_or(0, |result| result.rows_affected());
    if changed == 1 {
        let _ = sqlx::query("UPDATE machines SET lifecycle = 'pending', updated_at = clock_timestamp() WHERE id = $1 AND lifecycle = 'verifying' AND route_revision = $2")
            .bind(accepted.machine_id).bind(accepted.route_revision).execute(&mut *transaction).await;
    }
    let _ = transaction.commit().await;
}

async fn binding_key(
    state: &Arc<ServerState>,
    machine_id: Uuid,
    relay_id: Uuid,
    route_revision: i64,
) -> RelayResult<[u8; 32]> {
    let bytes: Vec<u8> = sqlx::query_scalar("SELECT relay_public_key FROM relay_bindings WHERE machine_id = $1 AND relay_id = $2 AND route_revision = $3 AND status = 'active'")
        .bind(machine_id).bind(relay_id).bind(route_revision).fetch_optional(state.database.ordinary()).await.map_err(|_| RelayError::Unavailable)?.ok_or(RelayError::InvalidState)?;
    bytes.try_into().map_err(|_| RelayError::InvalidState)
}

async fn claim_owner(
    state: &Arc<ServerState>,
    machine_id: Uuid,
    connection_id: Uuid,
    route_revision: i64,
) -> RelayResult<i64> {
    let mut transaction = state
        .database
        .critical()
        .begin()
        .await
        .map_err(|_| RelayError::Unavailable)?;
    lock_deployment(&mut transaction, state).await?;
    validate_node(&mut transaction, state).await?;
    let row = sqlx::query(
        "SELECT o.connection_epoch, o.owner_incarnation_id, m.lifecycle, EXISTS(SELECT 1 FROM server_nodes n WHERE n.incarnation_id = o.owner_incarnation_id AND n.state IN ('serving', 'draining') AND n.lease_until > clock_timestamp()) AS owner_valid FROM machine_owners o JOIN machines m ON m.id = o.machine_id JOIN relay_bindings b ON b.machine_id = m.id WHERE o.machine_id = $1 AND o.route_revision = $2 AND m.lifecycle IN ('verifying', 'active') AND m.route_revision = $2 AND b.status = 'active' AND b.route_revision = $2 FOR UPDATE OF o, m, b",
    )
    .bind(machine_id).bind(route_revision).fetch_optional(&mut *transaction).await.map_err(|_| RelayError::Unavailable)?.ok_or(RelayError::InvalidState)?;
    let owner_valid: bool = row
        .try_get("owner_valid")
        .map_err(|_| RelayError::Unavailable)?;
    if owner_valid {
        return Err(RelayError::OwnerConflict);
    }
    let current_epoch: i64 = row
        .try_get("connection_epoch")
        .map_err(|_| RelayError::Unavailable)?;
    let connection_epoch = current_epoch
        .checked_add(1)
        .ok_or(RelayError::InvalidState)?;
    sqlx::query("UPDATE machine_owners SET connection_epoch = $1, owner_incarnation_id = $2, relay_connection_id = $3, claimed_at = clock_timestamp() WHERE machine_id = $4")
        .bind(connection_epoch).bind(state.lease.incarnation_id()).bind(connection_id).bind(machine_id).execute(&mut *transaction).await.map_err(|_| RelayError::Unavailable)?;
    sqlx::query("UPDATE machines SET lifecycle = 'active', updated_at = clock_timestamp() WHERE id = $1 AND lifecycle = 'verifying' AND route_revision = $2")
        .bind(machine_id).bind(route_revision).execute(&mut *transaction).await.map_err(|_| RelayError::Unavailable)?;
    append_audit(
        &mut transaction,
        state.database.deployment_id(),
        "machine_owner",
        Some(machine_id),
        None,
        "claim",
    )
    .await
    .map_err(|_| RelayError::Unavailable)?;
    if transaction.commit().await.is_err() {
        state.lease.hard_fence();
        tracing::error!(%machine_id, %connection_id, connection_epoch, "owner claim commit is ambiguous; hard-fenced node");
        let _ = timeout(
            Duration::from_secs(1),
            record_audit(
                state.database.ordinary(),
                state.database.deployment_id(),
                "machine_owner",
                Some(machine_id),
                None,
                "claim_observation",
                "ambiguous",
            ),
        )
        .await;
        return Err(RelayError::Ambiguous);
    }
    Ok(connection_epoch)
}

async fn release_owner(
    state: &Arc<ServerState>,
    machine_id: Uuid,
    connection_id: Uuid,
    connection_epoch: i64,
) -> RelayResult<()> {
    let result = async {
        let mut transaction = state
            .database
            .critical()
            .begin()
            .await
            .map_err(|_| RelayError::Unavailable)?;
        lock_deployment(&mut transaction, state).await?;
        validate_releasing_node(&mut transaction, state).await?;
        let changed = sqlx::query("UPDATE machine_owners SET owner_incarnation_id = NULL, relay_connection_id = NULL, claimed_at = NULL WHERE machine_id = $1 AND owner_incarnation_id = $2 AND relay_connection_id = $3 AND connection_epoch = $4")
            .bind(machine_id)
            .bind(state.lease.incarnation_id())
            .bind(connection_id)
            .bind(connection_epoch)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RelayError::Unavailable)?
            .rows_affected();
        if changed != 1 {
            return Err(RelayError::Fenced);
        }
        append_audit(
            &mut transaction,
            state.database.deployment_id(),
            "machine_owner",
            Some(machine_id),
            None,
            "release",
        )
        .await
        .map_err(|_| RelayError::Unavailable)?;
        transaction
            .commit()
            .await
            .map_err(|_| RelayError::Ambiguous)
    }
    .await;
    if let Err(error) = result {
        tracing::error!(%machine_id, %connection_id, connection_epoch, ?error, "exact owner release failed; hard-fencing node");
        state.lease.hard_fence();
        return Err(error);
    }
    Ok(())
}

async fn lock_deployment(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: &ServerState,
) -> RelayResult<()> {
    let config_proof = state
        .config
        .configuration_proof(state.database.deployment_id())
        .map(Vec::from);
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM deployment WHERE singleton = true AND id = $1 AND config_epoch = $2 AND server_build_id = $3 AND relay_protocol_version = 1 AND profile = $4 AND config_proof IS NOT DISTINCT FROM $5 FOR UPDATE)")
        .bind(state.database.deployment_id())
        .bind(state.config.config_epoch())
        .bind(build::BUILD_ID)
        .bind(state.config.profile_database_value())
        .bind(config_proof)
        .fetch_one(&mut **transaction).await.map_err(|_| RelayError::Unavailable)?;
    if valid {
        Ok(())
    } else {
        Err(RelayError::Fenced)
    }
}

async fn validate_node(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: &ServerState,
) -> RelayResult<()> {
    state.lease.check().map_err(|_| RelayError::Fenced)?;
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_nodes WHERE incarnation_id = $1 AND state = 'serving' AND config_epoch = $2 AND server_build_id = $3 AND relay_protocol_version = 1 AND lease_until > clock_timestamp() FOR UPDATE)")
        .bind(state.lease.incarnation_id()).bind(state.config.config_epoch()).bind(build::BUILD_ID)
        .fetch_one(&mut **transaction).await.map_err(|_| RelayError::Unavailable)?;
    if valid {
        Ok(())
    } else {
        Err(RelayError::Fenced)
    }
}

async fn validate_releasing_node(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: &ServerState,
) -> RelayResult<()> {
    state.lease.check().map_err(|_| RelayError::Fenced)?;
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM server_nodes WHERE incarnation_id = $1 AND state IN ('serving', 'draining') AND config_epoch = $2 AND server_build_id = $3 AND relay_protocol_version = 1 AND lease_until > clock_timestamp() FOR UPDATE)")
        .bind(state.lease.incarnation_id()).bind(state.config.config_epoch()).bind(build::BUILD_ID)
        .fetch_one(&mut **transaction).await.map_err(|_| RelayError::Unavailable)?;
    if valid {
        Ok(())
    } else {
        Err(RelayError::Fenced)
    }
}

async fn send_route_frame(
    socket: &mut WebSocket,
    frame: &ServerFrame,
    dispatch_open: &Mutex<bool>,
    barrier: &CancellationToken,
    lease: &NodeLease,
) -> RelayResult<()> {
    let dispatch_open = dispatch_open.lock().await;
    lease.check().map_err(|_| RelayError::Fenced)?;
    if !*dispatch_open || barrier.is_cancelled() {
        return Err(RelayError::Unavailable);
    }
    send_frame(socket, frame).await
}

async fn receive_frame(socket: &mut WebSocket, deadline: Duration) -> RelayResult<ClientFrame> {
    let message = timeout(deadline, socket.next())
        .await
        .map_err(|_| RelayError::Protocol)?
        .ok_or(RelayError::Unavailable)?
        .map_err(|_| RelayError::Protocol)?;
    decode_frame(message)
}

fn decode_frame(message: Message) -> RelayResult<ClientFrame> {
    let Message::Text(text) = message else {
        return Err(RelayError::Protocol);
    };
    if text.len() > MAX_FRAME_BYTES {
        return Err(RelayError::Protocol);
    }
    serde_json::from_str(&text).map_err(|_| RelayError::Protocol)
}

async fn send_frame(socket: &mut WebSocket, frame: &ServerFrame) -> RelayResult<()> {
    let encoded = serde_json::to_string(frame).map_err(|_| RelayError::Protocol)?;
    timeout(SEND_TIMEOUT, socket.send(Message::Text(encoded.into())))
        .await
        .map_err(|_| RelayError::Unavailable)?
        .map_err(|_| RelayError::Unavailable)
}

async fn send_error(socket: &mut WebSocket, error: RelayError) -> RelayResult<()> {
    let _ = send_frame(
        socket,
        &ServerFrame::Error {
            code: error.code(),
            message: error.message(),
        },
    )
    .await;
    Err(error)
}

async fn write_data(writer: &mut WriteHalf<DuplexStream>, data: &str) -> RelayResult<()> {
    let decoded = URL_SAFE_NO_PAD
        .decode(data)
        .map_err(|_| RelayError::Protocol)?;
    if decoded.len() > MAX_DATA_BYTES {
        return Err(RelayError::Protocol);
    }
    timeout(SEND_TIMEOUT, writer.write_all(&decoded))
        .await
        .map_err(|_| RelayError::Unavailable)?
        .map_err(|_| RelayError::Unavailable)
}

fn random_32() -> [u8; 32] {
    let mut value = [0_u8; 32];
    rand_core::OsRng.fill_bytes(&mut value);
    value
}

fn decode_32(value: &str) -> RelayResult<[u8; 32]> {
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| RelayError::Protocol)?;
    let bytes: [u8; 32] = decoded.try_into().map_err(|_| RelayError::Protocol)?;
    if URL_SAFE_NO_PAD.encode(bytes) != value {
        return Err(RelayError::Protocol);
    }
    Ok(bytes)
}

fn decode_signature(value: &str) -> RelayResult<Signature> {
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| RelayError::Protocol)?;
    Signature::from_slice(&decoded).map_err(|_| RelayError::Protocol)
}

fn verify_signature(public_key: [u8; 32], signature: Signature, message: &[u8]) -> RelayResult<()> {
    VerifyingKey::from_bytes(&public_key)
        .map_err(|_| RelayError::Protocol)?
        .verify(message, &signature)
        .map_err(|_| RelayError::ProofFailed)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayError {
    InvalidToken,
    InvalidState,
    ProofFailed,
    Protocol,
    Unavailable,
    Overloaded,
    OwnerConflict,
    Conflict,
    Ambiguous,
    Fenced,
}

impl RelayError {
    const fn code(self) -> &'static str {
        match self {
            Self::InvalidToken => "invalid_token",
            Self::InvalidState | Self::Conflict => "invalid_state",
            Self::ProofFailed => "proof_failed",
            Self::Protocol => "protocol_error",
            Self::OwnerConflict => "owner_conflict",
            Self::Unavailable | Self::Overloaded | Self::Ambiguous | Self::Fenced => {
                "temporarily_unavailable"
            }
        }
    }
    const fn message(self) -> &'static str {
        match self {
            Self::InvalidToken => "Enrollment token is invalid or expired.",
            Self::InvalidState | Self::Conflict => "Relay or Machine state is invalid.",
            Self::ProofFailed => "Relay identity or SSH access proof failed.",
            Self::Protocol => "Relay protocol frame is invalid.",
            Self::OwnerConflict => "Machine already has a valid owner.",
            Self::Unavailable | Self::Overloaded | Self::Ambiguous | Self::Fenced => {
                "Relay service is temporarily unavailable."
            }
        }
    }
}

impl std::fmt::Display for RelayError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}
impl std::error::Error for RelayError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn route() -> RouteIdentity {
        RouteIdentity {
            route_revision: 3,
            connection_epoch: 7,
            connection_id: Uuid::new_v4(),
        }
    }

    #[test]
    fn owner_release_and_lifecycle_transition_have_one_atomic_winner() {
        let machine_id = Uuid::new_v4();
        let route = route();

        let mut transition_first = RegistryState::default();
        assert!(transition_first.begin_transition(machine_id, route));
        assert_eq!(
            transition_first.begin_owner_release(machine_id, route),
            Some(OwnerReleaseDecision::Defer)
        );
        assert_eq!(
            transition_first.owner_exits.get(&machine_id),
            Some(&OwnerExitState::Transitioning(route))
        );

        let mut release_first = RegistryState::default();
        assert_eq!(
            release_first.begin_owner_release(machine_id, route),
            Some(OwnerReleaseDecision::Release)
        );
        assert!(!release_first.begin_transition(machine_id, route));
        assert_eq!(
            release_first.owner_exits.get(&machine_id),
            Some(&OwnerExitState::Releasing(route))
        );
        let replacement_route = RouteIdentity {
            connection_epoch: route.connection_epoch + 1,
            ..route
        };
        release_first.finish_owner_release(machine_id, replacement_route);
        assert_eq!(
            release_first.owner_exits.get(&machine_id),
            Some(&OwnerExitState::Releasing(route))
        );
        release_first.finish_owner_release(machine_id, route);
        assert!(!release_first.owner_exits.contains_key(&machine_id));
    }
}
