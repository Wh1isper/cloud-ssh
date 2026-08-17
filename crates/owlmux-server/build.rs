use std::{env, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=OWLMUX_BUILD_REVISION");
    println!("cargo:rerun-if-changed=../../.git/HEAD");

    let revision = env::var("OWLMUX_BUILD_REVISION")
        .ok()
        .or_else(git_revision)
        .filter(|value| is_safe_revision(value))
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=OWLMUX_BUILD_REVISION={revision}");
}

fn git_revision() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_owned())
}

fn is_safe_revision(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}
