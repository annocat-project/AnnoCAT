#!/usr/bin/env python3
"""Fetch a reproducible Ensembl 115 VEP REST oracle for a public VCF."""

import argparse
import json
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path


SERVER = "https://sep2025.rest.ensembl.org"
PARAMETERS = {
    "canonical": 1,
    "ccds": 1,
    "hgvs": 1,
    "mane": 1,
    "numbers": 1,
    "protein": 1,
    "tsl": 1,
}
MAX_BATCH = 200


def variants(path):
    rows = []
    with path.open(encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, 1):
            if not line or line.startswith("#"):
                continue
            columns = line.rstrip("\r\n").split("\t")
            if len(columns) < 8:
                raise ValueError(f"{path}:{line_number}: invalid VCF row")
            rows.append(" ".join(columns[:8]))
    if not rows:
        raise ValueError(f"{path}: no variants")
    return rows


def request_json(url, payload=None):
    body = None if payload is None else json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=body,
        headers={
            "Accept": "application/json",
            "Content-Type": "application/json",
            "User-Agent": "AnnoCAT-annotation-concordance",
        },
        method="GET" if body is None else "POST",
    )
    for attempt in range(3):
        try:
            with urllib.request.urlopen(request, timeout=180) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            if error.code not in (429, 500, 502, 503, 504) or attempt == 2:
                raise
            time.sleep(2**attempt)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("input", type=Path)
    parser.add_argument("--response", required=True, type=Path)
    parser.add_argument("--request", required=True, type=Path)
    parser.add_argument("--software", required=True, type=Path)
    args = parser.parse_args()

    software = request_json(f"{SERVER}/info/software")
    if software.get("release") != 115:
        raise SystemExit(f"expected Ensembl release 115, got {software!r}")

    input_variants = variants(args.input)
    endpoint = f"{SERVER}/vep/homo_sapiens/region?{urllib.parse.urlencode(PARAMETERS)}"
    batches = [
        input_variants[index : index + MAX_BATCH]
        for index in range(0, len(input_variants), MAX_BATCH)
    ]
    responses = []
    for batch in batches:
        response = request_json(endpoint, {"variants": batch})
        if not isinstance(response, list):
            raise ValueError("VEP REST response is not an array")
        responses.extend(response)
    if len(responses) != len(input_variants):
        raise ValueError(
            f"VEP returned {len(responses)} records for {len(input_variants)} inputs"
        )

    write_json(
        args.request,
        {
            "server": SERVER,
            "endpoint": "/vep/homo_sapiens/region",
            "parameters": PARAMETERS,
            "batches": [{"variants": batch} for batch in batches],
        },
    )
    write_json(args.response, responses)
    write_json(args.software, software)


if __name__ == "__main__":
    main()
