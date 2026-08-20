use std::{
    fs::{File, OpenOptions},
    io,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use nix::fcntl::{Flock, FlockArg};
use sqlx::Row as _;
use ssh_key::{Algorithm, HashAlg, PublicKey};
use tokio::{
    io::{
        AsyncBufRead, AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader,
        DuplexStream,
    },
    net::TcpListener,
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
    time::timeout,
};
use tokio_util::task::AbortOnDropHandle;
use uuid::Uuid;

use crate::{crypto, deployment::NodeLease, service::ServerState};

const PROOF_MARKER: &[u8] = b"__OWLMUX_SSH_OK_V1__\n";
const SESSION_MARKER: &[u8] = b"__OWLMUX_TMUX_SESSION_OK_V1__\n";
const CREATE_MARKER: &str = "__OWLMUX_TMUX_CREATED_V1__\t";
const SSH_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_DIAGNOSTIC_BYTES: usize = 4096;
const MAX_DIAGNOSTIC_READ: u64 = 4097;
const MAX_CONTROL_LINE_BYTES: usize = 256 * 1024;

pub struct SshRuntime {
    instance_dir: PathBuf,
    owner_uid: u32,
    _root_lock: Flock<File>,
    child_limit: Arc<Semaphore>,
}

struct ChildDirectory {
    path: PathBuf,
    owner_uid: u32,
    _permit: OwnedSemaphorePermit,
}

impl ChildDirectory {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ChildDirectory {
    fn drop(&mut self) {
        let _ = cleanup_child_directory(&self.path, self.owner_uid);
    }
}

impl SshRuntime {
    /// Create an exclusive startup-instance directory beneath the node-local runtime root.
    ///
    /// # Errors
    ///
    /// Returns a custody error when the root is linked, shared, has unsafe modes, or cannot be
    /// created exclusively.
    pub fn create(root: &Path) -> Result<Arc<Self>, SshError> {
        if !root.exists() {
            std::fs::create_dir(root).map_err(|_| SshError::Custody)?;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| SshError::Custody)?;
        }
        let root_metadata = std::fs::symlink_metadata(root).map_err(|_| SshError::Custody)?;
        if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
            return Err(SshError::Custody);
        }
        let root_mode = root_metadata.permissions().mode() & 0o777;
        if root_mode != 0o700 {
            return Err(SshError::Custody);
        }
        let owner_uid = root_metadata.uid();
        let root_lock = lock_runtime_root(root, owner_uid)?;
        scavenge_runtime_root(root, owner_uid)?;
        let instance_dir = root.join(format!("instance-{}", Uuid::new_v4()));
        std::fs::create_dir(&instance_dir).map_err(|_| SshError::Custody)?;
        std::fs::set_permissions(&instance_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| SshError::Custody)?;
        validate_directory(&instance_dir, owner_uid)?;
        Ok(Arc::new(Self {
            instance_dir,
            owner_uid,
            _root_lock: root_lock,
            child_limit: Arc::new(Semaphore::new(32)),
        }))
    }

    fn child_dir(&self) -> Result<ChildDirectory, SshError> {
        let permit = self
            .child_limit
            .clone()
            .try_acquire_owned()
            .map_err(|_| SshError::Unavailable)?;
        let path = self.instance_dir.join(format!("child-{}", Uuid::new_v4()));
        std::fs::create_dir(&path).map_err(|_| SshError::Custody)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| SshError::Custody)?;
        validate_directory(&path, self.owner_uid)?;
        Ok(ChildDirectory {
            path,
            owner_uid: self.owner_uid,
            _permit: permit,
        })
    }
}

impl Drop for SshRuntime {
    fn drop(&mut self) {
        let _ = cleanup_instance_directory(&self.instance_dir, self.owner_uid);
    }
}

fn lock_runtime_root(root: &Path, owner_uid: u32) -> Result<Flock<File>, SshError> {
    let lock_path = root.join(".lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW)
        .open(&lock_path)
        .map_err(|_| SshError::Custody)?;
    validate_regular_file(
        &lock_path,
        &file.metadata().map_err(|_| SshError::Custody)?,
        owner_uid,
    )?;
    Flock::lock(file, FlockArg::LockExclusiveNonblock).map_err(|_| SshError::Custody)
}

fn scavenge_runtime_root(root: &Path, owner_uid: u32) -> Result<(), SshError> {
    for entry in std::fs::read_dir(root).map_err(|_| SshError::Custody)? {
        let entry = entry.map_err(|_| SshError::Custody)?;
        let name = entry.file_name();
        if name == ".lock" {
            continue;
        }
        let name = name.to_str().ok_or(SshError::Custody)?;
        validate_scoped_name(name, "instance-")?;
        cleanup_instance_directory(&entry.path(), owner_uid)?;
    }
    Ok(())
}

fn cleanup_instance_directory(path: &Path, owner_uid: u32) -> Result<(), SshError> {
    validate_directory(path, owner_uid)?;
    for entry in std::fs::read_dir(path).map_err(|_| SshError::Custody)? {
        let entry = entry.map_err(|_| SshError::Custody)?;
        let name = entry.file_name();
        let name = name.to_str().ok_or(SshError::Custody)?;
        validate_scoped_name(name, "child-")?;
        cleanup_child_directory(&entry.path(), owner_uid)?;
    }
    std::fs::remove_dir(path).map_err(|_| SshError::Custody)
}

fn cleanup_child_directory(path: &Path, owner_uid: u32) -> Result<(), SshError> {
    validate_directory(path, owner_uid)?;
    for entry in std::fs::read_dir(path).map_err(|_| SshError::Custody)? {
        let entry = entry.map_err(|_| SshError::Custody)?;
        let name = entry.file_name();
        if name != "identity" && name != "known_hosts" {
            return Err(SshError::Custody);
        }
        let metadata = std::fs::symlink_metadata(entry.path()).map_err(|_| SshError::Custody)?;
        validate_regular_file(&entry.path(), &metadata, owner_uid)?;
        std::fs::remove_file(entry.path()).map_err(|_| SshError::Custody)?;
    }
    std::fs::remove_dir(path).map_err(|_| SshError::Custody)
}

fn validate_directory(path: &Path, owner_uid: u32) -> Result<(), SshError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| SshError::Custody)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != owner_uid
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(SshError::Custody);
    }
    Ok(())
}

