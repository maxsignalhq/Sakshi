#!/usr/bin/env python3
"""Fill the package-manager templates for one release.

    python3 packaging/render.py --version 0.1.0 --sums dist/SHA256SUMS --out dist/manifests

Reads the release's SHA256SUMS, substitutes the version and each archive's
checksum into every ``*.in`` template under packaging/, and writes the results
(without the ``.in``) under the output directory. Fails loudly on a missing
checksum or a placeholder left unfilled, so a broken manifest is never emitted.
"""

import argparse
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent

# Release targets, in the order the workflow builds them.
TARGETS = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
]

PLACEHOLDER = re.compile(r"@@([A-Z0-9_]+)@@")


def archive_name(version: str, target: str) -> str:
    extension = "zip" if "windows" in target else "tar.gz"
    return f"sakshi-v{version}-{target}.{extension}"


def parse_sums(text: str) -> dict:
    """Map file name to lowercase SHA-256 from `sha256sum`-style lines."""
    sums = {}
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        digest, _, name = line.partition(" ")
        name = name.strip().lstrip("*")
        if not re.fullmatch(r"[0-9a-fA-F]{64}", digest) or not name:
            raise ValueError(f"unreadable checksum line: {line!r}")
        sums[name] = digest.lower()
    return sums


def placeholder_values(version: str, sums: dict) -> dict:
    values = {"VERSION": version}
    for target in TARGETS:
        name = archive_name(version, target)
        if name not in sums:
            raise ValueError(f"no checksum for {name}")
        values["SHA256_" + target.upper().replace("-", "_")] = sums[name]
    return values


def render_text(template: str, values: dict, source: str) -> str:
    def fill(match):
        key = match.group(1)
        if key not in values:
            raise ValueError(f"{source}: unknown placeholder @@{key}@@")
        return values[key]

    text = PLACEHOLDER.sub(fill, template)
    if PLACEHOLDER.search(text):
        raise ValueError(f"{source}: placeholder left unfilled")
    return text


def render(version: str, sums_text: str, templates_dir: Path = HERE) -> dict:
    """Return {relative output path: rendered text} for every template."""
    version = version.removeprefix("v")
    values = placeholder_values(version, parse_sums(sums_text))
    rendered = {}
    for template in sorted(templates_dir.rglob("*.in")):
        relative = template.relative_to(templates_dir).with_suffix("")
        text = render_text(template.read_text(encoding="utf-8"), values, str(template))
        if relative.suffix == ".json":
            json.loads(text)  # a Scoop manifest that is not JSON is useless
        rendered[relative.as_posix()] = text
    if not rendered:
        raise ValueError(f"no templates found under {templates_dir}")
    return rendered


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--version", required=True, help="release version, e.g. 0.1.0")
    parser.add_argument("--sums", required=True, type=Path, help="path to SHA256SUMS")
    parser.add_argument("--out", required=True, type=Path, help="output directory")
    args = parser.parse_args()
    try:
        rendered = render(args.version, args.sums.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        print(f"render.py: {error}", file=sys.stderr)
        return 1
    for relative, text in rendered.items():
        path = args.out / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8", newline="\n")
        print(f"wrote {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
