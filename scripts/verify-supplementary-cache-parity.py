#!/usr/bin/env python3
"""Require OSA1 and OSA2 supplementary annotations to match source contracts."""

import argparse
import difflib
import hashlib
import json
import os
import subprocess
import tempfile
from pathlib import Path


SOURCE_SPECS = {
    "clinvar": ("clinvar", "clinvar.vcf", "clinvar"),
    "gnomad": ("gnomad", "gnomad.vcf", "gnomad"),
    "gnomad-genomes": ("gnomad", "gnomad.vcf", "gnomad"),
    "dbsnp": ("dbsnp", "dbsnp.vcf", "dbsnp"),
    "spliceai": ("spliceai", "spliceai.vcf", "spliceAI"),
    "cadd": ("cadd", "cadd.tsv", "cadd"),
    "phylop": ("phylop", "phylop.tsv", "phylop"),
    "revel": ("revel", "revel.csv", "revel"),
    "dbnsfp": ("dbnsfp", "dbnsfp.tsv", "dbnsfp"),
}
DEFAULT_SOURCES = tuple(
    source for source in SOURCE_SPECS if source != "gnomad-genomes"
)
DBNSFP_FIELDS = json.dumps(
    ["Ensembl_transcriptid", "REVEL_score", "AlphaMissense_score"],
    separators=(",", ":"),
)


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def run(command, env=None):
    result = subprocess.run(
        [str(part) for part in command],
        check=False,
        capture_output=True,
        text=True,
        env=env,
    )
    if result.returncode:
        detail = (result.stderr or result.stdout).strip()
        raise RuntimeError(f"command failed ({result.returncode}): {' '.join(map(str, command))}\n{detail}")
    return result.stdout.strip()


def load_ndjson(path):
    records = {}
    with path.open(encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, 1):
            if not line.strip():
                continue
            record = json.loads(line)
            key = (
                str(record.get("seq_region_name", "")),
                int(record.get("start", 0)),
                str(record.get("allele_string", "")),
            )
            if not all(key) or key in records:
                raise ValueError(f"{path}:{line_number}: invalid or duplicate record identity {key!r}")
            if isinstance(record.get("alleles"), list):
                record["alleles"] = sorted(record["alleles"], key=lambda item: item.get("allele", ""))
            records[key] = record
    if not records:
        raise ValueError(f"{path}: no structured records")
    return dict(sorted(records.items()))


def require_equal(label, expected, actual):
    if expected == actual:
        return
    expected_text = json.dumps(expected, indent=2, sort_keys=True).splitlines()
    actual_text = json.dumps(actual, indent=2, sort_keys=True).splitlines()
    difference = "\n".join(
        list(difflib.unified_diff(expected_text, actual_text, "expected", label, lineterm=""))[:120]
    )
    raise AssertionError(f"{label} differs from the source contract\n{difference}")


def select_sources(records, source_ids):
    keys = {SOURCE_SPECS[source_id][2] for source_id in source_ids}
    selected = {}
    for identity, record in records.items():
        projected = {key: value for key, value in record.items() if key != "alleles"}
        alleles = []
        for allele in record.get("alleles", []):
            selected_allele = {"allele": allele["allele"]}
            selected_allele.update(
                (key, allele[key]) for key in keys if key in allele
            )
            if len(selected_allele) > 1:
                alleles.append(selected_allele)
        if alleles:
            projected["alleles"] = alleles
        selected[identity] = projected
    return selected


def write_ndjson(path, records):
    path.parent.mkdir(parents=True, exist_ok=True)
    lines = (
        json.dumps(record, sort_keys=True, separators=(",", ":"))
        for record in records.values()
    )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def cache_file(directory, source, cache_format):
    path = directory / f"{source}.{cache_format}"
    if not path.is_file():
        raise FileNotFoundError(f"fastVEP did not create {path}")
    return path