fn validate_regular_file(
    _path: &Path,
    metadata: &std::fs::Metadata,
    owner_uid: u32,
) -> Result<(), SshError> {
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != owner_uid
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(SshError::Custody);
    }
    Ok(())
}

fn validate_scoped_name(name: &str, prefix: &str) -> Result<(), SshError> {
    let value = name.strip_prefix(prefix).ok_or(SshError::Custody)?;
    let id = Uuid::parse_str(value).map_err(|_| SshError::Custody)?;
    if id.to_string() != value {
        return Err(SshError::Custody);
    }
    Ok(())
}

fn spawn_abort_on_drop_bridge(
    listener: TcpListener,
    mut relay_stream: DuplexStream,
) -> AbortOnDropHandle<io::Result<()>> {
    AbortOnDropHandle::new(tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        tokio::io::copy_bidirectional(&mut socket, &mut relay_stream).await?;
        Ok(())
    }))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostIdentity {
    pub public_key: String,
    pub fingerprint_sha256: String,
}

/// Discover and stage the Ed25519 key offered by loopback sshd without authenticating.
///
/// # Errors
///
/// Returns a bounded custody, transport, parse, or fencing error.
pub async fn discover_host_identity(
    state: &Arc<ServerState>,
    relay_stream: DuplexStream,
) -> Result<HostIdentity, SshError> {
    state.lease.check().map_err(|_| SshError::Fenced)?;
    let child_dir = state.ssh.child_dir()?;
    let known_hosts_path = child_dir.path().join("known_hosts");
    write_exclusive(&known_hosts_path, b"", 0o600).await?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| SshError::Unavailable)?;
    let port = listener
        .local_addr()
        .map_err(|_| SshError::Unavailable)?
        .port();
    let bridge = spawn_abort_on_drop_bridge(listener, relay_stream);

    let mut child = command_for_host_discovery(&known_hosts_path, port)
        .spawn()
        .map_err(|_| SshError::Unavailable)?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or(SshError::Unavailable)?
        .take(MAX_DIAGNOSTIC_READ);
    let mut stderr = child
        .stderr
        .take()
        .ok_or(SshError::Unavailable)?
        .take(MAX_DIAGNOSTIC_READ);
    let result = timeout(SSH_TIMEOUT, async {
        let mut output = Vec::new();
        let mut diagnostic = Vec::new();
        let (stdout_result, stderr_result, status) = tokio::join!(
            stdout.read_to_end(&mut output),
            stderr.read_to_end(&mut diagnostic),
            child.wait()
        );
        let status = status.map_err(|_| SshError::ProofFailed)?;
        if stdout_result.is_err()
            || stderr_result.is_err()
            || !output.is_empty()
            || diagnostic.len() > MAX_DIAGNOSTIC_BYTES
            || status.success()
        {
            return Err(SshError::ProofFailed);
        }
        let mut known_hosts = tokio::fs::File::open(&known_hosts_path)
            .await
            .map_err(|_| SshError::Custody)?
            .take(2049);
        let mut bytes = Vec::new();
        known_hosts
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| SshError::Custody)?;
        if bytes.len() > 2048 {
            return Err(SshError::ProofFailed);
        }
        parse_known_host(&bytes)
    })
    .await
    .map_err(|_| SshError::ProofFailed)?;
    bridge.abort();
    state.lease.check().map_err(|_| SshError::Fenced)?;
    result
}

