#!/usr/bin/env python3
"""Fail unless fastVEP and an Ensembl VEP oracle emit the same consequence rows."""

import argparse
import hashlib
import json
import re
from collections import Counter
from pathlib import Path


FIELDS = (
    "Allele",
    "Consequence",
    "IMPACT",
    "SYMBOL",
    "Gene",
    "Feature_type",
    "Feature",
    "BIOTYPE",
    "EXON",
    "INTRON",
    "HGVSc",
    "HGVSp",
    "cDNA_position",
    "CDS_position",
    "Protein_position",
    "Amino_acids",
    "Codons",
    "DISTANCE",
    "STRAND",
    "FLAGS",
    "CANONICAL",
    "SYMBOL_SOURCE",
    "HGNC_ID",
    "TSL",
    "APPRIS",
    "SOURCE",
)

# The archive database adds HGNC provenance absent from Ensembl GFF3, does not
# expose VEP's source label, and fastVEP's GFF3 loader does not retain APPRIS.
# Everything below has a direct representation in both outputs.
REST_FIELDS = (
    "Allele",
    "Consequence",
    "IMPACT",
    "SYMBOL",
    "Gene",
    "Feature_type",
    "Feature",
    "BIOTYPE",
    "EXON",
    "INTRON",
    "HGVSc",
    "HGVSp",
    "cDNA_position",
    "CDS_position",
    "Protein_position",
    "Amino_acids",
    "Codons",
    "DISTANCE",
    "STRAND",
    "FLAGS",
    "CANONICAL",
    "MANE",
    "TSL",
    "CCDS",
    "ENSP",
)
ALL_FIELDS = tuple(dict.fromkeys(FIELDS + REST_FIELDS))


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def parse_vcf(path, fields=FIELDS):
    csq_fields = None
    variants = Counter()
    annotations = Counter()
    with path.open(encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, 1):
            if line.startswith("##INFO=<ID=CSQ"):
                match = re.search(r'Format: ([^">]+)', line)
                if not match:
                    raise ValueError(f"{path}:{line_number}: CSQ format is missing")
                csq_fields = match.group(1).split("|")
                continue
            if line.startswith("#"):
                continue
            columns = line.rstrip("\n\r").split("\t")
            if len(columns) < 8:
                raise ValueError(f"{path}:{line_number}: invalid VCF row")
            if csq_fields is None:
                raise ValueError(f"{path}: CSQ header is missing")
            key = tuple(columns[index] for index in (0, 1, 3, 4))
            variants[key] += 1
            csq_value = next(
                (item[4:] for item in columns[7].split(";") if item.startswith("CSQ=")),
                "",
            )
            for encoded in filter(None, csq_value.split(",")):
                values = encoded.split("|")
                row = dict(zip(csq_fields, values))
                annotations[(key, tuple(row.get(field, "") for field in fields))] += 1
    if csq_fields is None:
        raise ValueError(f"{path}: CSQ header is missing")
    return variants, annotations, set(csq_fields)


def csq_escape(value):
    return (
        str(value)
        .replace(",", "&")
        .replace("|", "&")
        .replace(";", "%3B")
        .replace("=", "%3D")
    )


def joined(value):
    if value is None:
        return ""
    if isinstance(value, list):
        return "&".join(csq_escape(item) for item in value)
    return csq_escape(value)


def position(row, prefix):
    start = row.get(f"{prefix}_start")
    end = row.get(f"{prefix}_end")
    if start is None:
        return ""
    return str(start) if end is None or end == start else f"{start}-{end}"


