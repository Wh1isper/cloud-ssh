#!/usr/bin/env python3
from __future__ import annotations

import re
from pathlib import Path
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
LINK_PATTERN = re.compile(r"(?<!!)\[[^\]]*\]\(([^)]+)\)")
SKIPPED_PREFIXES = ("http://", "https://", "mailto:", "#")


def markdown_files() -> list[Path]:
    files = [ROOT / "README.md", ROOT / "SECURITY.md"]
    files.extend(sorted((ROOT / "docs").rglob("*.md")))
    files.extend(sorted((ROOT / "spec").rglob("*.md")))
    return [path for path in files if path.is_file()]


def link_candidates(source: Path, target: str) -> tuple[Path, ...]:
    if target.startswith("/"):
        base = ROOT / "docs" / target.lstrip("/")
    else:
        base = source.parent / target
    if base.suffix:
        return (base,)
    return (base, base.with_suffix(".md"), base / "index.md")


def main() -> int:
    failures: list[str] = []
    for path in markdown_files():
        content = path.read_text(encoding="utf-8")
        for match in LINK_PATTERN.finditer(content):
            raw = match.group(1).strip().split(maxsplit=1)[0].strip("<>")
            if not raw or raw.startswith(SKIPPED_PREFIXES):
                continue
            parsed = urlsplit(raw)
            target = unquote(parsed.path)
            if not any(candidate.resolve().exists() for candidate in link_candidates(path, target)):
                line = content.count("\n", 0, match.start()) + 1
                failures.append(f"{path.relative_to(ROOT)}:{line}: missing link target {raw}")

    if failures:
        print("\n".join(failures))
        return 1

    print(f"Checked Markdown links in {len(markdown_files())} files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