fn command_for_host_discovery(known_hosts_path: &Path, port: u16) -> Command {
    let mut command = Command::new("ssh");
    command
        .arg("-F")
        .arg("/dev/null")
        .arg("-T")
        .arg("-oBatchMode=yes")
        .arg("-oIdentitiesOnly=yes")
        .arg("-oIdentityAgent=none")
        .arg("-oPubkeyAuthentication=no")
        .arg("-oPasswordAuthentication=no")
        .arg("-oKbdInteractiveAuthentication=no")
        .arg("-oPreferredAuthentications=none")
        .arg("-oStrictHostKeyChecking=accept-new")
        .arg(format!(
            "-oUserKnownHostsFile={}",
            known_hosts_path.display()
        ))
        .arg("-oGlobalKnownHostsFile=/dev/null")
        .arg("-oHostKeyAlias=owlmux-target")
        .arg("-oHashKnownHosts=no")
        .arg("-oHostKeyAlgorithms=ssh-ed25519")
        .arg("-oUpdateHostKeys=no")
        .arg("-oRequestTTY=no")
        .arg("-oClearAllForwardings=yes")
        .arg("-oPermitLocalCommand=no")
        .arg("-oConnectTimeout=10")
        .arg("-p")
        .arg(port.to_string())
        .arg("--")
        .arg("owlmux-host-key-preflight-invalid@127.0.0.1")
        .arg("false")
        .env_clear()
        .env("LANG", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

fn parse_known_host(value: &[u8]) -> Result<HostIdentity, SshError> {
    let value = std::str::from_utf8(value).map_err(|_| SshError::ProofFailed)?;
    let line = value.strip_suffix('\n').ok_or(SshError::ProofFailed)?;
    if line.contains(['\r', '\n', '\0']) {
        return Err(SshError::ProofFailed);
    }
    let mut fields = line.split_ascii_whitespace();
    if fields.next() != Some("owlmux-target") {
        return Err(SshError::ProofFailed);
    }
    let algorithm = fields.next().ok_or(SshError::ProofFailed)?;
    let encoded_key = fields.next().ok_or(SshError::ProofFailed)?;
    if fields.next().is_some() {
        return Err(SshError::ProofFailed);
    }
    let public_key = PublicKey::from_openssh(&format!("{algorithm} {encoded_key}"))
        .map_err(|_| SshError::ProofFailed)?;
    if public_key.algorithm() != Algorithm::Ed25519 {
        return Err(SshError::ProofFailed);
    }
    Ok(HostIdentity {
        public_key: public_key.to_openssh().map_err(|_| SshError::ProofFailed)?,
        fingerprint_sha256: public_key.fingerprint(HashAlg::Sha256).to_string(),
    })
}

/// Verify the configured Deployment credential against one confirmed loopback target identity.
///
/// # Errors
///
/// Returns a bounded custody, transport, proof, state, or fencing error.
pub async fn verify_access(
    state: &Arc<ServerState>,
    machine_id: Uuid,
    host_identity: &str,
    relay_stream: DuplexStream,
) -> Result<(), SshError> {
    state.lease.check().map_err(|_| SshError::Fenced)?;
    if !is_ed25519_host_identity(host_identity) {
        return Err(SshError::InvalidState);
    }
    let row = sqlx::query(
        "SELECT m.target_account, m.ssh_credential_id, c.encrypted_private_envelope FROM machines m JOIN ssh_credentials c ON c.id = m.ssh_credential_id WHERE m.id = $1 AND m.lifecycle = 'verifying' AND c.status = 'active'",
    )
    .bind(machine_id)
    .fetch_optional(state.database.ordinary())
    .await
    .map_err(|_| SshError::Unavailable)?
    .ok_or(SshError::InvalidState)?;
    state.lease.check().map_err(|_| SshError::Fenced)?;
    let account: String = row
        .try_get("target_account")
        .map_err(|_| SshError::Unavailable)?;
    if !is_target_account(&account) {
        return Err(SshError::InvalidState);
    }
    let credential_id: Uuid = row
        .try_get("ssh_credential_id")
        .map_err(|_| SshError::Unavailable)?;
    let envelope: Vec<u8> = row
        .try_get("encrypted_private_envelope")
        .map_err(|_| SshError::Unavailable)?;
    let private_key = crypto::open(
        state.config.encryption_key(),
        state.database.deployment_id(),
        credential_id,
        &envelope,
    )
    .map_err(|_| SshError::Custody)?;

    state.lease.check().map_err(|_| SshError::Fenced)?;
    let child_dir = state.ssh.child_dir()?;
    verify_with_child(
        &state.lease,
        child_dir.path(),
        &account,
        host_identity,
        &private_key,
        relay_stream,
    )
    .await
}

async fn verify_with_child(
    lease: &NodeLease,
    child_dir: &Path,
    account: &str,
    host_identity: &str,
    private_key: &[u8],
    relay_stream: tokio::io::DuplexStream,
) -> Result<(), SshError> {
    lease.check().map_err(|_| SshError::Fenced)?;
    let identity_path = child_dir.join("identity");
    let known_hosts_path = child_dir.join("known_hosts");
    write_exclusive(&identity_path, private_key, 0o600).await?;
    let known_hosts = format!("owlmux-target {host_identity}\n");
    write_exclusive(&known_hosts_path, known_hosts.as_bytes(), 0o600).await?;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| SshError::Unavailable)?;
    let port = listener
        .local_addr()
        .map_err(|_| SshError::Unavailable)?
        .port();
    let bridge = spawn_abort_on_drop_bridge(listener, relay_stream);

    lease.check().map_err(|_| SshError::Fenced)?;
    let mut child = Command::new("ssh")
        .arg("-F")
        .arg("/dev/null")
        .arg("-oBatchMode=yes")
        .arg("-oIdentitiesOnly=yes")
        .arg("-oIdentityAgent=none")
        .arg("-oPasswordAuthentication=no")
        .arg("-oKbdInteractiveAuthentication=no")
        .arg("-oHostKeyAlgorithms=ssh-ed25519")
        .arg("-oStrictHostKeyChecking=yes")
        .arg(format!(
            "-oUserKnownHostsFile={}",
            known_hosts_path.display()
        ))
        .arg("-oGlobalKnownHostsFile=/dev/null")
        .arg("-oHostKeyAlias=owlmux-target")
        .arg("-oRequestTTY=no")
        .arg("-oClearAllForwardings=yes")
        .arg("-oPermitLocalCommand=no")
        .arg("-oConnectTimeout=10")
        .arg("-i")
        .arg(&identity_path)
        .arg("-p")
        .arg(port.to_string())
        .arg("--")
        .arg(format!("{account}@127.0.0.1"))
        .arg("printf '%s\\n' '__OWLMUX_SSH_OK_V1__'")
        .env_clear()
        .env("LANG", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| SshError::Unavailable)?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or(SshError::Unavailable)?
        .take(MAX_DIAGNOSTIC_READ);
    let mut stderr = child
        .stderr
        .take()
        .ok_or(SshError::Unavailable)?
        .take(MAX_DIAGNOSTIC_READ);
    let proof = timeout(SSH_TIMEOUT, async {
        let mut marker = [0_u8; PROOF_MARKER.len()];
        stdout
            .read_exact(&mut marker)
            .await
            .map_err(|_| SshError::ProofFailed)?;
        if marker != PROOF_MARKER {
            return Err(SshError::ProofFailed);
        }
        tokio::fs::remove_file(&identity_path)
            .await
            .map_err(|_| SshError::Custody)?;

        let mut trailing_output = Vec::new();
        let mut diagnostic = Vec::new();
        let (_, _, status) = tokio::join!(
            stdout.read_to_end(&mut trailing_output),
            stderr.read_to_end(&mut diagnostic),
            child.wait()
        );
        let status = status.map_err(|_| SshError::ProofFailed)?;
        if !trailing_output.is_empty()
            || diagnostic.len() > MAX_DIAGNOSTIC_BYTES
            || !status.success()
        {
            return Err(SshError::ProofFailed);
        }
        Ok(())
    })
    .await
    .map_err(|_| SshError::ProofFailed)?;
    bridge.abort();
    proof
}

async fn write_exclusive(path: &Path, bytes: &[u8], mode: u32) -> Result<(), SshError> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true).mode(mode);
    let mut file = options.open(path).await.map_err(|_| SshError::Custody)?;
    file.write_all(bytes).await.map_err(|_| SshError::Custody)?;
    file.sync_all().await.map_err(|_| SshError::Custody)
}