def rest_row(row, feature_type, feature_key, fields=REST_FIELDS):
    values = {
        "Allele": joined(row.get("variant_allele")),
        "Consequence": joined(row.get("consequence_terms")),
        "IMPACT": joined(row.get("impact")),
        "SYMBOL": joined(row.get("gene_symbol")),
        "Gene": joined(row.get("gene_id")),
        "Feature_type": feature_type,
        "Feature": joined(row.get(feature_key)),
        "BIOTYPE": joined(row.get("biotype")),
        "EXON": joined(row.get("exon")),
        "INTRON": joined(row.get("intron")),
        "HGVSc": joined(row.get("hgvsc")),
        "HGVSp": joined(row.get("hgvsp")),
        "cDNA_position": position(row, "cdna"),
        "CDS_position": position(row, "cds"),
        "Protein_position": position(row, "protein"),
        "Amino_acids": joined(row.get("amino_acids")),
        "Codons": joined(row.get("codons")),
        "DISTANCE": joined(row.get("distance")),
        "STRAND": joined(row.get("strand")),
        "FLAGS": joined(row.get("flags")),
        "CANONICAL": "YES" if row.get("canonical") else "",
        "MANE": joined(row.get("mane")),
        "TSL": joined(row.get("tsl")),
        "CCDS": joined(row.get("ccds")),
        "ENSP": joined(row.get("protein_id")),
    }
    return tuple(values[field] for field in fields)


def parse_rest(path, fields=REST_FIELDS):
    document = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(document, list) or not document:
        raise ValueError(f"{path}: VEP REST response must be a non-empty array")
    variants = Counter()
    annotations = Counter()
    collections = (
        ("transcript_consequences", "Transcript", "transcript_id"),
        ("intergenic_consequences", "Intergenic", ""),
    )
    for index, result in enumerate(document, 1):
        if not isinstance(result, dict):
            raise ValueError(f"{path}: response item {index} is not an object")
        if result.get("assembly_name") != "GRCh38":
            raise ValueError(f"{path}: response item {index} is not GRCh38")
        columns = str(result.get("input", "")).split()
        if len(columns) < 5:
            raise ValueError(f"{path}: response item {index} has no VCF input identity")
        key = tuple(columns[item] for item in (0, 1, 3, 4))
        variants[key] += 1
        for collection, feature_type, feature_key in collections:
            rows = result.get(collection, [])
            if not isinstance(rows, list):
                raise ValueError(f"{path}: response item {index} has invalid {collection}")
            for row in rows:
                if not isinstance(row, dict):
                    raise ValueError(f"{path}: {collection} contains a non-object value")
                annotations[(key, rest_row(row, feature_type, feature_key, fields))] += 1
    return variants, annotations


def load_contract(path, fields):
    if path is None:
        return set(), {}, None
    document = json.loads(path.read_text(encoding="utf-8"))
    if document.get("schemaVersion") != 1:
        raise ValueError(f"{path}: unsupported contract schema")

    ignored = set()
    for item in document.get("ignoredFields", []):
        field = item.get("field", "")
        if field not in fields or not item.get("reason"):
            raise ValueError(f"{path}: invalid ignored field {field!r}")
        ignored.add(field)

    allowed = {}
    for item in document.get("allowedExtraIdentities", []):
        variant = item.get("variant")
        if not isinstance(variant, list) or len(variant) != 4 or not item.get("reason"):
            raise ValueError(f"{path}: invalid allowed extra identity")
        identity = (
            tuple(str(value) for value in variant),
            str(item.get("allele", "")),
            str(item.get("featureType", "")),
            str(item.get("feature", "")),
        )
        if not all(identity[1:]) or identity in allowed:
            raise ValueError(f"{path}: duplicate or incomplete allowed extra identity")
        allowed[identity] = int(item.get("count", 1))

    return ignored, allowed, {
        "path": str(path),
        "sha256": sha256(path),
        "ignoredFields": sorted(ignored),
        "allowedExtraIdentities": len(allowed),
    }


def apply_allowed_extras(candidate, oracle, fields, allowed):
    if not allowed:
        return candidate, 0, []
    indexes = tuple(fields.index(field) for field in ("Allele", "Feature_type", "Feature"))
    adjusted = candidate.copy()
    remaining = dict(allowed)
    applied = 0
    for (key, values), count in candidate.items():
        identity = (key, *(values[index] for index in indexes))
        allowance = remaining.get(identity, 0)
        oracle_count = oracle.get((key, values), 0)
        removable = min(max(count - oracle_count, 0), allowance)
        if removable:
            adjusted[(key, values)] -= removable
            if adjusted[(key, values)] == 0:
                del adjusted[(key, values)]
            remaining[identity] -= removable
            applied += removable
    unused = [
        {
            "variant": list(identity[0]),
            "allele": identity[1],
            "featureType": identity[2],
            "feature": identity[3],
            "count": count,
        }
        for identity, count in remaining.items()
        if count
    ]
    return adjusted, applied, unused


