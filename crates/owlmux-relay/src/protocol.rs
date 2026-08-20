use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 32_768;
pub const MAX_DATA_BYTES: usize = 16_384;
pub const MAX_STREAMS: usize = 32;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
    HostKeyAccepted {
        host_identity: String,
    },
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

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerFrame {
    Accepted {
        deployment_id: Uuid,
        machine_id: Uuid,
        #[serde(rename = "attempt_id")]
        _attempt_id: Uuid,
        route_revision: i64,
    },
    Credential {
        credential_id: Uuid,
        name: String,
        public_key: String,
        public_fingerprint_sha256: String,
    },
    HostKey {
        host_identity: String,
        fingerprint_sha256: String,
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
        #[serde(rename = "reason")]
        _reason: CloseReason,
    },
    Ping {
        nonce: u64,
    },
    Error {
        code: String,
        #[serde(rename = "message")]
        _message: String,
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

#[derive(Clone, Copy, Debug, Deserialize)]
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
    fn shared_fixtures_match_local_closed_types() {
        let setup = include_str!("../fixtures/relay/setup.json");
        let value: serde_json::Value = serde_json::from_str(setup).expect("fixture");
        assert_eq!(value["protocol"], VERSION);
        let open = include_str!("../fixtures/relay/open-stream.json");
        assert!(matches!(
            serde_json::from_str::<ServerFrame>(open).expect("open fixture"),
            ServerFrame::OpenStream { stream_id: 1 }
        ));
        let host_key = include_str!("../fixtures/relay/host-key.json");
        assert!(matches!(
            serde_json::from_str::<ServerFrame>(host_key).expect("host-key fixture"),
            ServerFrame::HostKey { .. }
        ));
        let expected: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/relay/host-key-accepted.json"))
                .expect("host-key acceptance fixture");
        let encoded = serde_json::to_value(ClientFrame::HostKeyAccepted {
            host_identity: expected["host_identity"]
                .as_str()
                .expect("host identity")
                .to_owned(),
        })
        .expect("serialize host-key acceptance");
        assert_eq!(encoded, expected);
        let unknown = include_str!("../fixtures/relay/unknown-field.json");
        assert!(serde_json::from_str::<ServerFrame>(unknown).is_err());
    }
}
