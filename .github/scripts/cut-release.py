#!/usr/bin/env python3
"""Prepare Cargo.toml for a release.

Drops the [patch.crates-io] block the sbpf-linker-next branch carries, so that the release
builds the published crates, and sets [package].version.

Usage: cut-release.py <version>
"""

import sys

import tomlkit


def main() -> int:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <version>", file=sys.stderr)
        return 2

    version = sys.argv[1].strip()

    with open("Cargo.toml") as f:
        doc = tomlkit.parse(f.read())

    patch = doc.get("patch")
    if patch is None or "crates-io" not in patch:
        print(
            "::error::Cargo.toml has no [patch.crates-io] block, so this branch does not "
            "track sbpf master",
            file=sys.stderr,
        )
        return 1

    del patch["crates-io"]
    doc["package"]["version"] = version

    with open("Cargo.toml", "w") as f:
        f.write(tomlkit.dumps(doc))

    print(f"sbpf-linker {version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
