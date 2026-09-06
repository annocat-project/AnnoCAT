#!/usr/bin/env python3
"""Verify that transcript filtering removes only declared VCF CSQ entries."""

from __future__ import annotations

import argparse
import gzip
import json
from contextlib import ExitStack
from pathlib import Path


def open_text(path: Path):
    return gzip.open(path, "rt", encoding="utf-8", newline="") if path.suffix == ".gz" else path.open("r", encoding="utf-8", newline="")


def stable_id(value: str) -> str:
    stem, separator, version = value.rpartition(".")
    return stem if separator and version.isdecimal() else value


def split_info(value: str) -> list[tuple[str, str | None]]:
    if value == ".":
        return []
    fields = []
    for item in value.split(";"):
        key, separator, item_value = item.partition("=")
        fields.append((key, item_value if separator else None))
    return fields


def csq_feature_index(header: list[str]) -> int:
    prefix = "##INFO=<ID=CSQ,"
    for line in header:
        if line.startswith(prefix) and "Format: " in line:
            names = line.split("Format: ", 1)[1].removesuffix('\">\n').split("|")
            return names.index("Feature")
    raise ValueError("CSQ header with a Feature field was not found")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("baseline", type=Path)
    parser.add_argument("filtered", type=Path)
    parser.add_argument("exclusions", type=Path)
    parser.add_argument("--require-removals", action="store_true")
    args = parser.parse_args()

    exclusions = set(args.exclusions.read_text(encoding="utf-8").splitlines())
    errors: list[str] = []
    affected_records = removed_entries = records = synthesized_intergenic = 0
    observed_ids: set[str] = set()

    with ExitStack() as stack:
        baseline = stack.enter_context(open_text(args.baseline))
        filtered = stack.enter_context(open_text(args.filtered))
        baseline_header = []
        filtered_header = []
        baseline_line = baseline.readline()
        filtered_line = filtered.readline()
        while baseline_line.startswith("#"):
            baseline_header.append(baseline_line)
            baseline_line = baseline.readline()
        while filtered_line.startswith("#"):
            filtered_header.append(filtered_line)
            filtered_line = filtered.readline()
        if baseline_header != filtered_header:
            errors.append("VCF headers differ")
        feature_index = csq_feature_index(baseline_header)

        while baseline_line or filtered_line:
            records += 1
            if not baseline_line or not filtered_line:
                errors.append(f"record count differs at record {records}")
                break
            left = baseline_line.rstrip("\r\n").split("\t")
            right = filtered_line.rstrip("\r\n").split("\t")
            if len(left) < 8 or len(right) < 8 or left[:7] != right[:7] or left[8:] != right[8:]:
                errors.append(f"non-INFO VCF fields differ at record {records}")
            else:
                left_info = split_info(left[7])
                right_info = split_info(right[7])
                left_other = [item for item in left_info if item[0] != "CSQ"]
                right_other = [item for item in right_info if item[0] != "CSQ"]
                if left_other != right_other:
                    errors.append(f"non-CSQ INFO fields differ at record {records}")
                left_csq = next((value or "" for key, value in left_info if key == "CSQ"), "")
                right_csq = next((value or "" for key, value in right_info if key == "CSQ"), "")
                kept = []
                removed_here = 0
                for entry in filter(None, left_csq.split(",")):
                    values = entry.split("|")
                    if len(values) <= feature_index:
                        errors.append(f"short CSQ entry at record {records}")
                        continue
                    transcript = stable_id(values[feature_index])
                    if transcript in exclusions:
                        removed_here += 1
                        observed_ids.add(transcript)
                    else:
                        kept.append(entry)
                actual = list(filter(None, right_csq.split(",")))
                only_intergenic_replacement = (
                    not kept
                    and removed_here > 0
                    and len(actual) == 1
                    and actual[0].split("|")[1] == "intergenic_variant"
                    and actual[0].split("|")[feature_index] == "-"
                )
                if only_intergenic_replacement:
                    synthesized_intergenic += 1
                elif kept != actual:
                    key = ":".join((left[0], left[1], left[3], left[4]))
                    actual_summary = [
                        f"{entry.split('|')[1]}:{entry.split('|')[feature_index]}"
                        for entry in actual
                    ]
                    errors.append(
                        f"filtered CSQ entries differ at {key} (record {records}); "
                        f"expected {len(kept)}, actual {actual_summary}"
                    )
                if removed_here:
                    affected_records += 1
                    removed_entries += removed_here
            if len(errors) >= 20:
                break
            baseline_line = baseline.readline()
            filtered_line = filtered.readline()

    if args.require_removals and removed_entries == 0:
        errors.append("the comparison removed no declared transcript entries")
    report = {
        "recordsCompared": records,
        "affectedRecords": affected_records,
        "removedCsqEntries": removed_entries,
        "synthesizedIntergenicEntries": synthesized_intergenic,
        "observedExcludedTranscripts": len(observed_ids),
        "declaredExcludedTranscripts": len(exclusions),
        "errors": errors,
        "passed": not errors,
    }
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
