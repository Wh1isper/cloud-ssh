use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use subtle::ConstantTimeEq as _;
use zeroize::Zeroizing;

const API_KEY_PREFIX: &str = "owlmux_sk_v1_";

pub struct ApiKey(Zeroizing<[u8; 32]>);

impl ApiKey {
    /// Parse the only accepted Deployment API-key representation.
    ///
    /// # Errors
    ///
    /// Returns an opaque error for absent, malformed, noncanonical, or wrong-length values.
    pub fn parse(value: &str) -> Result<Self, SecretFormatError> {
        let payload = value
            .strip_prefix(API_KEY_PREFIX)
            .ok_or(SecretFormatError)?;
        Ok(Self(Zeroizing::new(decode_canonical_32(payload)?)))
    }

    #[must_use]
    pub fn verify(&self, candidate: &str) -> bool {
        let Ok(candidate) = Self::parse(candidate) else {
            return false;
        };
        bool::from(self.0.as_ref().ct_eq(candidate.0.as_ref()))
    }
}

pub(crate) fn decode_canonical_32(value: &str) -> Result<[u8; 32], SecretFormatError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| SecretFormatError)?;
    if decoded.len() != 32 || URL_SAFE_NO_PAD.encode(&decoded) != value {
        return Err(SecretFormatError);
    }
    decoded.try_into().map_err(|_| SecretFormatError)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SecretFormatError;

impl std::fmt::Display for SecretFormatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid secret format")
    }
}

impl std::error::Error for SecretFormatError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> String {
        format!("{API_KEY_PREFIX}{}", URL_SAFE_NO_PAD.encode([byte; 32]))
    }

    #[test]
    fn accepts_only_canonical_api_keys() {
        let expected = ApiKey::parse(&key(7)).expect("key");
        assert!(expected.verify(&key(7)));
        assert!(!expected.verify(&key(8)));
        assert!(ApiKey::parse("wrong").is_err());
        assert!(ApiKey::parse(&format!("{}=", key(7))).is_err());
        assert!(ApiKey::parse(&format!("{API_KEY_PREFIX}AA")).is_err());
    }
}