def build_and_annotate(fastvep, fixtures, work, cache_format, source_ids):
    cache_directory = work / cache_format
    cache_directory.mkdir()
    environment = os.environ.copy()
    environment["ANNOCAT_DBNSFP_FIELDS"] = DBNSFP_FIELDS
    cache_hashes = {}
    for source_id in source_ids:
        cache_source, filename, _ = SOURCE_SPECS[source_id]
        output = cache_directory / source_id
        run(
            (
                fastvep,
                "sa-build",
                "--source",
                cache_source,
                "--input",
                fixtures / filename,
                "--output",
                output,
                "--format",
                cache_format,
                "--no-progress",
            ),
            environment,
        )
        cache = cache_file(cache_directory, source_id, cache_format)
        run((fastvep, "sa-verify", "--input", cache), environment)
        cache_hashes[cache.name] = sha256(cache)
        index = cache.with_suffix(cache.suffix + ".idx")
        if index.is_file():
            cache_hashes[index.name] = sha256(index)

    structured = work / f"{cache_format}.ndjson"
    output_vcf = work / f"{cache_format}.vcf"
    run(
        (
            fastvep,
            "annotate",
            "--input",
            fixtures / "query.vcf",
            "--output",
            output_vcf,
            "--sa-dir",
            cache_directory,
            "--sa-only",
            "--structured-output",
            structured,
            "--no-progress",
        ),
        environment,
    )
    return load_ndjson(structured), cache_hashes, sha256(structured)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fastvep", required=True, type=Path)
    parser.add_argument(
        "--fixtures",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "fixtures" / "source-cache-parity",
    )
    parser.add_argument("--json", type=Path, help="Write a reproducibility report")
    parser.add_argument(
        "--source",
        action="append",
        choices=tuple(SOURCE_SPECS),
        help="Validate one source contract; repeat to select more than one",
    )
    parser.add_argument(
        "--structured-output",
        type=Path,
        help="Write the verified OSA2 structured output for AnnoCAT projection tests",
    )
    arguments = parser.parse_args()

    fastvep = arguments.fastvep.resolve()
    fixtures = arguments.fixtures.resolve()
    if not fastvep.is_file():
        parser.error(f"fastVEP binary does not exist: {fastvep}")
    expected_path = fixtures / "expected.ndjson"
    source_ids = tuple(arguments.source or DEFAULT_SOURCES)
    if len(set(source_ids)) != len(source_ids):
        parser.error("each source can be selected only once")
    if "gnomad" in source_ids and "gnomad-genomes" in source_ids:
        parser.error("gnomAD exomes and genomes cannot be selected together")
    expected = select_sources(load_ndjson(expected_path), source_ids)

    with tempfile.TemporaryDirectory(prefix="annocat-source-parity-") as temporary:
        work = Path(temporary)
        osa1, osa1_hashes, osa1_output_hash = build_and_annotate(
            fastvep, fixtures, work, "osa", source_ids
        )
        osa2, osa2_hashes, osa2_output_hash = build_and_annotate(
            fastvep, fixtures, work, "osa2", source_ids
        )
        require_equal("OSA1 output", expected, osa1)
        require_equal("OSA2 output", expected, osa2)
        require_equal("OSA2 output", osa1, osa2)
        if arguments.structured_output:
            write_ndjson(arguments.structured_output.resolve(), osa2)

    fixture_hashes = {
        path.name: sha256(path)
        for path in sorted(fixtures.iterdir())
        if path.is_file()
    }
    version = run((fastvep, "--version"))
    report = {
        "schemaVersion": 1,
        "status": "pass",
        "recordCount": len(expected),
        "sourceCount": len(source_ids),
        "sources": list(source_ids),
        "fastvep": {
            "version": version,
            "sha256": sha256(fastvep),
        },
        "fixtures": fixture_hashes,
        "osa": {
            "cacheFiles": osa1_hashes,
            "structuredOutputSha256": osa1_output_hash,
        },
        "osa2": {
            "cacheFiles": osa2_hashes,
            "structuredOutputSha256": osa2_output_hash,
        },
    }
    if arguments.json:
        arguments.json.parent.mkdir(parents=True, exist_ok=True)
        arguments.json.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(
        f"Supplementary cache parity passed: {len(source_ids)} sources, "
        f"{len(expected)} structured records, OSA1 = OSA2 = source contract"
    )


if __name__ == "__main__":
    main()
