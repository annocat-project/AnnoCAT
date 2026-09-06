#!/usr/bin/env python3
"""Verify fastVEP fields that official VEP GFF mode does not project."""

import argparse
import gzip
import json
import re
import tempfile
from collections import Counter
from pathlib import Path


def attributes(value):
    return dict(item.split("=", 1) for item in value.rstrip().split(";") if "=" in item)


def transcript_id(value):
    if not value:
        return None
    value = value.split(",", 1)[0]
    return value.removeprefix("transcript:") if value.startswith("transcript:") else None


def candidate_projection(path):
    fields = None
    observed = Counter()
    rows = 0
    with path.open(encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, 1):
            if line.startswith("##INFO=<ID=CSQ"):
                match = re.search(r'Format: ([^">]+)', line)
                if not match:
                    raise ValueError(f"{path}:{line_number}: CSQ format is missing")
                fields = match.group(1).split("|")
                continue
            if line.startswith("#"):
                continue
            if fields is None:
                raise ValueError(f"{path}: CSQ header is missing")
            columns = line.rstrip("\r\n").split("\t")
            csq = next((item[4:] for item in columns[7].split(";") if item.startswith("CSQ=")), "")
            for encoded in filter(None, csq.split(",")):
                values = encoded.split("|")
                if len(values) != len(fields):
                    raise ValueError(f"{path}:{line_number}: malformed CSQ row")
                row = dict(zip(fields, values))
                if row.get("Feature_type") != "Transcript" or not row.get("Feature"):
                    continue
                feature = row["Feature"].split(".", 1)[0]
                observed[
                    (
                        feature,
                        row.get("CCDS", ""),
                        row.get("MANE", ""),
                        bool(row.get("MANE_SELECT")),
                        bool(row.get("MANE_PLUS_CLINICAL")),
                        row.get("SOURCE", ""),
                    )
                ] += 1
                rows += 1
    required = {"CCDS", "MANE", "MANE_SELECT", "MANE_PLUS_CLINICAL", "SOURCE"}
    if fields is None or not required.issubset(fields) or rows == 0:
        raise ValueError(f"{path}: transcript GFF field projection is unavailable")
    return observed, rows


def gff_projection(path, wanted):
    expected = {}
    opener = gzip.open if path.suffix == ".gz" else open
    with opener(path, "rt", encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("#"):
                continue
            columns = line.rstrip().split("\t")
            if len(columns) != 9:
                continue
            attrs = attributes(columns[8])
            feature = transcript_id(attrs.get("ID"))
            if feature in wanted:
                tags = set(attrs.get("tag", "").split(","))
                mane_select = "MANE_Select" in tags
                mane_plus_clinical = "MANE_Plus_Clinical" in tags
                expected[feature] = {
                    "CCDS": attrs.get("ccdsid", ""),
                    "MANE": (
                        "MANE_Select"
                        if mane_select
                        else "MANE_Plus_Clinical" if mane_plus_clinical else ""
                    ),
                    "MANE_SELECT_PRESENT": mane_select,
                    "MANE_PLUS_CLINICAL_PRESENT": mane_plus_clinical,
                }
    return expected


def verify(gff, candidate, expected_source):
    observed, rows = candidate_projection(candidate)
    features = {item[0] for item in observed}
    expected = gff_projection(gff, features)
    mismatches = []
    for values, count in sorted(observed.items()):
        (
            feature,
            ccds,
            mane,
            mane_select_present,
            mane_plus_clinical_present,
            source,
        ) = values
        if feature not in expected:
            mismatches.append(
                {
                    "feature": feature,
                    "field": "Feature",
                    "candidate": feature,
                    "expected": None,
                    "rows": count,
                }
            )
            continue
        candidate_values = {
            "CCDS": ccds,
            "MANE": mane,
            "MANE_SELECT_PRESENT": mane_select_present,
            "MANE_PLUS_CLINICAL_PRESENT": mane_plus_clinical_present,
            "SOURCE": source,
        }
        expected_values = {**expected[feature], "SOURCE": expected_source}
        for field, value in candidate_values.items():
            if value != expected_values[field]:
                mismatches.append(
                    {
                        "feature": feature,
                        "field": field,
                        "candidate": value,
                        "expected": expected_values[field],
                        "rows": count,
                    }
                )
    return {
        "candidate": str(candidate),
        "transcriptRows": rows,
        "transcripts": len(features),
        "validatedFields": [
            "CCDS",
            "MANE",
            "MANE_SELECT presence",
            "MANE_PLUS_CLINICAL presence",
            "SOURCE logical label",
        ],
        "unresolvedTranscripts": len(features - expected.keys()),
        "mismatches": mismatches,
        "passed": not mismatches,
    }


def self_test():
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        gff = root / "test.gff3"
        gff.write_text(
            "##gff-version 3\n"
            "1\tEnsembl\tmRNA\t1\t10\t.\t+\t.\tID=transcript:ENST1;ccdsid=CCDS1.1;tag=MANE_Select\n"
            "1\tEnsembl\tpseudogenic_transcript\t20\t30\t.\t+\t.\tID=transcript:ENST2;tag=MANE_Plus_Clinical\n",
            encoding="utf-8",
        )
        vcf = root / "test.vcf"
        header = (
            "##fileformat=VCFv4.2\n"
            '##INFO=<ID=CSQ,Number=.,Type=String,Description="Format: Feature_type|Feature|CCDS|MANE|MANE_SELECT|MANE_PLUS_CLINICAL|SOURCE">\n'
            "#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n"
        )
        vcf.write_text(
            header
            + "1\t2\t.\tA\tG\t.\tPASS\tCSQ=Transcript|ENST1|CCDS1.1|MANE_Select|ENST1.1||Ensembl,Transcript|ENST2||MANE_Plus_Clinical||ENST2.1|Ensembl\n",
            encoding="utf-8",
        )
        assert verify(gff, vcf, "Ensembl")["passed"]
        vcf.write_text(
            header
            + "1\t2\t.\tA\tG\t.\tPASS\tCSQ=Transcript|ENST1|CCDS1.1|MANE_Plus_Clinical|ENST1.1||Wrong\n",
            encoding="utf-8",
        )
        assert not verify(gff, vcf, "Ensembl")["passed"]
    print("VEP GFF projection validator self-test passed")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("candidate", nargs="*", type=Path)
    parser.add_argument("--gff", type=Path)
    parser.add_argument("--json", type=Path)
    parser.add_argument("--expected-source", default="Ensembl")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if not args.candidate or args.gff is None or args.json is None:
        parser.error("candidate VCF files, --gff, and --json are required")
    reports = [verify(args.gff, path, args.expected_source) for path in args.candidate]
    report = {"schemaVersion": 1, "gff": str(args.gff), "outputs": reports}
    report["passed"] = all(item["passed"] for item in reports)
    rendered = json.dumps(report, indent=2, sort_keys=True) + "\n"
    args.json.write_text(rendered, encoding="utf-8")
    print(rendered, end="")
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
