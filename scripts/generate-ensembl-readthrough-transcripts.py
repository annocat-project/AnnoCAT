#!/usr/bin/env python3
"""Generate AnnoCAT's Ensembl 115 readthrough-transcript exclusion list."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import re
from pathlib import Path


SOURCE_BYTES = 93_374_019
SOURCE_SHA256 = "d6e6fe0515c95b2a8cd36a853c1989cee9115c736c60237c56ae92b9daaaf7c4"
OUTPUT_COUNT = 2_115
OUTPUT_BYTES = 33_840
OUTPUT_SHA256 = "78a58d8855af0a1ac2807b9ed45fedb061f936a37916faf054cacebe46ed63ef"
VERSIONED_TRANSCRIPT_ID = re.compile(r"^(ENST[0-9]+)\.([0-9]+)$")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def attributes(value: str) -> list[tuple[str, str]]:
    parsed = []
    for field in value.split(";"):
        field = field.strip()
        if not field:
            continue
        key, separator, raw = field.partition(" ")
        raw = raw.strip()
        if not separator or not raw:
            raise ValueError(f"invalid GTF attribute: {field}")
        if raw.startswith('"') or raw.endswith('"'):
            if len(raw) < 2 or raw[0] != '"' or raw[-1] != '"':
                raise ValueError(f"invalid quoted GTF attribute: {field}")
            raw = raw[1:-1]
        parsed.append((key, raw))
    return parsed


def generate(source: Path) -> bytes:
    if source.stat().st_size != SOURCE_BYTES or sha256(source) != SOURCE_SHA256:
        raise SystemExit("input is not the pinned GENCODE 49 comprehensive GTF")

    versioned_by_stable: dict[str, str] = {}
    with gzip.open(source, "rt", encoding="utf-8", newline="") as gtf:
        for line_number, line in enumerate(gtf, 1):
            if line.startswith("#"):
                continue
            fields = line.rstrip("\n").split("\t")
            if len(fields) != 9 or fields[2] != "transcript":
                continue
            try:
                parsed_attributes = attributes(fields[8])
            except ValueError as error:
                raise SystemExit(f"{error} at line {line_number}") from error
            if ("tag", "readthrough_transcript") not in parsed_attributes:
                continue
            transcript_ids = [value for key, value in parsed_attributes if key == "transcript_id"]
            match = VERSIONED_TRANSCRIPT_ID.fullmatch(transcript_ids[0]) if len(transcript_ids) == 1 else None
            if match is None:
                raise SystemExit(f"tagged transcript at line {line_number} has no versioned ENST ID")
            stable_id = match.group(1)
            versioned_id = f"{stable_id}.{match.group(2)}"
            previous = versioned_by_stable.setdefault(stable_id, versioned_id)
            if previous != versioned_id:
                raise SystemExit(f"multiple versioned IDs normalize to {stable_id}")

    if len(versioned_by_stable) != OUTPUT_COUNT:
        raise SystemExit(
            f"expected {OUTPUT_COUNT} readthrough transcripts, found {len(versioned_by_stable)}"
        )
    output = ("\n".join(sorted(versioned_by_stable)) + "\n").encode("ascii")
    output_sha256 = hashlib.sha256(output).hexdigest()
    if len(output) != OUTPUT_BYTES or output_sha256 != OUTPUT_SHA256:
        raise SystemExit(
            "generated list does not match its pinned byte identity: "
            f"{len(output)} bytes, SHA-256 {output_sha256}"
        )
    return output


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path, help="pinned gencode.v49.annotation.gtf.gz")
    parser.add_argument("output", type=Path, help="output stable-ID list")
    parser.add_argument("--check", action="store_true", help="verify output instead of writing it")
    args = parser.parse_args()

    generated = generate(args.source)
    if args.check:
        if not args.output.is_file() or args.output.read_bytes() != generated:
            raise SystemExit(f"{args.output} does not match the generated list")
    else:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_bytes(generated)
    print(f"verified {OUTPUT_COUNT} transcripts: {OUTPUT_SHA256}")


if __name__ == "__main__":
    main()
