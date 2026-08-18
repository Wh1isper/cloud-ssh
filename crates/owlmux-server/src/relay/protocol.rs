use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 32_768;
pub const MAX_DATA_BYTES: usize = 16_384;
pub const MAX_STREAMS: usize = 32;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientFrame {
    Token {
        token: String,
    },
    Setup {
        protocol: u16,
        relay_id: Uuid,
        public_key: String,
        endpoint: String,
        observed_account: String,
    },
    Ready,
    Signature {
        signature: String,
    },
    TunnelHello {
        protocol: u16,
        deployment_id: Uuid,
        machine_id: Uuid,
        relay_id: Uuid,
        connection_id: Uuid,
        route_revision: i64,
    },
    StreamOpened {
        stream_id: u32,
    },
    StreamData {
        stream_id: u32,
        data: String,
    },
    StreamHalfClosed {
        stream_id: u32,
    },
    StreamClosed {
        stream_id: u32,
        reason: CloseReason,
    },
    Pong {
        nonce: u64,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    Accepted {
        deployment_id: Uuid,
        machine_id: Uuid,
        attempt_id: Uuid,
        route_revision: i64,
    },
    Credential {
        credential_id: Uuid,
        name: String,
        public_key: String,
        public_fingerprint_sha256: String,
    },
    Challenge {
        purpose: ChallengePurpose,
        nonce: String,
    },
    Activated {
        route_revision: i64,
    },
    TunnelEstablished {
        connection_epoch: i64,
    },
    OpenStream {
        stream_id: u32,
    },
    StreamData {
        stream_id: u32,
        data: String,
    },
    StreamHalfClosed {
        stream_id: u32,
    },
    StreamClosed {
        stream_id: u32,
        reason: CloseReason,
    },
    Ping {
        nonce: u64,
    },
    Error {
        code: &'static str,
        message: &'static str,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    Eof,
    ConnectFailed,
    Protocol,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChallengePurpose {
    Enrollment,
    Tunnel,
}

#[must_use]
pub fn signature_message(
    purpose: ChallengePurpose,
    deployment_id: Uuid,
    machine_id: Uuid,
    relay_id: Uuid,
    connection_id: Option<Uuid>,
    route_revision: i64,
    nonce: &[u8; 32],
) -> Vec<u8> {
    let mut message = Vec::with_capacity(160);
    message.extend_from_slice(b"owlmux:relay-signature:v1\0");
    message.extend_from_slice(match purpose {
        ChallengePurpose::Enrollment => b"enrollment\0",
        ChallengePurpose::Tunnel => b"tunnel\0",
    });
    message.extend_from_slice(deployment_id.as_bytes());
    message.extend_from_slice(machine_id.as_bytes());
    message.extend_from_slice(relay_id.as_bytes());
    if let Some(connection_id) = connection_id {
        message.extend_from_slice(connection_id.as_bytes());
    }
    message.extend_from_slice(&route_revision.to_be_bytes());
    message.extend_from_slice(nonce);
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_are_closed_and_versioned() {
        let setup = include_str!("../../fixtures/relay/setup.json");
        assert!(matches!(
            serde_json::from_str::<ClientFrame>(setup).expect("setup fixture"),
            ClientFrame::Setup { protocol: 1, .. }
        ));
        let unknown = include_str!("../../fixtures/relay/unknown-field.json");
        assert!(serde_json::from_str::<ClientFrame>(unknown).is_err());
        assert_eq!(crate::generated::contracts::RELAY_PROTOCOL_VERSION, VERSION);
        assert_eq!(
            crate::generated::contracts::RELAY_MAX_FRAME_BYTES,
            MAX_FRAME_BYTES
        );
        assert_eq!(
            crate::generated::contracts::RELAY_MAX_DATA_BYTES,
            MAX_DATA_BYTES
        );
        assert_eq!(crate::generated::contracts::RELAY_MAX_STREAMS, MAX_STREAMS);
    }
}