def examples(counter, fields, limit=10):
    rows = []
    for (key, values), count in counter.most_common(limit):
        identity = dict(zip(fields, values))
        rows.append(
            {
                "variant": ":".join(key),
                "allele": identity["Allele"],
                "feature": identity["Feature"],
                "count": count,
                "fields": identity,
            }
        )
    return rows


def field_mismatches(candidate, oracle, fields):
    identity_indexes = tuple(fields.index(field) for field in ("Allele", "Feature_type", "Feature"))

    def indexed(rows):
        result = {}
        for (key, values), count in rows.items():
            identity = (key, *(values[index] for index in identity_indexes))
            result.setdefault(identity, []).extend([values] * count)
        return result

    candidate_rows = indexed(candidate)
    oracle_rows = indexed(oracle)
    shared = candidate_rows.keys() & oracle_rows.keys()
    mismatches = Counter()
    comparable = 0
    for identity in shared:
        left = candidate_rows[identity]
        right = oracle_rows[identity]
        if len(left) != 1 or len(right) != 1:
            continue
        comparable += 1
        for field, candidate_value, oracle_value in zip(fields, left[0], right[0]):
            if candidate_value != oracle_value:
                mismatches[field] += 1
    return {
        "candidateIdentities": len(candidate_rows),
        "oracleIdentities": len(oracle_rows),
        "sharedIdentities": len(shared),
        "comparableUniqueIdentities": comparable,
        "missingIdentities": len(oracle_rows.keys() - candidate_rows.keys()),
        "extraIdentities": len(candidate_rows.keys() - oracle_rows.keys()),
        "mismatchesByField": dict(sorted(mismatches.items())),
    }


def compare(candidate, oracle, oracle_format="auto", contract=None):
    if oracle_format == "auto":
        oracle_format = "rest-json" if oracle.suffix.lower() == ".json" else "vcf"
    fields = REST_FIELDS if oracle_format == "rest-json" else FIELDS
    ignored_fields, allowed_extras, contract_report = load_contract(contract, fields)
    fields = tuple(field for field in fields if field not in ignored_fields)
    candidate_variants, candidate_annotations, candidate_fields = parse_vcf(candidate, fields)
    if oracle_format == "rest-json":
        oracle_variants, oracle_annotations = parse_rest(oracle, fields)
        oracle_fields = set(fields)
    else:
        oracle_variants, oracle_annotations, oracle_fields = parse_vcf(oracle, fields)
    candidate_annotations, applied_extras, unused_extras = apply_allowed_extras(
        candidate_annotations, oracle_annotations, fields, allowed_extras
    )
    missing_variants = oracle_variants - candidate_variants
    extra_variants = candidate_variants - oracle_variants
    missing_annotations = oracle_annotations - candidate_annotations
    extra_annotations = candidate_annotations - oracle_annotations
    missing_candidate_fields = sorted(set(fields) - candidate_fields)
    missing_oracle_fields = sorted(set(fields) - oracle_fields)
    passed = not any(
        (
            missing_candidate_fields,
            missing_oracle_fields,
            missing_variants,
            extra_variants,
            missing_annotations,
            extra_annotations,
            unused_extras,
        )
    ) and bool(candidate_variants and candidate_annotations)
    report = {
        "schemaVersion": 2,
        "oracleFormat": oracle_format,
        "candidate": {"path": str(candidate), "sha256": sha256(candidate)},
        "oracle": {"path": str(oracle), "sha256": sha256(oracle)},
        "variantRecords": {
            "candidate": sum(candidate_variants.values()),
            "oracle": sum(oracle_variants.values()),
            "missing": sum(missing_variants.values()),
            "extra": sum(extra_variants.values()),
        },
        "annotationRows": {
            "candidate": sum(candidate_annotations.values()),
            "oracle": sum(oracle_annotations.values()),
            "missing": sum(missing_annotations.values()),
            "extra": sum(extra_annotations.values()),
        },
        "requiredFields": list(fields),
        "missingCandidateFields": missing_candidate_fields,
        "missingOracleFields": missing_oracle_fields,
        "missingAnnotationExamples": examples(missing_annotations, fields),
        "extraAnnotationExamples": examples(extra_annotations, fields),
        "identityComparison": field_mismatches(
            candidate_annotations, oracle_annotations, fields
        ),
        "passed": passed,
    }
    if contract_report is not None:
        contract_report.update(
            appliedExtraRows=applied_extras,
            unusedAllowedExtraIdentities=unused_extras,
        )
        report["contract"] = contract_report
    return report


