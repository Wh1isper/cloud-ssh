#!/usr/bin/env python3
from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

GLIBC_VERSION = re.compile(r"\bGLIBC_(\d+)\.(\d+)\b")


def parse_baseline(value: str) -> tuple[int, int]:
    parts = value.split(".")
    if len(parts) != 2 or not all(part.isdecimal() for part in parts):
        raise ValueError("baseline must be MAJOR.MINOR")
    return int(parts[0]), int(parts[1])


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print(f"usage: {argv[0]} <elf-binary> <maximum-glibc-version>", file=sys.stderr)
        return 2

    binary = Path(argv[1])
    try:
        baseline = parse_baseline(argv[2])
        symbols = subprocess.run(
            ["objdump", "-T", str(binary)],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        print(f"could not inspect {binary.name} glibc baseline: {error}", file=sys.stderr)
        return 1

    required = sorted(
        {(int(major), int(minor)) for major, minor in GLIBC_VERSION.findall(symbols)}
    )
    if not required:
        print(f"{binary.name} exposes no versioned glibc imports", file=sys.stderr)
        return 1
    if required[-1] > baseline:
        print(
            f"{binary.name} requires glibc {required[-1][0]}.{required[-1][1]}, "
            f"above qualified baseline {baseline[0]}.{baseline[1]}",
            file=sys.stderr,
        )
        return 1

    print(
        f"{binary.name} glibc requirement {required[-1][0]}.{required[-1][1]} "
        f"is within baseline {baseline[0]}.{baseline[1]}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
