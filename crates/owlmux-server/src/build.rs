use serde::Serialize;

#[derive(Clone, Copy, Serialize)]
pub struct BuildMetadata {
    pub id: &'static str,
    pub version: &'static str,
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REVISION: &str = env!("OWLMUX_BUILD_REVISION");
pub const BUILD_ID: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "+",
    env!("OWLMUX_BUILD_REVISION")
);

#[must_use]
pub const fn metadata() -> BuildMetadata {
    BuildMetadata {
        id: BUILD_ID,
        version: VERSION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_metadata_is_bounded_and_opaque() {
        assert!(!BUILD_ID.is_empty());
        assert!(BUILD_ID.len() <= 105);
        assert!(BUILD_ID.bytes().all(|byte| byte.is_ascii_graphic()));
        assert!(!BUILD_ID.contains('/'));
    }
}