pub struct ProbeOutput {
    pub stdout: String,
}

pub struct ControlChild {
    lease: Arc<NodeLease>,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    pending_line: Vec<u8>,
    bridge: AbortOnDropHandle<io::Result<()>>,
    stderr: JoinHandle<()>,
    identity_path: Option<PathBuf>,
    _child_dir: ChildDirectory,
}

impl ControlChild {
    /// Read one bounded tmux control-mode line and unlink the identity at the first valid record.
    ///
    /// # Errors
    ///
    /// Returns a custody, transport, or framing error.
    pub async fn next_line(&mut self) -> Result<Vec<u8>, SshError> {
        self.lease.check().map_err(|_| SshError::Fenced)?;
        let line = read_bounded_line_into(
            &mut self.stdout,
            &mut self.pending_line,
            MAX_CONTROL_LINE_BYTES,
        )
        .await?;
        if line.starts_with(b"%begin ")
            && let Some(identity_path) = self.identity_path.take()
        {
            tokio::fs::remove_file(identity_path)
                .await
                .map_err(|_| SshError::Custody)?;
        }
        Ok(line)
    }

    /// Send one closed, bounded tmux control command.
    ///
    /// # Errors
    ///
    /// Returns a state or transport error for invalid or undeliverable commands.
    pub async fn send_command(&mut self, command: &str) -> Result<(), SshError> {
        self.lease.check().map_err(|_| SshError::Fenced)?;
        if command.len() > 4096 || command.contains(['\r', '\n', '\0']) {
            return Err(SshError::InvalidState);
        }
        self.stdin
            .write_all(command.as_bytes())
            .await
            .map_err(|_| SshError::Unavailable)?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(|_| SshError::Unavailable)?;
        self.stdin.flush().await.map_err(|_| SshError::Unavailable)
    }
}

async fn read_bounded_line<R>(reader: &mut R, max_bytes: usize) -> Result<Vec<u8>, SshError>
where
    R: AsyncBufRead + Unpin,
{
    read_bounded_line_into(reader, &mut Vec::new(), max_bytes).await
}

async fn read_bounded_line_into<R>(
    reader: &mut R,
    pending: &mut Vec<u8>,
    max_bytes: usize,
) -> Result<Vec<u8>, SshError>
where
    R: AsyncBufRead + Unpin,
{
    loop {
        let buffer = reader.fill_buf().await.map_err(|_| SshError::Unavailable)?;
        if buffer.is_empty() {
            return Err(SshError::Unavailable);
        }
        let take = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |position| position + 1);
        if pending
            .len()
            .checked_add(take)
            .is_none_or(|size| size > max_bytes)
        {
            return Err(SshError::ProofFailed);
        }
        let complete = buffer[take - 1] == b'\n';
        pending.extend_from_slice(&buffer[..take]);
        reader.consume(take);
        if complete {
            return Ok(std::mem::take(pending));
        }
    }
}

impl Drop for ControlChild {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        self.bridge.abort();
        self.stderr.abort();
    }
}

