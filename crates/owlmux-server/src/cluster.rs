use hmac::{Hmac, Mac as _};
use rand_core::{OsRng, RngCore as _};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    auth::{SecretFormatError, decode_canonical_32},
    generated::contracts::{
        INTERNAL_AUTH_TIMEOUT_SECONDS, INTERNAL_CONFIGURATION_DOMAIN,
        INTERNAL_CONFIGURATION_VERSION, INTERNAL_CONTRACT_VERSION, INTERNAL_MAX_FRAME_BYTES,
    },
};

const OWNER_AUTH_DOMAIN: &[u8] = b"owlmux:owner-wss-auth:v1\0";
const SCHEMA_GENERATION: &[u8] = b"3";
const PUBLIC_GENERATION: &[u8] = b"public.v1";
const RELAY_GENERATION: &[u8] = b"relay.v1";
const ENVELOPE_GENERATION: &[u8] = b"ssh-envelope.v1";
const ORIGIN_POLICY: &[u8] = b"exact";

type HmacSha256 = Hmac<Sha256>;

pub struct ClusterKey(Zeroizing<[u8; 32]>);

impl ClusterKey {
    /// Parse the only accepted cluster-key representation.
    ///
    /// # Errors
    ///
    /// Returns an opaque error for malformed, noncanonical, or wrong-length input.
    pub fn parse(value: &str) -> Result<Self, SecretFormatError> {
        Ok(Self(Zeroizing::new(decode_canonical_32(value)?)))
    }

    pub(crate) fn configuration_digest(&self) -> [u8; 32] {
        Sha256::digest(self.0.as_ref()).into()
    }

    pub(crate) fn configuration_proof(&self, input: &ConfigurationInput<'_>) -> [u8; 32] {
        let mut transcript = Vec::with_capacity(320);
        transcript.extend_from_slice(INTERNAL_CONFIGURATION_DOMAIN.as_bytes());
        append_field(&mut transcript, INTERNAL_CONFIGURATION_VERSION.as_bytes());
        append_field(&mut transcript, input.deployment_id.as_bytes());
        append_field(&mut transcript, &input.config_epoch.to_be_bytes());
        append_field(&mut transcript, input.server_build_id.as_bytes());
        append_field(&mut transcript, &input.api_key_digest);
        append_field(&mut transcript, &input.encryption_key_digest);
        append_field(&mut transcript, input.public_origin.as_bytes());
        append_field(&mut transcript, ORIGIN_POLICY);
        append_field(&mut transcript, SCHEMA_GENERATION);
        append_field(&mut transcript, PUBLIC_GENERATION);
        append_field(&mut transcript, RELAY_GENERATION);
        append_field(&mut transcript, INTERNAL_CONTRACT_VERSION.as_bytes());
        append_field(&mut transcript, ENVELOPE_GENERATION);
        append_field(
            &mut transcript,
            INTERNAL_AUTH_TIMEOUT_SECONDS.to_string().as_bytes(),
        );
        append_field(
            &mut transcript,
            INTERNAL_MAX_FRAME_BYTES.to_string().as_bytes(),
        );
        self.sign(&transcript)
    }

    pub(crate) fn owner_response(&self, context: &OwnerAuthContext) -> [u8; 32] {
        self.sign(&context.transcript())
    }

    pub(crate) fn verify_owner_response(
        &self,
        context: &OwnerAuthContext,
        candidate: &[u8],
    ) -> bool {
        let Ok(mut mac) = HmacSha256::new_from_slice(self.0.as_ref()) else {
            return false;
        };
        mac.update(&context.transcript());
        mac.verify_slice(candidate).is_ok()
    }

    fn sign(&self, value: &[u8]) -> [u8; 32] {
        let mut mac = HmacSha256::new_from_slice(self.0.as_ref())
            .expect("HMAC accepts a 32-byte cluster key");
        mac.update(value);
        mac.finalize().into_bytes().into()
    }
}

pub(crate) struct ConfigurationInput<'a> {
    pub deployment_id: Uuid,
    pub config_epoch: i64,
    pub server_build_id: &'a str,
    pub api_key_digest: [u8; 32],
    pub encryption_key_digest: [u8; 32],
    pub public_origin: &'a str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionClass {
    Attachment,
    Control,
}

