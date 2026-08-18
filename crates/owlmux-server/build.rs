use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-env-changed=OWLMUX_BUILD_REVISION");
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let package_vcs_info = manifest_dir.join(".cargo_vcs_info.json");
    if package_vcs_info.is_file() {
        println!("cargo:rerun-if-changed={}", package_vcs_info.display());
    }
    let git_head = manifest_dir.join("../../.git/HEAD");
    if git_head.is_file() {
        println!("cargo:rerun-if-changed={}", git_head.display());
    }

    let revision = env::var("OWLMUX_BUILD_REVISION")
        .ok()
        .or_else(|| package_revision(&package_vcs_info))
        .or_else(git_revision)
        .filter(|value| is_safe_revision(value))
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=OWLMUX_BUILD_REVISION={revision}");
}

fn package_revision(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    contents.lines().find_map(|line| {
        line.trim()
            .strip_prefix("\"sha1\": \"")
            .map(|value| value.strip_suffix(',').unwrap_or(value))
            .and_then(|value| value.strip_suffix('"'))
            .map(str::to_owned)
    })
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
