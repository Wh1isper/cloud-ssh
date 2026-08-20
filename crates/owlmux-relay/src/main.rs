mod protocol;
mod state;

use std::{
    collections::HashMap,
    env,
    error::Error,
    io::{self, IsTerminal as _, Read as _, Write as _},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer as _, SigningKey};
use futures_util::{SinkExt as _, StreamExt as _};
use protocol::{
    ChallengePurpose, ClientFrame, CloseReason, MAX_DATA_BYTES, MAX_FRAME_BYTES, MAX_STREAMS,
    ServerFrame, VERSION, signature_message,
};
use ssh_key::{Algorithm, HashAlg, PublicKey};
use state::RelayState;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpStream, tcp::OwnedWriteHalf},
    sync::mpsc,
    time::{Instant, sleep, sleep_until, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{self, Message},
};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;
use zeroize::Zeroizing;

type RelaySocket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type RelayResult<T> = Result<T, RelayError>;

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const TUNNEL_INBOUND_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Start,
    Enroll,
    Run,
    Reset,
}

struct Options {
    command: Command,
    server: Option<String>,
    state_path: PathBuf,
    account: Option<String>,
    confirm_ready: bool,
    expected_host_key_sha256: Option<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .try_init()
        .ok();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("owlmux-relay: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
    let options = parse_options()?;
    match options.command {
        Command::Enroll => {
            let account = options
                .account
                .as_deref()
                .ok_or("--account is required for enrollment")?;
            let server = options.server.as_deref().ok_or("--server is required")?;
            let mut state = RelayState::load_or_create(&options.state_path, account)?;
            enroll(
                server,
                &options.state_path,
                &mut state,
                options.confirm_ready,
                options.expected_host_key_sha256.as_deref(),
            )
            .await?;
        }
        Command::Run => {
            let server = options.server.as_deref().ok_or("--server is required")?;
            let state = RelayState::load(&options.state_path)?;
            run_forever(server, state).await?;
        }
        Command::Start => {
            let server = options.server.as_deref().ok_or("--server is required")?;
            let mut state = if options.state_path.exists() {
                RelayState::load(&options.state_path)?
            } else {
                let account = options
                    .account
                    .as_deref()
                    .ok_or("--account is required for first enrollment")?;
                RelayState::load_or_create(&options.state_path, account)?
            };
            if state.deployment_id.is_none()
                || state.machine_id.is_none()
                || state.route_revision.is_none()
            {
                enroll(
                    server,
                    &options.state_path,
                    &mut state,
                    options.confirm_ready,
                    options.expected_host_key_sha256.as_deref(),
                )
                .await?;
            }
            run_forever(server, state).await?;
        }
        Command::Reset => {
            let mut state = RelayState::load(&options.state_path)?;
            state.reset_identity(&options.state_path)?;
            println!(
                "Relay candidate identity reset; issue a new enrollment token before enrolling."
            );
        }
    }
    Ok(())
}

fn parse_options() -> Result<Options, Box<dyn Error + Send + Sync>> {
    let mut arguments = env::args().skip(1);
    let command = match arguments.next().as_deref() {
        Some("start") => Command::Start,
        Some("enroll") => Command::Enroll,
        Some("run") => Command::Run,
        Some("reset") => Command::Reset,
        Some("--version" | "-V") => {
            println!("owlmux-relay {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        Some("--help" | "-h") | None => {
            print_help();
            std::process::exit(0);
        }
        Some(_) => return Err("expected start, enroll, run, or reset".into()),
    };
    let mut server = None;
    let mut state_path = None;
    let mut account = None;
    let mut confirm_ready = false;
    let mut expected_host_key_sha256 = None;
    while let Some(argument) = arguments.next() {
        if argument == "--confirm-ready" {
            confirm_ready = true;
            continue;
        }
        let value = arguments.next().ok_or("option value is missing")?;
        match argument.as_str() {
            "--server" => server = Some(value),
            "--state" => state_path = Some(PathBuf::from(value)),
            "--account" => account = Some(value),
            "--expected-host-key-sha256" => expected_host_key_sha256 = Some(value),
            _ => return Err(format!("unknown option: {argument}").into()),
        }
    }
    if server.as_ref().is_some_and(|value| {
        (!value.starts_with("ws://") && !value.starts_with("wss://"))
            || value.contains('?')
            || value.contains('#')
    }) {
        return Err("--server must be a ws:// or wss:// origin".into());
    }
    validate_command_options(
        command,
        server.as_deref(),
        account.as_deref(),
        confirm_ready,
        expected_host_key_sha256.as_deref(),
    )?;
    Ok(Options {
        command,
        server: server.map(|value| value.trim_end_matches('/').to_owned()),
        state_path: state_path.ok_or("--state is required")?,
        account,
        confirm_ready,
        expected_host_key_sha256,
    })
}

fn validate_command_options(
    command: Command,
    server: Option<&str>,
    account: Option<&str>,
    confirm_ready: bool,
    expected_host_key_sha256: Option<&str>,
) -> Result<(), &'static str> {
    match command {
        Command::Start if server.is_none() => Err("--server is required for start"),
        Command::Enroll if server.is_none() => Err("--server is required for enrollment"),
        Command::Enroll if account.is_none() => Err("--account is required for enrollment"),
        Command::Run if server.is_none() => Err("--server is required for run"),
        Command::Run
            if account.is_some() || confirm_ready || expected_host_key_sha256.is_some() =>
        {
            Err("run accepts only --server and --state")
        }
        Command::Reset
            if server.is_some()
                || account.is_some()
                || confirm_ready
                || expected_host_key_sha256.is_some() =>
        {
            Err("reset accepts only --state")
        }
        _ => Ok(()),
    }
}

fn print_help() {
    println!("owlmux-relay {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Usage:");
    println!(
        "  owlmux-relay start  --server <ws-origin> --state <path> [--account <user>] [--confirm-ready] [--expected-host-key-sha256 <fingerprint>]"
    );
    println!(
        "  owlmux-relay enroll --server <ws-origin> --state <path> --account <user> [--confirm-ready] [--expected-host-key-sha256 <fingerprint>]"
    );
    println!("  owlmux-relay run    --server <ws-origin> --state <path>");
    println!("  owlmux-relay reset  --state <path>");
    println!();
    println!(
        "Enrollment tokens are read from a no-echo prompt or bounded stdin, never argv or environment."
    );
}

async fn enroll(
    server: &str,
    state_path: &Path,
    state: &mut RelayState,
    confirm_ready: bool,
    expected_host_key_sha256: Option<&str>,
) -> RelayResult<()> {
    if state.route_revision.is_some() {
        return Err(RelayError::State("Relay is already enrolled"));
    }
    let token = read_token()?;
    let (mut socket, _) = connect_async(format!("{server}/relay/v1/enroll"))
        .await
        .map_err(|_| RelayError::Transport)?;
    send_frame(
        &mut socket,
        &ClientFrame::Token {
            token: token.to_string(),
        },
    )
    .await?;
    let (deployment_id, machine_id, route_revision) =
        match receive_frame(&mut socket, Duration::from_secs(5)).await? {
            ServerFrame::Accepted {
                deployment_id,
                machine_id,
                route_revision,
                ..
            } => (deployment_id, machine_id, route_revision),
            ServerFrame::Error { code, .. } => return Err(RelayError::Remote(code)),
            _ => return Err(RelayError::Protocol),
        };
    state.deployment_id = Some(deployment_id);
    state.machine_id = Some(machine_id);
    state.route_revision = Some(route_revision);
    state.persist(state_path).map_err(RelayError::StateFile)?;

    let signing_key = state.signing_key().map_err(RelayError::StateFile)?;
    send_frame(
        &mut socket,
        &ClientFrame::Setup {
            protocol: VERSION,
            relay_id: state.relay_id,
            public_key: URL_SAFE_NO_PAD.encode(signing_key.verifying_key().as_bytes()),
            endpoint: "127.0.0.1:22".to_owned(),
            observed_account: state.target_account.clone(),
        },
    )
    .await?;
    let credential = receive_frame(&mut socket, Duration::from_secs(5)).await?;
    confirm_target_authorization(credential, confirm_ready)?;
    send_frame(&mut socket, &ClientFrame::Ready).await?;
    let (purpose, nonce) = receive_challenge(&mut socket).await?;
    if !matches!(purpose, ChallengePurpose::Enrollment) {
        return Err(RelayError::Protocol);
    }
    send_signature(
        &mut socket,
        &signing_key,
        purpose,
        deployment_id,
        machine_id,
        state.relay_id,
        None,
        route_revision,
        &nonce,
    )
    .await?;
    match enrollment_stream(&mut socket, state, state_path, expected_host_key_sha256).await {
        Ok(()) | Err(RelayError::Transport) => confirm_enrollment(server, state).await,
        Err(error) => Err(error),
    }
}

fn confirm_target_authorization(frame: ServerFrame, confirmed: bool) -> RelayResult<()> {
    let ServerFrame::Credential {
        credential_id,
        name,
        public_key,
        public_fingerprint_sha256,
    } = frame
    else {
        return Err(RelayError::Protocol);
    };
    println!("Target SSH credential: {name} ({credential_id})");
    println!("Public key: {public_key}");
    println!("Fingerprint: {public_fingerprint_sha256}");
    if confirmed {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        return Err(RelayError::State(
            "target authorization confirmation requires --confirm-ready in non-interactive mode",
        ));
    }
    eprint!("Confirm this public key is installed for the target account [y/N]: ");
    io::stderr().flush().map_err(|_| RelayError::Input)?;
    let mut response = String::new();
    io::stdin()
        .read_line(&mut response)
        .map_err(|_| RelayError::Input)?;
    if matches!(response.trim(), "y" | "Y" | "yes" | "YES") {
        Ok(())
    } else {
        Err(RelayError::State("target authorization was not confirmed"))
    }
}

fn confirm_host_key(
    host_identity: &str,
    advertised_fingerprint_sha256: &str,
    expected_fingerprint: Option<&str>,
) -> RelayResult<()> {
    let mut fields = host_identity.split_ascii_whitespace();
    let algorithm = fields.next().ok_or(RelayError::Protocol)?;
    let encoded_key = fields.next().ok_or(RelayError::Protocol)?;
    if fields.next().is_some() || format!("{algorithm} {encoded_key}") != host_identity {
        return Err(RelayError::Protocol);
    }
    let public_key = PublicKey::from_openssh(host_identity).map_err(|_| RelayError::Protocol)?;
    if public_key.algorithm() != Algorithm::Ed25519 {
        return Err(RelayError::Protocol);
    }
    let fingerprint_sha256 = public_key.fingerprint(HashAlg::Sha256).to_string();
    if fingerprint_sha256 != advertised_fingerprint_sha256 {
        return Err(RelayError::Protocol);
    }

    println!("The authenticity of host '127.0.0.1:22' can't be established.");
    println!("ED25519 key fingerprint is {fingerprint_sha256}.");
    if let Some(expected) = expected_fingerprint {
        if expected != fingerprint_sha256 {
            return Err(RelayError::State(
                "discovered SSH host-key fingerprint does not match the expected fingerprint",
            ));
        }
        println!("Matched the expected target SSH host-key fingerprint.");
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        return Err(RelayError::State(
            "SSH host-key confirmation requires --expected-host-key-sha256 in non-interactive mode",
        ));
    }
    eprint!("Are you sure you want to continue connecting (yes/no)? ");
    io::stderr().flush().map_err(|_| RelayError::Input)?;
    let mut response = String::new();
    io::stdin()
        .read_line(&mut response)
        .map_err(|_| RelayError::Input)?;
    if !is_exact_yes(&response) {
        return Err(RelayError::State("SSH host key was not accepted"));
    }
    println!("Accepted the target SSH host key for strict verification.");
    Ok(())
}

fn is_exact_yes(response: &str) -> bool {
    matches!(response, "yes" | "yes\n" | "yes\r\n")
}

async fn enrollment_stream(
    socket: &mut RelaySocket,
    state: &mut RelayState,
    state_path: &Path,
    expected_host_key_sha256: Option<&str>,
) -> RelayResult<()> {
    let mut target: Option<(u32, OwnedWriteHalf)> = None;
    let mut next_stream_id = 1_u32;
    let mut host_key_confirmed = false;
    let (outbound_tx, mut outbound_rx) = mpsc::channel(16);
    loop {
        tokio::select! {
            biased;
            incoming = receive_frame(socket, Duration::from_secs(45)) => match incoming? {
                ServerFrame::OpenStream { stream_id }
                    if target.is_none() && stream_id == next_stream_id && stream_id <= 2 =>
                {
                    let stream = timeout(Duration::from_secs(5), TcpStream::connect("127.0.0.1:22"))
                        .await.map_err(|_| RelayError::Transport)?.map_err(|_| RelayError::Transport)?;
                    let (reader, writer) = stream.into_split();
                    target = Some((stream_id, writer));
                    next_stream_id = next_stream_id.checked_add(1).ok_or(RelayError::Protocol)?;
                    spawn_target_reader(stream_id, reader, outbound_tx.clone());
                    send_frame(socket, &ClientFrame::StreamOpened { stream_id }).await?;
                }
                ServerFrame::StreamData { stream_id, data } => {
                    let (active_stream_id, writer) = target.as_mut().ok_or(RelayError::Protocol)?;
                    if *active_stream_id != stream_id {
                        return Err(RelayError::Protocol);
                    }
                    write_data(writer, &data).await?;
                }
                ServerFrame::StreamHalfClosed { stream_id } => {
                    let (active_stream_id, writer) = target.as_mut().ok_or(RelayError::Protocol)?;
                    if *active_stream_id != stream_id {
                        return Err(RelayError::Protocol);
                    }
                    let _ = writer.shutdown().await;
                }
                ServerFrame::StreamClosed { stream_id, .. } => {
                    let (active_stream_id, mut writer) = target.take().ok_or(RelayError::Protocol)?;
                    if active_stream_id != stream_id {
                        return Err(RelayError::Protocol);
                    }
                    let _ = writer.shutdown().await;
                    send_frame(
                        socket,
                        &ClientFrame::StreamClosed {
                            stream_id,
                            reason: CloseReason::Eof,
                        },
                    )
                    .await?;
                }
                ServerFrame::HostKey { host_identity, fingerprint_sha256 }
                    if target.is_none() && next_stream_id == 2 && !host_key_confirmed =>
                {
                    confirm_host_key(
                        &host_identity,
                        &fingerprint_sha256,
                        expected_host_key_sha256,
                    )?;
                    send_frame(socket, &ClientFrame::HostKeyAccepted { host_identity }).await?;
                    host_key_confirmed = true;
                }
                ServerFrame::Activated { route_revision } if target.is_none() => {
                    state.route_revision = Some(route_revision);
                    state.persist(state_path).map_err(RelayError::StateFile)?;
                    if host_key_confirmed {
                        println!("Permanently pinned '127.0.0.1:22' (ED25519) to the OwlMux Host.");
                    }
                    info!(%route_revision, "Relay enrollment activated");
                    let _ = timeout(IO_TIMEOUT, socket.close(None)).await;
                    return Ok(());
                }
                ServerFrame::Ping { nonce } => send_frame(socket, &ClientFrame::Pong { nonce }).await?,
                ServerFrame::Error { code, .. } => return Err(RelayError::Remote(code)),
                _ => return Err(RelayError::Protocol),
            },
            Some(frame) = outbound_rx.recv() => {
                let stream_id = match &frame {
                    ClientFrame::StreamData { stream_id, .. }
                    | ClientFrame::StreamHalfClosed { stream_id }
                    | ClientFrame::StreamClosed { stream_id, .. } => *stream_id,
                    _ => return Err(RelayError::Protocol),
                };
                if target
                    .as_ref()
                    .is_some_and(|(active_stream_id, _)| *active_stream_id == stream_id)
                {
                    send_frame(socket, &frame).await?;
                }
            }
        }
    }
}

async fn run_forever(server: &str, state: RelayState) -> RelayResult<()> {
    loop {
        match tunnel_once(server, &state).await {
            Ok(TunnelExit::Shutdown) => return Ok(()),
            Ok(TunnelExit::Disconnected) | Err(RelayError::Transport | RelayError::Remote(_)) => {
                warn!("Relay tunnel disconnected; retrying");
                sleep(Duration::from_secs(1)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

async fn confirm_enrollment(server: &str, state: &RelayState) -> RelayResult<()> {
    let (mut socket, _, _, _) = open_tunnel(server, state).await?;
    let _ = timeout(IO_TIMEOUT, socket.close(None)).await;
    info!("Relay enrollment and initial owner claim completed");
    Ok(())
}

async fn tunnel_once(server: &str, state: &RelayState) -> RelayResult<TunnelExit> {
    let (mut socket, _, _, _) = open_tunnel(server, state).await?;
    relay_tunnel(&mut socket).await
}

async fn open_tunnel(
    server: &str,
    state: &RelayState,
) -> RelayResult<(RelaySocket, Uuid, Uuid, i64)> {
    let deployment_id = state
        .deployment_id
        .ok_or(RelayError::State("Relay enrollment is incomplete"))?;
    let machine_id = state
        .machine_id
        .ok_or(RelayError::State("Relay enrollment is incomplete"))?;
    let route_revision = state
        .route_revision
        .ok_or(RelayError::State("Relay enrollment is incomplete"))?;
    let connection_id = Uuid::new_v4();
    let signing_key = state.signing_key().map_err(RelayError::StateFile)?;
    let (mut socket, _) = connect_async(format!("{server}/relay/v1/tunnel"))
        .await
        .map_err(|_| RelayError::Transport)?;
    send_frame(
        &mut socket,
        &ClientFrame::TunnelHello {
            protocol: VERSION,
            deployment_id,
            machine_id,
            relay_id: state.relay_id,
            connection_id,
            route_revision,
        },
    )
    .await?;
    let (purpose, nonce) = receive_challenge(&mut socket).await?;
    if !matches!(purpose, ChallengePurpose::Tunnel) {
        return Err(RelayError::Protocol);
    }
    send_signature(
        &mut socket,
        &signing_key,
        purpose,
        deployment_id,
        machine_id,
        state.relay_id,
        Some(connection_id),
        route_revision,
        &nonce,
    )
    .await?;
    let connection_epoch = match receive_frame(&mut socket, Duration::from_secs(5)).await? {
        ServerFrame::TunnelEstablished { connection_epoch } => connection_epoch,
        ServerFrame::Error { code, .. } => return Err(RelayError::Remote(code)),
        _ => return Err(RelayError::Protocol),
    };
    info!(%machine_id, %connection_id, %connection_epoch, "Relay tunnel authenticated and owned");
    Ok((socket, machine_id, connection_id, connection_epoch))
}

async fn relay_tunnel(socket: &mut RelaySocket) -> RelayResult<TunnelExit> {
    let mut targets = HashMap::<u32, OwnedWriteHalf>::new();
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<ClientFrame>(64);
    let mut inbound_deadline = Instant::now() + TUNNEL_INBOUND_TIMEOUT;
    loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal.map_err(|_| RelayError::Transport)?;
                return Ok(TunnelExit::Shutdown);
            }
            Some(frame) = outbound_rx.recv() => send_frame(socket, &frame).await?,
            () = sleep_until(inbound_deadline) => return Ok(TunnelExit::Disconnected),
            message = socket.next() => {
                let Some(Ok(message)) = message else {
                    return Ok(TunnelExit::Disconnected);
                };
                let incoming = decode_frame(message)?;
                inbound_deadline = Instant::now() + TUNNEL_INBOUND_TIMEOUT;
                match incoming {
                ServerFrame::OpenStream { stream_id } => {
                    if stream_id == 0 || targets.len() >= MAX_STREAMS || targets.contains_key(&stream_id) {
                        return Err(RelayError::Protocol);
                    }
                    match timeout(Duration::from_secs(5), TcpStream::connect("127.0.0.1:22")).await {
                        Ok(Ok(stream)) => {
                            let (reader, writer) = stream.into_split();
                            targets.insert(stream_id, writer);
                            spawn_target_reader(stream_id, reader, outbound_tx.clone());
                            send_frame(socket, &ClientFrame::StreamOpened { stream_id }).await?;
                        }
                        _ => send_frame(socket, &ClientFrame::StreamClosed { stream_id, reason: CloseReason::ConnectFailed }).await?,
                    }
                }
                ServerFrame::StreamData { stream_id, data } => {
                    let writer = targets.get_mut(&stream_id).ok_or(RelayError::Protocol)?;
                    write_data(writer, &data).await?;
                }
                ServerFrame::StreamHalfClosed { stream_id } | ServerFrame::StreamClosed { stream_id, .. } => {
                    if let Some(mut writer) = targets.remove(&stream_id) { let _ = writer.shutdown().await; }
                }
                ServerFrame::Ping { nonce } => send_frame(socket, &ClientFrame::Pong { nonce }).await?,
                ServerFrame::Error { code, .. } => return Err(RelayError::Remote(code)),
                _ => return Err(RelayError::Protocol),
                }
            }
        }
    }
}

fn spawn_target_reader(
    stream_id: u32,
    mut reader: tokio::net::tcp::OwnedReadHalf,
    outbound: mpsc::Sender<ClientFrame>,
) {
    tokio::spawn(async move {
        let mut buffer = vec![0_u8; MAX_DATA_BYTES];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => {
                    let _ = outbound
                        .send(ClientFrame::StreamHalfClosed { stream_id })
                        .await;
                    return;
                }
                Ok(size) => {
                    if outbound
                        .send(ClientFrame::StreamData {
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
                        .send(ClientFrame::StreamClosed {
                            stream_id,
                            reason: CloseReason::Shutdown,
                        })
                        .await;
                    return;
                }
            }
        }
    });
}

async fn receive_challenge(socket: &mut RelaySocket) -> RelayResult<(ChallengePurpose, [u8; 32])> {
    match receive_frame(socket, Duration::from_secs(5)).await? {
        ServerFrame::Challenge { purpose, nonce } => Ok((purpose, decode_32(&nonce)?)),
        ServerFrame::Error { code, .. } => Err(RelayError::Remote(code)),
        _ => Err(RelayError::Protocol),
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_signature(
    socket: &mut RelaySocket,
    signing_key: &SigningKey,
    purpose: ChallengePurpose,
    deployment_id: Uuid,
    machine_id: Uuid,
    relay_id: Uuid,
    connection_id: Option<Uuid>,
    route_revision: i64,
    nonce: &[u8; 32],
) -> RelayResult<()> {
    let signature = signing_key.sign(&signature_message(
        purpose,
        deployment_id,
        machine_id,
        relay_id,
        connection_id,
        route_revision,
        nonce,
    ));
    send_frame(
        socket,
        &ClientFrame::Signature {
            signature: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
        },
    )
    .await
}

async fn receive_frame(socket: &mut RelaySocket, deadline: Duration) -> RelayResult<ServerFrame> {
    let message = timeout(deadline, socket.next())
        .await
        .map_err(|_| RelayError::Transport)?
        .ok_or(RelayError::Transport)?
        .map_err(|_| RelayError::Transport)?;
    decode_frame(message)
}

fn decode_frame(message: Message) -> RelayResult<ServerFrame> {
    let Message::Text(text) = message else {
        return Err(RelayError::Protocol);
    };
    if text.len() > MAX_FRAME_BYTES {
        return Err(RelayError::Protocol);
    }
    serde_json::from_str(&text).map_err(|_| RelayError::Protocol)
}

async fn send_frame(socket: &mut RelaySocket, frame: &ClientFrame) -> RelayResult<()> {
    let encoded = serde_json::to_string(frame).map_err(|_| RelayError::Protocol)?;
    if encoded.len() > MAX_FRAME_BYTES {
        return Err(RelayError::Protocol);
    }
    timeout(IO_TIMEOUT, socket.send(Message::Text(encoded.into())))
        .await
        .map_err(|_| RelayError::Transport)?
        .map_err(|_| RelayError::Transport)
}

async fn write_data(writer: &mut OwnedWriteHalf, data: &str) -> RelayResult<()> {
    let decoded = URL_SAFE_NO_PAD
        .decode(data)
        .map_err(|_| RelayError::Protocol)?;
    if decoded.len() > MAX_DATA_BYTES {
        return Err(RelayError::Protocol);
    }
    timeout(IO_TIMEOUT, writer.write_all(&decoded))
        .await
        .map_err(|_| RelayError::Transport)?
        .map_err(|_| RelayError::Transport)
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

fn read_token() -> RelayResult<Zeroizing<String>> {
    let value = if io::stdin().is_terminal() {
        rpassword::prompt_password("Enrollment token: ").map_err(|_| RelayError::Input)?
    } else {
        let mut bytes = Vec::new();
        io::stdin()
            .take(128)
            .read_to_end(&mut bytes)
            .map_err(|_| RelayError::Input)?;
        String::from_utf8(bytes).map_err(|_| RelayError::Input)?
    };
    let value = value.trim_end_matches(['\r', '\n']).to_owned();
    if !value.starts_with("owlmux_enroll_v1_") || value.len() > 80 {
        return Err(RelayError::Input);
    }
    Ok(Zeroizing::new(value))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TunnelExit {
    Shutdown,
    Disconnected,
}

#[derive(Debug)]
enum RelayError {
    Input,
    State(&'static str),
    StateFile(state::StateError),
    Protocol,
    Transport,
    Remote(String),
}

impl std::fmt::Display for RelayError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input => formatter.write_str("enrollment token input is invalid"),
            Self::State(message) => formatter.write_str(message),
            Self::StateFile(error) => error.fmt(formatter),
            Self::Protocol => formatter.write_str("Relay protocol failed closed"),
            Self::Transport => formatter.write_str("Relay transport is unavailable"),
            Self::Remote(code) => write!(formatter, "Server rejected Relay operation ({code})"),
        }
    }
}
impl std::error::Error for RelayError {}

impl From<tungstenite::Error> for RelayError {
    fn from(_: tungstenite::Error) -> Self {
        Self::Transport
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_key_confirmation_binds_the_key_to_its_fingerprint() {
        let private_key = ssh_key::PrivateKey::random(&mut rand_core::OsRng, Algorithm::Ed25519)
            .expect("generate host fixture");
        let public_key = private_key
            .public_key()
            .to_openssh()
            .expect("encode host fixture");
        let fingerprint = private_key
            .public_key()
            .fingerprint(HashAlg::Sha256)
            .to_string();

        assert!(confirm_host_key(&public_key, &fingerprint, Some(&fingerprint)).is_ok());
        assert!(matches!(
            confirm_host_key(&public_key, "SHA256:wrong", Some(&fingerprint)),
            Err(RelayError::Protocol)
        ));
        assert!(matches!(
            confirm_host_key(
                &format!("{public_key} untrusted-comment"),
                &fingerprint,
                Some(&fingerprint),
            ),
            Err(RelayError::Protocol)
        ));
    }

    #[test]
    fn host_key_confirmation_requires_exact_yes() {
        for accepted in ["yes", "yes\n", "yes\r\n"] {
            assert!(is_exact_yes(accepted));
        }
        for rejected in [" yes\n", "yes \n", "YES\n", "y\n", "yes\t\n", "yes\r"] {
            assert!(!is_exact_yes(rejected));
        }
    }

    #[test]
    fn command_options_reject_silent_no_ops() {
        assert!(
            validate_command_options(Command::Run, Some("ws://server"), None, false, None).is_ok()
        );
        assert!(validate_command_options(Command::Reset, None, None, false, None).is_ok());
        assert!(
            validate_command_options(Command::Run, Some("ws://server"), None, true, None).is_err()
        );
        assert!(
            validate_command_options(Command::Run, Some("ws://server"), Some("user"), false, None,)
                .is_err()
        );
        assert!(
            validate_command_options(Command::Reset, Some("ws://server"), None, false, None)
                .is_err()
        );
        assert!(
            validate_command_options(
                Command::Run,
                Some("ws://server"),
                None,
                false,
                Some("SHA256:test"),
            )
            .is_err()
        );
    }
}