impl ConnectionClass {
    const fn label(self) -> &'static [u8] {
        match self {
            Self::Attachment => b"attachment",
            Self::Control => b"control",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct OwnerAuthContext {
    pub deployment_id: Uuid,
    pub config_epoch: i64,
    pub source_incarnation_id: Uuid,
    pub destination_incarnation_id: Uuid,
    pub machine_id: Uuid,
    pub route_revision: i64,
    pub connection_epoch: i64,
    pub connection_class: ConnectionClass,
    pub challenge: [u8; 32],
    pub source_nonce: [u8; 32],
    pub trace_id: Uuid,
}

impl OwnerAuthContext {
    fn transcript(self) -> Vec<u8> {
        let mut transcript = Vec::with_capacity(256);
        transcript.extend_from_slice(OWNER_AUTH_DOMAIN);
        append_field(&mut transcript, INTERNAL_CONTRACT_VERSION.as_bytes());
        append_field(&mut transcript, self.connection_class.label());
        append_field(&mut transcript, self.deployment_id.as_bytes());
        append_field(&mut transcript, &self.config_epoch.to_be_bytes());
        append_field(&mut transcript, self.source_incarnation_id.as_bytes());
        append_field(&mut transcript, self.destination_incarnation_id.as_bytes());
        append_field(&mut transcript, self.machine_id.as_bytes());
        append_field(&mut transcript, &self.route_revision.to_be_bytes());
        append_field(&mut transcript, &self.connection_epoch.to_be_bytes());
        append_field(&mut transcript, b"api_key_verified");
        append_field(&mut transcript, &self.challenge);
        append_field(&mut transcript, &self.source_nonce);
        append_field(&mut transcript, self.trace_id.as_bytes());
        transcript
    }
}

pub(crate) fn random_32() -> [u8; 32] {
    let mut value = [0_u8; 32];
    OsRng.fill_bytes(&mut value);
    value
}

fn append_field(target: &mut Vec<u8>, value: &[u8]) {
    let length = u32::try_from(value.len()).expect("fixed cluster transcript field is bounded");
    target.extend_from_slice(&length.to_be_bytes());
    target.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

    use super::*;

    fn key(byte: u8) -> ClusterKey {
        ClusterKey::parse(&URL_SAFE_NO_PAD.encode([byte; 32])).expect("cluster key")
    }

    #[test]
    fn configuration_proof_binds_shared_security_configuration() {
        let deployment_id = Uuid::new_v4();
        let input = ConfigurationInput {
            deployment_id,
            config_epoch: 2,
            server_build_id: "build-a",
            api_key_digest: [1; 32],
            encryption_key_digest: [2; 32],
            public_origin: "https://owlmux.example",
        };
        let proof = key(3).configuration_proof(&input);
        assert_eq!(proof, key(3).configuration_proof(&input));
        assert_ne!(proof, key(4).configuration_proof(&input));
        let changed = ConfigurationInput {
            public_origin: "https://other.example",
            ..input
        };
        assert_ne!(proof, key(3).configuration_proof(&changed));
    }

    #[test]
    fn owner_response_binds_every_route_authority_field() {
        let context = OwnerAuthContext {
            deployment_id: Uuid::new_v4(),
            config_epoch: 2,
            source_incarnation_id: Uuid::new_v4(),
            destination_incarnation_id: Uuid::new_v4(),
            machine_id: Uuid::new_v4(),
            route_revision: 3,
            connection_epoch: 4,
            connection_class: ConnectionClass::Attachment,
            challenge: [5; 32],
            source_nonce: [6; 32],
            trace_id: Uuid::new_v4(),
        };
        let response = key(7).owner_response(&context);
        assert!(key(7).verify_owner_response(&context, &response));
        assert!(!key(8).verify_owner_response(&context, &response));
        let stale = OwnerAuthContext {
            connection_epoch: 5,
            ..context
        };
        assert!(!key(7).verify_owner_response(&stale, &response));
    }
}