/// Run the fixed tmux capability and complete-session-list probe.
///
/// # Errors
///
/// Returns a bounded SSH custody, transport, target, or fencing error.
pub async fn run_tmux_probe(
    state: &Arc<ServerState>,
    machine_id: Uuid,
    relay_stream: DuplexStream,
) -> Result<ProbeOutput, SshError> {
    let access = load_active_access(state, machine_id).await?;
    let tmux = shell_literal(&access.tmux_path)?;
    let socket = shell_literal(&access.tmux_socket_identity)?;
    let remote_command = format!(
        "v=$({tmux} -V) || exit 42; printf '__OWLMUX_TMUX_CLIENT_V1__\\t%s\\n' \"$v\"; if sv=$({tmux} -L {socket} display-message -p 'tmux #{{version}}' 2>/dev/null); then printf '__OWLMUX_TMUX_SERVER_V1__\\t%s\\n' \"$sv\"; {tmux} -L {socket} list-sessions -F '#{{session_id}}:#{{session_created}}:#{{session_attached}}:#{{session_windows}}:#{{session_name}}'; else printf '__OWLMUX_TMUX_SERVER_V1__\\tnone\\n'; fi"
    );
    let child_dir = state.ssh.child_dir()?;
    let (mut child, bridge, identity_path) = spawn_access(
        &state.lease,
        child_dir.path(),
        &access,
        &remote_command,
        relay_stream,
    )
    .await?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or(SshError::Unavailable)?
        .take(256 * 1024 + 1);
    let mut stderr = child
        .stderr
        .take()
        .ok_or(SshError::Unavailable)?
        .take(MAX_DIAGNOSTIC_READ);
    let result = timeout(SSH_TIMEOUT, async {
        let mut output = Vec::new();
        let mut diagnostic = Vec::new();
        let (_, _, status) = tokio::join!(
            stdout.read_to_end(&mut output),
            stderr.read_to_end(&mut diagnostic),
            child.wait()
        );
        let status = status.map_err(|_| SshError::Unavailable)?;
        if output.len() > 256 * 1024 || diagnostic.len() > MAX_DIAGNOSTIC_BYTES {
            return Err(SshError::ProofFailed);
        }
        if !output.starts_with(b"__OWLMUX_TMUX_CLIENT_V1__\t") {
            return Err(SshError::ProofFailed);
        }
        tokio::fs::remove_file(&identity_path)
            .await
            .map_err(|_| SshError::Custody)?;
        if !status.success() {
            return Err(SshError::ProofFailed);
        }
        let stdout = String::from_utf8(output).map_err(|_| SshError::ProofFailed)?;
        Ok(ProbeOutput { stdout })
    })
    .await
    .map_err(|_| SshError::ProofFailed)?;
    bridge.abort();
    result
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreateSessionOutcome {
    Succeeded,
    Failed,
    Ambiguous,
}

/// Create one target-owned tmux session through a fixed noninteractive entry operation.
///
/// # Errors
///
/// Returns before dispatch when the Machine, name, SSH custody, or route is invalid. Once the
/// remote operation may have started, transport uncertainty is returned as `Ambiguous` and is
/// never retried.
pub async fn create_tmux_session(
    state: &Arc<ServerState>,
    machine_id: Uuid,
    name: &str,
    relay_stream: DuplexStream,
) -> Result<CreateSessionOutcome, SshError> {
    if name.is_empty()
        || name.len() > 64
        || name.trim() != name
        || name.chars().any(char::is_control)
    {
        return Err(SshError::InvalidState);
    }
    let access = load_active_access(state, machine_id).await?;
    let tmux = shell_literal(&access.tmux_path)?;
    let socket = shell_literal(&access.tmux_socket_identity)?;
    let name = shell_literal(name)?;
    let remote_command = format!(
        "created=$({tmux} -L {socket} new-session -d -P -F '#{{session_id}}:#{{session_created}}' -s {name}) || exit 46; printf '__OWLMUX_TMUX_CREATED_V1__\\t%s\\n' \"$created\""
    );
    let child_dir = state.ssh.child_dir()?;
    let (mut child, bridge, identity_path) = spawn_access(
        &state.lease,
        child_dir.path(),
        &access,
        &remote_command,
        relay_stream,
    )
    .await?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or(SshError::Unavailable)?
        .take(MAX_DIAGNOSTIC_READ);
    let mut stderr = child
        .stderr
        .take()
        .ok_or(SshError::Unavailable)?
        .take(MAX_DIAGNOSTIC_READ);
    let result = timeout(SSH_TIMEOUT, async {
        let mut output = Vec::new();
        let mut diagnostic = Vec::new();
        let (stdout_result, stderr_result, status) = tokio::join!(
            stdout.read_to_end(&mut output),
            stderr.read_to_end(&mut diagnostic),
            child.wait()
        );
        let Ok(status) = status else {
            return CreateSessionOutcome::Ambiguous;
        };
        if stdout_result.is_err()
            || stderr_result.is_err()
            || output.len() > MAX_DIAGNOSTIC_BYTES
            || diagnostic.len() > MAX_DIAGNOSTIC_BYTES
        {
            return CreateSessionOutcome::Ambiguous;
        }
        if !status.success() {
            return if status.code() == Some(46) {
                CreateSessionOutcome::Failed
            } else {
                CreateSessionOutcome::Ambiguous
            };
        }
        let Ok(output) = std::str::from_utf8(&output) else {
            return CreateSessionOutcome::Ambiguous;
        };
        let Some(identity) = output
            .strip_prefix(CREATE_MARKER)
            .and_then(|value| value.strip_suffix('\n'))
        else {
            return CreateSessionOutcome::Ambiguous;
        };
        let Some((session_id, created)) = identity.split_once(':') else {
            return CreateSessionOutcome::Ambiguous;
        };
        let Ok(created) = created.parse::<i64>() else {
            return CreateSessionOutcome::Ambiguous;
        };
        if !is_tmux_session_id(session_id) || created <= 0 {
            return CreateSessionOutcome::Ambiguous;
        }
        if tokio::fs::remove_file(&identity_path).await.is_err() {
            return CreateSessionOutcome::Ambiguous;
        }
        CreateSessionOutcome::Succeeded
    })
    .await
    .unwrap_or(CreateSessionOutcome::Ambiguous);
    bridge.abort();
    Ok(result)
}

/// Start one tmux control-mode attachment for an observed session ID.
///
/// # Errors
///
/// Returns a bounded SSH custody, transport, target, or fencing error.
pub async fn start_tmux_control(
    state: &Arc<ServerState>,
    machine_id: Uuid,
    session_id: &str,
    session_created: i64,
    relay_stream: DuplexStream,
) -> Result<ControlChild, SshError> {
    if !is_tmux_session_id(session_id) || session_created <= 0 {
        return Err(SshError::InvalidState);
    }
    let access = load_active_access(state, machine_id).await?;
    let tmux = shell_literal(&access.tmux_path)?;
    let socket = shell_literal(&access.tmux_socket_identity)?;
    let session = shell_literal(session_id)?;
    let expected_created = session_created.to_string();
    let remote_command = format!(
        "actual=$({tmux} -L {socket} display-message -p -t {session} '#{{session_created}}') || exit 44; [ \"$actual\" = {expected_created} ] || exit 45; printf '__OWLMUX_TMUX_SESSION_OK_V1__\\n'; exec {tmux} -L {socket} -C attach-session -E -f 'read-only,ignore-size' -t {session}"
    );
    let child_dir = state.ssh.child_dir()?;
    let (mut child, bridge, identity_path) = spawn_access(
        &state.lease,
        child_dir.path(),
        &access,
        &remote_command,
        relay_stream,
    )
    .await?;
    let stdin = child.stdin.take().ok_or(SshError::Unavailable)?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or(SshError::Unavailable)?);
    let marker = timeout(
        Duration::from_secs(5),
        read_bounded_line(&mut stdout, SESSION_MARKER.len()),
    )
    .await
    .map_err(|_| SshError::InvalidState)??;
    if marker != SESSION_MARKER {
        return Err(SshError::InvalidState);
    }
    let mut stderr = child
        .stderr
        .take()
        .ok_or(SshError::Unavailable)?
        .take(MAX_DIAGNOSTIC_READ);
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes).await;
    });
    Ok(ControlChild {
        lease: state.lease.clone(),
        child,
        stdin,
        stdout,
        pending_line: Vec::new(),
        bridge,
        stderr: stderr_task,
        identity_path: Some(identity_path),
        _child_dir: child_dir,
    })
}

