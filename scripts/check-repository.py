#!/usr/bin/env python3
from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXCLUDED_PARTS = {
    ".git",
    ".wrangler",
    "node_modules",
    "target",
}
EXCLUDED_PREFIXES = (
    ("docs", ".vitepress", "cache"),
    ("docs", ".vitepress", "dist"),
)
LEGACY_PRODUCT = "cloud" + r"[-_ ]ssh"
LEGACY_REPOSITORY = r"github\.com/(?:wh1isper|owlfoundry)/" + "cloud" + "-ssh"
LEGACY_IDENTIFIER = "cloud" + "_ssh"
FORBIDDEN = tuple(
    re.compile(pattern, re.IGNORECASE)
    for pattern in (LEGACY_PRODUCT, LEGACY_REPOSITORY, LEGACY_IDENTIFIER)
)
PLAN_LABEL_IN_PATH_PART = re.compile(
    r"(?:^|[-_.])(?:blocks?|milestones?|phases?)[-_.]?[0-9a-d](?:[-_.]|$)",
    re.IGNORECASE,
)
PLAN_LABEL_IN_SOURCE = re.compile(
    r"\b(?:blocks?|milestones?)\s*[0-9]+(?:\s*[-–]\s*[0-9]+)?\b|\bphase\s+[a-d]\b",
    re.IGNORECASE,
)
PLANNING_DOCUMENT_ROOTS = {"local-reference", "spec"}
REQUIRED = (
    "AGENTS.md",
    "Cargo.toml",
    "Dockerfile",
    "README.md",
    "apps/web/package.json",
    "crates/owlmux-relay/Cargo.toml",
    "crates/owlmux-relay/LICENSE",
    "crates/owlmux-server/Cargo.toml",
    "crates/owlmux-server/LICENSE",
    "docs/wrangler.jsonc",
    "scripts/release/publish-github-release.sh",
    "scripts/release/publish-server-image.sh",
    "spec/README.md",
)
TEXT_SUFFIXES = {
    "",
    ".css",
    ".html",
    ".js",
    ".json",
    ".mjs",
    ".jsonc",
    ".md",
    ".py",
    ".rs",
    ".sh",
    ".svg",
    ".toml",
    ".ts",
    ".tsx",
    ".vue",
    ".yaml",
    ".yml",
}


def is_checked_file(path: Path) -> bool:
    relative = path.relative_to(ROOT)
    excluded_prefix = any(
        relative.parts[: len(prefix)] == prefix for prefix in EXCLUDED_PREFIXES
    )
    return (
        path.is_file()
        and not EXCLUDED_PARTS.intersection(relative.parts)
        and not excluded_prefix
        and path.suffix in TEXT_SUFFIXES
    )


def path_has_plan_label(relative: Path) -> bool:
    return any(PLAN_LABEL_IN_PATH_PART.search(part) for part in relative.parts)


def scanner_self_check() -> list[str]:
    path_cases = (
        (Path("scripts") / ("block" + "-4.sh"), True),
        (Path("scripts") / ("milestone" + "-2") / "test.sh", True),
        (Path("scripts") / ("phase" + "-a") / "test.sh", True),
        (Path("scripts") / "docker" / "e2e-single-node.sh", False),
    )
    failures = [
        "implementation-plan path scanner self-check failed"
        for path, expected in path_cases
        if path_has_plan_label(path) != expected
    ]
    source_cases = (("Block" + " 4", True), ("single-node", False))
    failures.extend(
        "implementation-plan source scanner self-check failed"
        for value, expected in source_cases
        if bool(PLAN_LABEL_IN_SOURCE.search(value)) != expected
    )
    return failures


def main() -> int:
    failures = scanner_self_check()

    for required in REQUIRED:
        if not (ROOT / required).is_file():
            failures.append(f"missing required file: {required}")

    root_license = (ROOT / "LICENSE").read_bytes()
    for relative in ("crates/owlmux-server/LICENSE", "crates/owlmux-relay/LICENSE"):
        packaged_license = ROOT / relative
        if packaged_license.is_file() and packaged_license.read_bytes() != root_license:
            failures.append(f"packaged license is stale: {relative}")

    for path in sorted(ROOT.rglob("*")):
        relative = path.relative_to(ROOT)
        excluded_path = bool(EXCLUDED_PARTS.intersection(relative.parts)) or any(
            relative.parts[: len(prefix)] == prefix for prefix in EXCLUDED_PREFIXES
        )
        if not excluded_path and path_has_plan_label(relative):
            failures.append(f"implementation-plan label in repository path: {relative}")
        if not is_checked_file(path):
            continue
        try:
            content = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        for pattern in FORBIDDEN:
            if pattern.search(str(relative)) or pattern.search(content):
                failures.append(f"legacy product reference in {relative}: {pattern.pattern}")
                break
        if (
            relative.parts[0] not in PLANNING_DOCUMENT_ROOTS
            and PLAN_LABEL_IN_SOURCE.search(content)
        ):
            failures.append(f"implementation-plan label in product source: {relative}")

    if failures:
        print("\n".join(failures))
        return 1

    print("Repository boundary check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
