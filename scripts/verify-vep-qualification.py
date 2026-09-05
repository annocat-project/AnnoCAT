#!/usr/bin/env python3
"""Fail closed when the checked-in VEP 115 qualification inputs drift."""

import argparse
import hashlib
import importlib.util
import json
from collections import Counter
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "fixtures" / "fastvep"


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def document(path):
    return json.loads(path.read_text(encoding="utf-8"))


def vcf_summary(path):
    records = 0
    alleles = 0
    identifiers = []
    saw_header = False
    with path.open(encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, 1):
            if line.startswith("#CHROM"):
                saw_header = True
            if line.startswith("#"):
                continue
            columns = line.rstrip("\r\n").split("\t")
            if len(columns) < 8:
                raise ValueError(f"{path}:{line_number}: invalid VCF row")
            records += 1
            alleles += len(columns[4].split(","))
            identifiers.append(columns[2])
    if not saw_header or records == 0:
        raise ValueError(f"{path}: missing header or records")
    return {"records": records, "alleles": alleles, "identifiers": identifiers}


def verify_corpus(manifest_path):
    manifest = document(manifest_path)
    output = manifest["output"]
    vcf = ROOT / output["path"]
    summary = vcf_summary(vcf)
    if sha256(vcf) != output["sha256"]:
        raise ValueError(f"{vcf}: SHA-256 mismatch")
    if summary["records"] != output["records"]:
        raise ValueError(f"{vcf}: record count mismatch")
    if "alleles" in output and summary["alleles"] != output["alleles"]:
        raise ValueError(f"{vcf}: allele count mismatch")
    for generator in [manifest.get("generator")]:
        if generator:
            path = ROOT / generator["path"]
            if sha256(path) != generator["sha256"]:
                raise ValueError(f"{path}: generator SHA-256 mismatch")
    selected = manifest.get("selectedVariantIds")
    if selected is not None and Counter(map(str, selected)) != Counter(summary["identifiers"]):
        raise ValueError(f"{manifest_path}: selected variant IDs do not match the VCF")
    if manifest["corpusId"] == "ensembl-115-boundary":
        expected = [f"BOUNDARY{index}" for index in range(1, summary["records"] + 1)]
        if summary["identifiers"] != expected:
            raise ValueError(f"{vcf}: boundary identifiers are not complete and ordered")
    return {
        "manifest": str(manifest_path.relative_to(ROOT)),
        "manifestSha256": sha256(manifest_path),
        "vcf": str(vcf.relative_to(ROOT)),
        "vcfSha256": output["sha256"],
        "records": summary["records"],
        "alleles": summary["alleles"],
    }


def verify_field_contract():
    path = FIXTURES / "ensembl-115-gff-gff-contract.json"
    contract = document(path)
    fields = contract.get("fields", [])
    names = [item.get("name") for item in fields]
    comparator_path = ROOT / "scripts" / "compare-vep-concordance.py"
    spec = importlib.util.spec_from_file_location("vep_concordance_comparator", comparator_path)
    comparator = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(comparator)
    if (
        contract.get("schemaVersion") != 2
        or tuple(names) != comparator.PRODUCTION_FIELDS
        or len(names) != len(set(names))
    ):
        raise ValueError(f"{path}: expected 49 unique production fields")
    if any(item.get("disposition") not in {"exact", "input-derived", "source-derived", "excluded"} for item in fields):
        raise ValueError(f"{path}: invalid field disposition")
    return {"path": str(path.relative_to(ROOT)), "sha256": sha256(path), "fields": len(names)}


def verify_consequence_contract():
    path = FIXTURES / "ensembl-115-consequences.json"
    contract = document(path)
    terms = contract.get("terms", [])
    ranks = [item.get("rank") for item in terms]
    names = [item.get("term") for item in terms]
    if ranks != list(range(1, 42)) or len(names) != len(set(names)):
        raise ValueError(f"{path}: expected the ordered 41-term VEP inventory")
    allowed = {"pending-source-matched-qualification", "qualified", "excluded"}
    if any(item.get("status") not in allowed for item in terms):
        raise ValueError(f"{path}: invalid qualification status")
    if any(item.get("status") == "excluded" and not item.get("reason") for item in terms):
        raise ValueError(f"{path}: excluded terms require reasons")
    return {"path": str(path.relative_to(ROOT)), "sha256": sha256(path), "terms": len(names)}


def verify_cache_contract():
    path = FIXTURES / "previous-cache-compatibility.json"
    contract = document(path)
    releases = contract.get("releases", [])
    if not releases:
        raise ValueError(f"{path}: no supported-release entries")
    for release in releases:
        status = release.get("status")
        expected_hash = release.get("expectedCacheSha256")
        expected_bytes = release.get("expectedCacheBytes")
        if status == "enrolled":
            if not isinstance(expected_hash, str) or len(expected_hash) != 64:
                raise ValueError(f"{path}: enrolled cache requires a SHA-256")
            if not isinstance(expected_bytes, int) or expected_bytes < 1:
                raise ValueError(f"{path}: enrolled cache requires a positive size")
        elif status == "enrollment-required":
            if expected_hash is not None or expected_bytes is not None:
                raise ValueError(f"{path}: unenrolled cache cannot declare expected bytes")
        else:
            raise ValueError(f"{path}: invalid cache support status {status!r}")
    return {
        "path": str(path.relative_to(ROOT)),
        "sha256": sha256(path),
        "releases": len(releases),
        "allEnrolled": all(item["status"] == "enrolled" for item in releases),
    }


def verify_sources():
    catalog = document(ROOT / "config" / "source-catalog.json")
    resources = {item["id"]: item for item in catalog["resources"]}
    reference = resources["grch38-reference"]["release"]["checksum"]
    semantics = document(FIXTURES / "ensembl-115-semantics.json")
    if reference.get("algorithm") != "sha256":
        raise ValueError("GRCh38 source catalog does not use SHA-256")
    if reference.get("value") != semantics["sources"]["referenceArchiveSha256"]:
        raise ValueError("GRCh38 source catalog and VEP contract disagree")
    return {
        "semanticContract": "fixtures/fastvep/ensembl-115-semantics.json",
        "semanticContractSha256": sha256(FIXTURES / "ensembl-115-semantics.json"),
        "referenceArchiveSha256": reference["value"],
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--json", type=Path)
    parser.add_argument("--require-enrolled-caches", action="store_true")
    args = parser.parse_args()

    corpora = [
        verify_corpus(FIXTURES / "ensembl-115-legacy-manifest.json"),
        verify_corpus(FIXTURES / "ensembl-115-boundary-manifest.json"),
        verify_corpus(FIXTURES / "ensembl-115-clinvar-reviewed-manifest.json"),
    ]
    report = {
        "schemaVersion": 1,
        "sources": verify_sources(),
        "fieldContract": verify_field_contract(),
        "consequenceContract": verify_consequence_contract(),
        "cacheCompatibility": verify_cache_contract(),
        "corpora": corpora,
    }
    report["passed"] = not (
        args.require_enrolled_caches and not report["cacheCompatibility"]["allEnrolled"]
    )
    rendered = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.json:
        args.json.write_text(rendered, encoding="utf-8")
    print(rendered, end="")
    if not report["passed"]:
        raise SystemExit("previous-release cache enrollment is incomplete")


if __name__ == "__main__":
    main()
