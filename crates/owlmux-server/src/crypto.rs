use chacha20poly1305::{
    KeyInit as _, XChaCha20Poly1305, XNonce,
    aead::{Aead as _, AeadCore as _, OsRng, Payload},
};
use ssh_key::{Algorithm, HashAlg, LineEnding, PrivateKey};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::auth::{SecretFormatError, decode_canonical_32};

const ENVELOPE_VERSION: u8 = 1;
const ENVELOPE_DOMAIN: &[u8] = b"owlmux:ssh-private-key:v1\0";
const NONCE_LENGTH: usize = 24;

pub struct EncryptionKey(Zeroizing<[u8; 32]>);

impl EncryptionKey {
    /// Parse the canonical unpadded base64url encryption key.
    ///
    /// # Errors
    ///
    /// Returns an opaque error for malformed, noncanonical, or wrong-length values.
    pub fn parse(value: &str) -> Result<Self, SecretFormatError> {
        Ok(Self(Zeroizing::new(decode_canonical_32(value)?)))
    }
}

pub struct GeneratedCredential {
    pub public_key: String,
    pub public_fingerprint_sha256: String,
    pub encrypted_private_envelope: Vec<u8>,
}

/// Generate and seal one Ed25519 OpenSSH credential.
///
/// # Errors
///
/// Returns an opaque custody error when generation, serialization, or encryption fails.
pub fn generate_credential(
    key: &EncryptionKey,
    deployment_id: Uuid,
    credential_id: Uuid,
) -> Result<GeneratedCredential, CryptoError> {
    let private_key =
        PrivateKey::random(&mut rand_core::OsRng, Algorithm::Ed25519).map_err(|_| CryptoError)?;
    let private_openssh = private_key
        .to_openssh(LineEnding::LF)
        .map_err(|_| CryptoError)?;
    let public_key = private_key
        .public_key()
        .to_openssh()
        .map_err(|_| CryptoError)?;
    let public_fingerprint_sha256 = private_key
        .public_key()
        .fingerprint(HashAlg::Sha256)
        .to_string();
    let encrypted_private_envelope = seal(
        key,
        deployment_id,
        credential_id,
        private_openssh.as_bytes(),
    )?;

    Ok(GeneratedCredential {
        public_key,
        public_fingerprint_sha256,
        encrypted_private_envelope,
    })
}

fn associated_data(deployment_id: Uuid, credential_id: Uuid) -> Vec<u8> {
    let mut value = Vec::with_capacity(ENVELOPE_DOMAIN.len() + 32);
    value.extend_from_slice(ENVELOPE_DOMAIN);
    value.extend_from_slice(deployment_id.as_bytes());
    value.extend_from_slice(credential_id.as_bytes());
    value
}

fn seal(
    key: &EncryptionKey,
    deployment_id: Uuid,
    credential_id: Uuid,
    plaintext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = XChaCha20Poly1305::new(key.0.as_ref().into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: &associated_data(deployment_id, credential_id),
            },
        )
        .map_err(|_| CryptoError)?;
    let mut envelope = Vec::with_capacity(1 + NONCE_LENGTH + ciphertext.len());
    envelope.push(ENVELOPE_VERSION);
    envelope.extend_from_slice(&nonce);
    envelope.extend_from_slice(&ciphertext);
    Ok(envelope)
}

/// Open a credential envelope into zeroizing memory.
///
/// # Errors
///
/// Returns an opaque custody error for an unsupported version, malformed envelope, wrong key,
/// tampering, or context substitution.
pub fn open(
    key: &EncryptionKey,
    deployment_id: Uuid,
    credential_id: Uuid,
    envelope: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if envelope.first() != Some(&ENVELOPE_VERSION) || envelope.len() <= 1 + NONCE_LENGTH {
        return Err(CryptoError);
    }
    let nonce = XNonce::from_slice(&envelope[1..=NONCE_LENGTH]);
    let cipher = XChaCha20Poly1305::new(key.0.as_ref().into());
    let plaintext = cipher
        .decrypt(
            nonce,
            Payload {
                msg: &envelope[1 + NONCE_LENGTH..],
                aad: &associated_data(deployment_id, credential_id),
            },
        )
        .map_err(|_| CryptoError)?;
    Ok(Zeroizing::new(plaintext))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CryptoError;

impl std::fmt::Display for CryptoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SSH credential custody operation failed")
    }
}

impl std::error::Error for CryptoError {}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

    use super::*;

    fn key(byte: u8) -> EncryptionKey {
        EncryptionKey::parse(&URL_SAFE_NO_PAD.encode([byte; 32])).expect("key")
    }

    #[test]
    fn envelope_round_trips_only_in_exact_context() {
        let deployment_id = Uuid::new_v4();
        let credential_id = Uuid::new_v4();
        let envelope = seal(&key(3), deployment_id, credential_id, b"private").expect("seal");
        assert_eq!(
            open(&key(3), deployment_id, credential_id, &envelope)
                .expect("open")
                .as_slice(),
            b"private"
        );
        assert!(open(&key(4), deployment_id, credential_id, &envelope).is_err());
        assert!(open(&key(3), Uuid::new_v4(), credential_id, &envelope).is_err());
        assert!(open(&key(3), deployment_id, Uuid::new_v4(), &envelope).is_err());
    }

    #[test]
    fn tampering_and_unknown_versions_fail_closed() {
        let deployment_id = Uuid::new_v4();
        let credential_id = Uuid::new_v4();
        let mut envelope = seal(&key(3), deployment_id, credential_id, b"private").expect("seal");
        envelope[0] = 2;
        assert!(open(&key(3), deployment_id, credential_id, &envelope).is_err());
        envelope[0] = 1;
        let last = envelope.len() - 1;
        envelope[last] ^= 1;
        assert!(open(&key(3), deployment_id, credential_id, &envelope).is_err());
    }
}