struct ActiveAccess {
    account: String,
    host_identity: String,
    tmux_path: String,
    tmux_socket_identity: String,
    private_key: zeroize::Zeroizing<Vec<u8>>,
}

async fn load_active_access(
    state: &Arc<ServerState>,
    machine_id: Uuid,
) -> Result<ActiveAccess, SshError> {
    state.lease.check().map_err(|_| SshError::Fenced)?;
    let row = sqlx::query(
        "SELECT m.target_account, m.host_identity, m.tmux_path, m.tmux_socket_identity, m.ssh_credential_id, c.encrypted_private_envelope FROM machines m JOIN ssh_credentials c ON c.id = m.ssh_credential_id WHERE m.id = $1 AND m.lifecycle = 'active' AND c.status = 'active'",
    )
    .bind(machine_id)
    .fetch_optional(state.database.ordinary())
    .await
    .map_err(|_| SshError::Unavailable)?
    .ok_or(SshError::InvalidState)?;
    state.lease.check().map_err(|_| SshError::Fenced)?;
    let credential_id: Uuid = row
        .try_get("ssh_credential_id")
        .map_err(|_| SshError::Unavailable)?;
    let envelope: Vec<u8> = row
        .try_get("encrypted_private_envelope")
        .map_err(|_| SshError::Unavailable)?;
    let private_key = crypto::open(
        state.config.encryption_key(),
        state.database.deployment_id(),
        credential_id,
        &envelope,
    )
    .map_err(|_| SshError::Custody)?;
    state.lease.check().map_err(|_| SshError::Fenced)?;
    let account: String = row
        .try_get("target_account")
        .map_err(|_| SshError::Unavailable)?;
    let host_identity: String = row
        .try_get("host_identity")
        .map_err(|_| SshError::Unavailable)?;
    if !is_target_account(&account) || !is_ed25519_host_identity(&host_identity) {
        return Err(SshError::InvalidState);
    }
    Ok(ActiveAccess {
        account,
        host_identity,
        tmux_path: row
            .try_get("tmux_path")
            .map_err(|_| SshError::Unavailable)?,
        tmux_socket_identity: row
            .try_get("tmux_socket_identity")
            .map_err(|_| SshError::Unavailable)?,
        private_key,
    })
}