def self_test():
    import tempfile

    header = (
        '##fileformat=VCFv4.2\n'
        '##INFO=<ID=CSQ,Number=.,Type=String,Description="Format: '
        + "|".join(ALL_FIELDS)
        + '">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n'
    )
    values = dict.fromkeys(ALL_FIELDS, "")
    values.update(
        Allele="G",
        Consequence="missense_variant",
        Gene="ENSG1",
        Feature_type="Transcript",
        Feature="ENST1",
    )
    row = "|".join(values[field] for field in ALL_FIELDS)
    with tempfile.TemporaryDirectory() as directory:
        left = Path(directory) / "left.vcf"
        right = Path(directory) / "right.vcf"
        text = header + f"1\t10\t.\tA\tG\t.\tPASS\tCSQ={row}\n"
        left.write_text(text, encoding="utf-8")
        right.write_text(text, encoding="utf-8")
        assert compare(left, right)["passed"]
        right.write_text(text.replace("missense_variant", "synonymous_variant"), encoding="utf-8")
        failed = compare(left, right)
        assert not failed["passed"]
        assert failed["annotationRows"]["missing"] == 1
        assert failed["annotationRows"]["extra"] == 1

        rest = Path(directory) / "oracle.json"
        rest.write_text(
            json.dumps(
                [
                    {
                        "assembly_name": "GRCh38",
                        "input": "1 10 . A G . . .",
                        "transcript_consequences": [
                            {
                                "variant_allele": "G",
                                "consequence_terms": ["missense_variant"],
                                "gene_id": "ENSG1",
                                "transcript_id": "ENST1",
                            }
                        ],
                    }
                ]
            ),
            encoding="utf-8",
        )
        assert compare(left, rest)["passed"]

        contract = Path(directory) / "contract.json"
        contract.write_text(
            json.dumps(
                {
                    "schemaVersion": 1,
                    "ignoredFields": [
                        {"field": "FLAGS", "reason": "fixture source difference"}
                    ],
                    "allowedExtraIdentities": [
                        {
                            "variant": ["1", "10", "A", "G"],
                            "allele": "G",
                            "featureType": "Transcript",
                            "feature": "ENST2",
                            "reason": "fixture source difference",
                        }
                    ],
                }
            ),
            encoding="utf-8",
        )
        extra_values = values.copy()
        extra_values["Feature"] = "ENST2"
        extra_row = "|".join(extra_values[field] for field in ALL_FIELDS)
        left.write_text(text.rstrip() + f",{extra_row}\n", encoding="utf-8")
        contracted = compare(left, rest, contract=contract)
        assert contracted["passed"]
        assert contracted["contract"]["appliedExtraRows"] == 1
        contract_document = json.loads(contract.read_text(encoding="utf-8"))
        contract_document["allowedExtraIdentities"][0]["feature"] = "ENST3"
        contract.write_text(json.dumps(contract_document), encoding="utf-8")
        assert not compare(left, rest, contract=contract)["passed"]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("candidate", nargs="?", type=Path)
    parser.add_argument("oracle", nargs="?", type=Path)
    parser.add_argument("--json", type=Path)
    parser.add_argument("--contract", type=Path)
    parser.add_argument(
        "--oracle-format", choices=("auto", "vcf", "rest-json"), default="auto"
    )
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        print("VEP concordance comparator self-test passed")
        return
    if not args.candidate or not args.oracle:
        parser.error("candidate and oracle VCF files are required")
    report = compare(args.candidate, args.oracle, args.oracle_format, args.contract)
    rendered = json.dumps(report, indent=2, sort_keys=True)
    if args.json:
        args.json.write_text(rendered + "\n", encoding="utf-8")
    print(rendered)
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
