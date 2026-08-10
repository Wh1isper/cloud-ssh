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
REQUIRED = (
    "AGENTS.md",
    "Cargo.toml",
    "Dockerfile",
    "README.md",
    "apps/web/package.json",
    "crates/owlmux-relay/Cargo.toml",
    "crates/owlmux-server/Cargo.toml",
    "docs/wrangler.jsonc",
    "spec/README.md",
)
TEXT_SUFFIXES = {
    "",
    ".css",
    ".html",
    ".js",
    ".json",
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


def main() -> int:
    failures: list[str] = []

    for required in REQUIRED:
        if not (ROOT / required).is_file():
            failures.append(f"missing required file: {required}")

    for path in sorted(ROOT.rglob("*")):
        if not is_checked_file(path):
            continue
        relative = path.relative_to(ROOT)
        try:
            content = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        for pattern in FORBIDDEN:
            if pattern.search(str(relative)) or pattern.search(content):
                failures.append(f"legacy product reference in {relative}: {pattern.pattern}")
                break

    if failures:
        print("\n".join(failures))
        return 1

    print("Repository boundary check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