async fn spawn_access(
    lease: &NodeLease,
    child_dir: &Path,
    access: &ActiveAccess,
    remote_command: &str,
    relay_stream: DuplexStream,
) -> Result<(Child, AbortOnDropHandle<io::Result<()>>, PathBuf), SshError> {
    lease.check().map_err(|_| SshError::Fenced)?;
    let identity_path = child_dir.join("identity");
    let known_hosts_path = child_dir.join("known_hosts");
    write_exclusive(&identity_path, &access.private_key, 0o600).await?;
    let known_hosts = format!("owlmux-target {}\n", access.host_identity);
    write_exclusive(&known_hosts_path, known_hosts.as_bytes(), 0o600).await?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| SshError::Unavailable)?;
    let port = listener
        .local_addr()
        .map_err(|_| SshError::Unavailable)?
        .port();
    let bridge = spawn_abort_on_drop_bridge(listener, relay_stream);
    if lease.check().is_err() {
        bridge.abort();
        return Err(SshError::Fenced);
    }
    let Ok(child) = command_for_access(
        access,
        &identity_path,
        &known_hosts_path,
        port,
        remote_command,
    )
    .spawn() else {
        bridge.abort();
        return Err(SshError::Unavailable);
    };
    Ok((child, bridge, identity_path))
}

