use std::{
    fs::{File, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::SigningKey;
use rand_core::RngCore as _;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroize as _;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RelayState {
    pub protocol: u16,
    pub relay_id: Uuid,
    secret_key: String,
    pub target_account: String,
    pub deployment_id: Option<Uuid>,
    pub machine_id: Option<Uuid>,
    pub route_revision: Option<i64>,
}

impl RelayState {
    pub fn load_or_create(path: &Path, target_account: &str) -> Result<Self, StateError> {
        if path.exists() {
            let state = Self::load(path)?;
            if state.target_account != target_account {
                return Err(StateError::Invalid);
            }
            return Ok(state);
        }
        validate_account(target_account)?;
        let mut secret = [0_u8; 32];
        rand_core::OsRng.fill_bytes(&mut secret);
        let state = Self {
            protocol: 1,
            relay_id: Uuid::new_v4(),
            secret_key: URL_SAFE_NO_PAD.encode(secret),
            target_account: target_account.to_owned(),
            deployment_id: None,
            machine_id: None,
            route_revision: None,
        };
        secret.zeroize();
        state.persist(path)?;
        Ok(state)
    }

    pub fn load(path: &Path) -> Result<Self, StateError> {
        let metadata = std::fs::symlink_metadata(path).map_err(|_| StateError::Io)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.permissions().mode() & 0o777 != 0o600
        {
            return Err(StateError::UnsafePermissions);
        }
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| StateError::Io)?
            .take(16 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| StateError::Io)?;
        if bytes.len() > 16 * 1024 {
            return Err(StateError::Invalid);
        }
        let state: Self = serde_json::from_slice(&bytes).map_err(|_| StateError::Invalid)?;
        if state.protocol != 1 {
            return Err(StateError::Invalid);
        }
        validate_account(&state.target_account)?;
        let _ = state.signing_key()?;
        Ok(state)
    }

    pub fn reset_identity(&mut self, path: &Path) -> Result<(), StateError> {
        let mut secret = [0_u8; 32];
        rand_core::OsRng.fill_bytes(&mut secret);
        self.relay_id = Uuid::new_v4();
        self.secret_key = URL_SAFE_NO_PAD.encode(secret);
        self.deployment_id = None;
        self.machine_id = None;
        self.route_revision = None;
        secret.zeroize();
        self.persist(path)
    }

    pub fn signing_key(&self) -> Result<SigningKey, StateError> {
        let mut decoded = URL_SAFE_NO_PAD
            .decode(&self.secret_key)
            .map_err(|_| StateError::Invalid)?;
        let secret: [u8; 32] = decoded
            .as_slice()
            .try_into()
            .map_err(|_| StateError::Invalid)?;
        if URL_SAFE_NO_PAD.encode(secret) != self.secret_key {
            decoded.zeroize();
            return Err(StateError::Invalid);
        }
        let key = SigningKey::from_bytes(&secret);
        decoded.zeroize();
        Ok(key)
    }

    pub fn persist(&self, path: &Path) -> Result<(), StateError> {
        let parent = path
            .parent()
            .filter(|value| !value.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|_| StateError::Io)?;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| StateError::Io)?;
        }
        let parent_metadata = std::fs::symlink_metadata(parent).map_err(|_| StateError::Io)?;
        if !parent_metadata.is_dir()
            || parent_metadata.file_type().is_symlink()
            || parent_metadata.permissions().mode() & 0o077 != 0
        {
            return Err(StateError::UnsafePermissions);
        }
        let temporary = temporary_path(path);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|_| StateError::Io)?;
        let encoded = serde_json::to_vec(self).map_err(|_| StateError::Invalid)?;
        file.write_all(&encoded).map_err(|_| StateError::Io)?;
        file.sync_all().map_err(|_| StateError::Io)?;
        drop(file);
        if std::fs::rename(&temporary, path).is_err() {
            let _ = std::fs::remove_file(&temporary);
            return Err(StateError::Io);
        }
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| StateError::Io)
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("relay-state");
    path.with_file_name(format!(".{name}.{}.tmp", Uuid::new_v4()))
}

fn validate_account(account: &str) -> Result<(), StateError> {
    if account.is_empty()
        || account.len() > 64
        || account
            .chars()
            .any(|value| value.is_whitespace() || value.is_control())
    {
        return Err(StateError::Invalid);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateError {
    Io,
    Invalid,
    UnsafePermissions,
}

impl std::fmt::Display for StateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Io => "Relay state file operation failed",
            Self::Invalid => "Relay state file is invalid",
            Self::UnsafePermissions => "Relay state path permissions are unsafe",
        })
    }
}
impl std::error::Error for StateError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_is_created_exclusively_and_round_trips() {
        let root = std::env::temp_dir().join(format!("owlmux-relay-test-{}", Uuid::new_v4()));
        let path = root.join("state.json");
        let state = RelayState::load_or_create(&path, "owlmux").expect("create");
        let loaded = RelayState::load(&path).expect("load");
        assert_eq!(loaded.relay_id, state.relay_id);
        assert_eq!(
            loaded.signing_key().expect("key").verifying_key(),
            state.signing_key().expect("key").verifying_key()
        );
        std::fs::remove_dir_all(root).expect("cleanup");
    }
}