fn command_for_access(
    access: &ActiveAccess,
    identity_path: &Path,
    known_hosts_path: &Path,
    port: u16,
    remote_command: &str,
) -> Command {
    let mut command = Command::new("ssh");
    command
        .arg("-F")
        .arg("/dev/null")
        .arg("-oBatchMode=yes")
        .arg("-oIdentitiesOnly=yes")
        .arg("-oIdentityAgent=none")
        .arg("-oPasswordAuthentication=no")
        .arg("-oKbdInteractiveAuthentication=no")
        .arg("-oHostKeyAlgorithms=ssh-ed25519")
        .arg("-oStrictHostKeyChecking=yes")
        .arg(format!(
            "-oUserKnownHostsFile={}",
            known_hosts_path.display()
        ))
        .arg("-oGlobalKnownHostsFile=/dev/null")
        .arg("-oHostKeyAlias=owlmux-target")
        .arg("-oRequestTTY=no")
        .arg("-oClearAllForwardings=yes")
        .arg("-oPermitLocalCommand=no")
        .arg("-oConnectTimeout=10")
        .arg("-i")
        .arg(identity_path)
        .arg("-p")
        .arg(port.to_string())
        .arg("--")
        .arg(format!("{}@127.0.0.1", access.account))
        .arg(remote_command)
        .env_clear()
        .env("LANG", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

pub(crate) fn is_target_account(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && value.len() <= 64
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

pub(crate) fn is_ed25519_host_identity(value: &str) -> bool {
    PublicKey::from_openssh(value)
        .is_ok_and(|public_key| public_key.algorithm() == Algorithm::Ed25519)
}

fn shell_literal(value: &str) -> Result<String, SshError> {
    if value.is_empty() || value.contains('\0') {
        return Err(SshError::InvalidState);
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

fn is_tmux_session_id(value: &str) -> bool {
    value.strip_prefix('$').is_some_and(|digits| {
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SshError {
    Custody,
    Unavailable,
    InvalidState,
    ProofFailed,
    Fenced,
}

impl std::fmt::Display for SshError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Custody => "SSH private runtime custody failed",
            Self::Unavailable => "SSH proof transport is unavailable",
            Self::InvalidState => "Machine is not ready for SSH proof",
            Self::ProofFailed => "SSH access proof failed",
            Self::Fenced => "Server incarnation is fenced",
        })
    }
}

impl std::error::Error for SshError {}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    fn private_directory(path: &Path) {
        std::fs::create_dir(path).expect("create directory");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .expect("set directory mode");
    }

    fn private_file(path: &Path) {
        std::fs::write(path, b"residue").expect("write file");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .expect("set file mode");
    }

    #[tokio::test]
    async fn control_lines_are_bounded_before_an_oversized_allocation() {
        let input = b"bounded\nnext\n";
        let mut reader = BufReader::with_capacity(3, &input[..]);
        assert_eq!(
            read_bounded_line(&mut reader, 8)
                .await
                .expect("bounded line"),
            b"bounded\n"
        );

        let oversized = vec![b'x'; 9];
        let mut reader = BufReader::with_capacity(2, oversized.as_slice());
        assert!(matches!(
            read_bounded_line(&mut reader, 8).await,
            Err(SshError::ProofFailed)
        ));
    }

    #[tokio::test]
    async fn partial_control_line_survives_a_cancelled_read() {
        let (mut input, output) = tokio::io::duplex(64);
        let mut reader = BufReader::new(output);
        let mut pending = Vec::new();
        input
            .write_all(b"%layout-")
            .await
            .expect("write first fragment");

        assert!(
            timeout(
                Duration::from_millis(10),
                read_bounded_line_into(&mut reader, &mut pending, 64),
            )
            .await
            .is_err()
        );
        assert_eq!(pending, b"%layout-");

        input
            .write_all(b"change @1 deadbeef\n")
            .await
            .expect("write second fragment");
        assert_eq!(
            read_bounded_line_into(&mut reader, &mut pending, 64)
                .await
                .expect("complete fragmented line"),
            b"%layout-change @1 deadbeef\n"
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn target_identity_fields_are_closed_before_ssh_argv() {
        assert!(is_target_account("owlmux"));
        assert!(is_target_account("build.user-1"));
        for invalid in [
            "",
            "-oProxyCommand=touch",
            "user name",
            "user@host",
            "user/other",
        ] {
            assert!(!is_target_account(invalid), "{invalid}");
        }

        let private_key = ssh_key::PrivateKey::random(&mut rand_core::OsRng, Algorithm::Ed25519)
            .expect("generate host fixture");
        let host_identity = private_key
            .public_key()
            .to_openssh()
            .expect("encode host fixture");
        assert!(is_ed25519_host_identity(&host_identity));
        assert!(!is_ed25519_host_identity("ssh-rsa invalid"));

        let access = ActiveAccess {
            account: "owlmux".to_owned(),
            host_identity,
            tmux_path: "/usr/bin/tmux".to_owned(),
            tmux_socket_identity: "owlmux".to_owned(),
            private_key: zeroize::Zeroizing::new(Vec::new()),
        };
        let command = command_for_access(
            &access,
            Path::new("/tmp/identity"),
            Path::new("/tmp/known_hosts"),
            2222,
            "fixed-command",
        );
        let arguments = command
            .as_std()
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let terminator = arguments
            .iter()
            .position(|value| value == "--")
            .expect("--");
        let destination = arguments
            .iter()
            .position(|value| value == "owlmux@127.0.0.1")
            .expect("destination");
        assert!(terminator < destination);
        assert!(
            arguments
                .iter()
                .any(|value| value == "-oHostKeyAlgorithms=ssh-ed25519")
        );
        assert!(
            arguments
                .iter()
                .any(|value| value == "-oStrictHostKeyChecking=yes")
        );
    }

    #[test]
    fn host_discovery_command_is_credential_free_and_accept_new_only() {
        let command = command_for_host_discovery(Path::new("/tmp/known_hosts"), 2222);
        let arguments = command
            .as_std()
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        for required in [
            "-oPubkeyAuthentication=no",
            "-oPreferredAuthentications=none",
            "-oStrictHostKeyChecking=accept-new",
            "-oHostKeyAlgorithms=ssh-ed25519",
            "-oUpdateHostKeys=no",
            "owlmux-host-key-preflight-invalid@127.0.0.1",
        ] {
            assert!(arguments.iter().any(|value| value == required));
        }
        assert!(!arguments.iter().any(|value| value == "-i"));
    }

    #[test]
    fn known_host_parser_accepts_one_canonical_ed25519_key_only() {
        let private_key = ssh_key::PrivateKey::random(&mut rand_core::OsRng, Algorithm::Ed25519)
            .expect("generate host fixture");
        let public_key = private_key
            .public_key()
            .to_openssh()
            .expect("encode host fixture");
        let known_host = format!("owlmux-target {public_key}\n");
        let parsed = parse_known_host(known_host.as_bytes()).expect("parse known host");
        assert_eq!(parsed.public_key, public_key);
        assert_eq!(
            parsed.fingerprint_sha256,
            private_key
                .public_key()
                .fingerprint(HashAlg::Sha256)
                .to_string()
        );
        assert!(parse_known_host(known_host.trim_end().as_bytes()).is_err());
        assert!(parse_known_host(format!("other-target {public_key}\n").as_bytes()).is_err());
        assert!(parse_known_host(format!("{known_host}{known_host}").as_bytes()).is_err());
        assert!(parse_known_host(b"owlmux-target ssh-rsa invalid\n").is_err());
    }

    #[test]
    fn runtime_scavenges_only_valid_private_residue() {
        let root = std::env::temp_dir().join(format!("owlmux-ssh-test-{}", Uuid::new_v4()));
        private_directory(&root);
        let instance = root.join(format!("instance-{}", Uuid::new_v4()));
        private_directory(&instance);
        let child = instance.join(format!("child-{}", Uuid::new_v4()));
        private_directory(&child);
        private_file(&child.join("identity"));
        private_file(&child.join("known_hosts"));

        let runtime = SshRuntime::create(&root).expect("create runtime");
        assert!(!instance.exists());
        assert!(matches!(SshRuntime::create(&root), Err(SshError::Custody)));
        drop(runtime);
        std::fs::remove_file(root.join(".lock")).expect("remove lock");
        std::fs::remove_dir(root).expect("remove root");
    }

    #[test]
    fn runtime_fails_closed_on_linked_residue() {
        let root = std::env::temp_dir().join(format!("owlmux-ssh-test-{}", Uuid::new_v4()));
        private_directory(&root);
        let outside = std::env::temp_dir().join(format!("owlmux-ssh-outside-{}", Uuid::new_v4()));
        private_directory(&outside);
        let linked = root.join(format!("instance-{}", Uuid::new_v4()));
        symlink(&outside, &linked).expect("create symlink");

        assert!(matches!(SshRuntime::create(&root), Err(SshError::Custody)));
        std::fs::remove_file(linked).expect("remove symlink");
        std::fs::remove_file(root.join(".lock")).expect("remove lock");
        std::fs::remove_dir(root).expect("remove root");
        std::fs::remove_dir(outside).expect("remove outside");
    }
}
