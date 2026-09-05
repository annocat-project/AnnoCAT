#!/usr/bin/env python3
"""Verify that an AnnoCAT result preserves the accepted VEP consequence rows."""

import argparse
import importlib.util
import json
import tempfile
from collections import Counter, defaultdict
from pathlib import Path


def load_comparator():
    path = Path(__file__).with_name("compare-vep-concordance.py")
    spec = importlib.util.spec_from_file_location("vep_concordance", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


VEP = load_comparator()
FEATURE_TYPES = {
    "transcript": "Transcript",
    "regulatory": "RegulatoryFeature",
    "motif": "MotifFeature",
    "intergenic": "Intergenic",
    "unresolved": "",
}
# These VCF-only values are validated by compare-vep-concordance.py. The
# versioned consequence table does not claim to store them.
VCF_ONLY_FIELDS = {
    "MANE": "membership label is represented by the stored MANE transcript fields",
    "REF_ALLELE": "input allele serialization remains in the retained VCF",
    "UPLOADED_ALLELE": "submitted allele serialization remains in the retained VCF",
}
PROJECTION_NORMALIZERS = {
    "Amino_acids": "collapse an identical structured reference/alternate pair to VEP's synonymous VCF value",
}
STRING_COLUMNS = {
    "feature_id": "feature_id",
    "gene_id": "gene_id",
    "gene_symbol": "gene_symbol",
    "biotype": "biotype",
    "impact": "impact",
    "mane_select": "mane_select",
    "mane_plus_clinical": "mane_plus_clinical",
    "protein_id": "protein_id",
    "exon": "exon",
    "intron": "intron",
    "hgvsg": "hgvsg",
    "hgvsc": "hgvsc",
    "hgvsp": "hgvsp",
}


def json_truthy(value):
    return value is True or isinstance(value, int) and not isinstance(value, bool) and value != 0


def optional_string(value):
    return value if isinstance(value, str) else None


def position(document, prefix):
    start = document.get(f"{prefix}_start")
    end = document.get(f"{prefix}_end")
    if start is None:
        return ""
    return str(start) if end is None or end == start else f"{start}-{end}"


def projected_values(document, fields, normalizers):
    feature_type = str(document.get("feature_type", "")).lower()
    values = {
        "Allele": VEP.joined(document.get("variant_allele")),
        "Consequence": VEP.joined(document.get("consequence_terms")),
        "IMPACT": VEP.joined(document.get("impact")),
        "SYMBOL": VEP.joined(document.get("gene_symbol")),
        "Gene": VEP.joined(document.get("gene_id")),
        "Feature_type": FEATURE_TYPES.get(feature_type, feature_type),
        "Feature": VEP.joined(document.get("feature_id")),
        "BIOTYPE": VEP.joined(document.get("biotype")),
        "EXON": VEP.joined(document.get("exon")),
        "INTRON": VEP.joined(document.get("intron")),
        "HGVSc": VEP.joined(document.get("hgvsc")),
        "HGVSp": VEP.joined(document.get("hgvsp")),
        "cDNA_position": position(document, "cdna"),
        "CDS_position": position(document, "cds"),
        "Protein_position": position(document, "protein"),
        "Amino_acids": VEP.joined(document.get("amino_acids")),
        "Codons": VEP.joined(document.get("codons")),
        "REF_ALLELE": VEP.joined(document.get("ref_allele")),
        "UPLOADED_ALLELE": VEP.joined(document.get("uploaded_allele")),
        "DISTANCE": VEP.joined(document.get("distance")),
        "STRAND": VEP.joined(document.get("strand")),
        "FLAGS": VEP.joined(document.get("flags")),
        "CANONICAL": "YES" if json_truthy(document.get("canonical")) else "",
        "MANE": VEP.joined(document.get("mane")),
        "MANE_SELECT": VEP.joined(document.get("mane_select")),
        "MANE_PLUS_CLINICAL": VEP.joined(document.get("mane_plus_clinical")),
        "TSL": VEP.joined(document.get("tsl")),
        "CCDS": VEP.joined(document.get("ccds")),
        "ENSP": VEP.joined(document.get("protein_id")),
        "SOURCE": VEP.joined(document.get("source")),
        "HGVS_OFFSET": VEP.joined(document.get("hgvs_offset")),
    }
    for field, normalizer in normalizers.items():
        if field in values:
            values[field] = VEP.normalize_field(values[field], normalizer)
    amino_acids = values["Amino_acids"].split("/")
    if len(amino_acids) == 2 and amino_acids[0] == amino_acids[1]:
        values["Amino_acids"] = amino_acids[0]
    return tuple(values.get(field, "") for field in fields)


def variant_identities(connection, variants_path):
    rows = connection.execute(
        """
        SELECT allele_id, record_number, alt_index, alternate_count,
               original_chromosome, original_position, original_reference,
               original_alternate, gene_symbol, gene_id, transcript_id,
               consequence, impact, canonical, mane_select
        FROM read_parquet(?)
        ORDER BY record_number, alt_index
        """,
        [str(variants_path)],
    ).fetchall()
    records = {}
    alleles = {}
    record_alleles = {}
    representatives = {}
    errors = []
    for row in rows:
        (
            allele_id,
            record_number,
            alt_index,
            alternate_count,
            chromosome,
            position_value,
            reference,
            alternate,
            *representative,
        ) = row
        current = records.setdefault(
            record_number,
            {
                "identity": (str(chromosome), str(position_value), str(reference)),
                "alternateCount": alternate_count,
                "alternates": {},
            },
        )
        if current["identity"] != (str(chromosome), str(position_value), str(reference)):
            errors.append(f"record {record_number} has inconsistent original identity")
        if current["alternateCount"] != alternate_count:
            errors.append(f"record {record_number} has inconsistent alternate count")
        if alt_index in current["alternates"]:
            errors.append(f"record {record_number} repeats alternate index {alt_index}")
        current["alternates"][alt_index] = str(alternate)
        record_alleles[(record_number, alt_index)] = allele_id
        if allele_id in representatives:
            errors.append(f"allele {allele_id} occurs more than once in variants.parquet")
        representatives[allele_id] = tuple(representative)

    for record_number, record in records.items():
        indexes = set(record["alternates"])
        expected = set(range(1, record["alternateCount"] + 1))
        if indexes != expected:
            errors.append(
                f"record {record_number} retains alternate indexes {sorted(indexes)}, "
                f"expected {sorted(expected)}"
            )
            continue
        key = (*record["identity"], ",".join(record["alternates"][index] for index in sorted(indexes)))
        for index in indexes:
            allele_id = record_alleles[(record_number, index)]
            previous = alleles.setdefault(allele_id, key)
            if previous != key:
                errors.append(f"allele {allele_id} maps to multiple input records")
    return rows, alleles, representatives, errors


def typed_row_errors(row, document):
    errors = []
    feature_type = str(document.get("feature_type", "")).lower()
    expected_transcript = optional_string(document.get("transcript_id")) if feature_type == "transcript" else None
    checks = {
        "feature_type": feature_type,
        "transcript_id": expected_transcript,
        **{column: optional_string(document.get(key)) for column, key in STRING_COLUMNS.items()},
        "canonical": json_truthy(document.get("canonical")),
        "distance": document.get("distance") if isinstance(document.get("distance"), int) else None,
        "strand": document.get("strand") if isinstance(document.get("strand"), int) else None,
    }
    terms = document.get("consequence_terms")
    terms = terms if isinstance(terms, list) else []
    checks["consequence_terms_json"] = json.dumps(terms, separators=(",", ":"))
    checks["primary_consequence"] = terms[0] if terms and isinstance(terms[0], str) else None
    for name, expected in checks.items():
        if row[name] != expected:
            errors.append({"field": name, "stored": row[name], "structured": expected})
    return errors


def normalized_optional(value):
    if value is None:
        return ""
    rendered = str(value).strip()
    return "" if rendered.upper() in {"", ".", "-", "NA", "N/A", "NONE", "NULL"} else rendered


def representative_errors(representatives, consequence_rows):
    selected = defaultdict(list)
    for row in consequence_rows:
        if row["selected"]:
            selected[row["allele_id"]].append(row)
    errors = []
    for allele_id, variant in representatives.items():
        rows = selected.get(allele_id, [])
        if len(rows) != 1:
            errors.append({"alleleId": allele_id, "selectedRows": len(rows)})
            continue
        row = rows[0]
        stored = (
            normalized_optional(variant[0]),
            normalized_optional(variant[1]),
            normalized_optional(variant[2]),
            normalized_optional(str(variant[3] or "").split("&", 1)[0]),
            normalized_optional(variant[4]),
            bool(variant[5]),
            normalized_optional(variant[6]),
        )
        projected = (
            normalized_optional(row["gene_symbol"]),
            normalized_optional(row["gene_id"]),
            normalized_optional(row["transcript_id"] or row["feature_id"]),
            normalized_optional(row["primary_consequence"]),
            normalized_optional(row["impact"]),
            bool(row["canonical"]),
            normalized_optional(row["mane_select"]),
        )
        if stored != projected:
            errors.append(
                {"alleleId": allele_id, "variantRow": list(stored), "selectedConsequence": list(projected)}
            )
    for allele_id in selected.keys() - representatives.keys():
        errors.append({"alleleId": allele_id, "error": "selected consequence has no variant row"})
    return errors


def read_projection(connection, variants_path, consequences_path, fields, normalizers):
    variant_rows, identities, representatives, identity_errors = variant_identities(
        connection, variants_path
    )
    names = (
        "consequence_id",
        "allele_id",
        "feature_type",
        "feature_id",
        "transcript_id",
        "gene_id",
        "gene_symbol",
        "biotype",
        "consequence_terms_json",
        "primary_consequence",
        "impact",
        "canonical",
        "mane_select",
        "mane_plus_clinical",
        "protein_id",
        "exon",
        "intron",
        "hgvsg",
        "hgvsc",
        "hgvsp",
        "distance",
        "strand",
        "consequence_json",
        "selected",
    )
    raw_rows = connection.execute(
        f"SELECT {', '.join(names)} FROM read_parquet(?) ORDER BY allele_id, ordinal",
        [str(consequences_path)],
    ).fetchall()
    annotations = Counter()
    rows = []
    typed_errors = []
    for raw in raw_rows:
        row = dict(zip(names, raw))
        rows.append(row)
        key = identities.get(row["allele_id"])
        if key is None:
            identity_errors.append(f"consequence {row['consequence_id']} has no variant allele")
            continue
        try:
            document = json.loads(row["consequence_json"])
        except (TypeError, json.JSONDecodeError) as error:
            typed_errors.append({"consequenceId": row["consequence_id"], "error": str(error)})
            continue
        mismatches = typed_row_errors(row, document)
        if mismatches:
            typed_errors.append({"consequenceId": row["consequence_id"], "mismatches": mismatches})
        annotations[(key, projected_values(document, fields, normalizers))] += 1
    return variant_rows, rows, annotations, identity_errors, typed_errors, representative_errors(representatives, rows)


def count_checks(manifest, input_records, annotated_records, annotated_csq, variant_rows, consequence_rows):
    expected = {
        "vcfRecordCount": annotated_records,
        "structuredRecordCount": annotated_records,
        "variantCount": variant_rows,
        "alleleCount": variant_rows,
        "csqEntryCount": annotated_csq,
        "consequenceCount": consequence_rows,
    }
    failures = [
        {"field": field, "declared": manifest.get(field), "observed": observed}
        for field, observed in expected.items()
        if manifest.get(field) != observed
    ]
    if input_records != annotated_records:
        failures.append({"field": "inputRecords", "input": input_records, "annotated": annotated_records})
    for field in ("excludedAuxiliaryRecordCount", "excludedUncarriedAlleleCount"):
        if manifest.get(field, 0) != 0:
            failures.append({"field": field, "declared": manifest.get(field), "expected": 0})
    return expected, failures


def verify(input_path, annotated_vcf, oracle, variants, consequences, manifest_path, contract_path):
    try:
        import duckdb
    except ImportError as error:
        raise RuntimeError("duckdb is required; install the pinned CI version") from error

    contract = VEP.load_contract(contract_path, VEP.FIELDS)
    fields = tuple(field for field in contract["fields"] if field not in VCF_ONLY_FIELDS)
    normalizers = contract["normalizers"]
    _, oracle_annotations, _, _, _ = VEP.parse_vcf(oracle, fields, normalizers)
    annotated_variants, annotated_annotations, _, _, _ = VEP.parse_vcf(
        annotated_vcf, fields, normalizers
    )
    input_records, _ = VEP.parse_input_vcf(input_path)
    connection = duckdb.connect(":memory:")
    try:
        (
            variant_rows,
            consequence_rows,
            projected_annotations,
            identity_errors,
            typed_errors,
            selected_errors,
        ) = read_projection(connection, variants, consequences, fields, normalizers)
    finally:
        connection.close()

    adjusted, applied_extras, unused_extras = VEP.apply_allowed_extras(
        projected_annotations, oracle_annotations, fields, contract["allowedExtras"]
    )
    missing = oracle_annotations - adjusted
    extra = adjusted - oracle_annotations
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    observed_counts, count_errors = count_checks(
        manifest,
        sum(input_records.values()),
        sum(annotated_variants.values()),
        sum(annotated_annotations.values()),
        len(variant_rows),
        len(consequence_rows),
    )
    feature_index = fields.index("Feature_type")
    intergenic_oracle = sum(
        count for (_key, values), count in oracle_annotations.items() if values[feature_index] == "Intergenic"
    )
    intergenic_projected = sum(
        count for (_key, values), count in adjusted.items() if values[feature_index] == "Intergenic"
    )
    passed = not any(
        (missing, extra, unused_extras, identity_errors, typed_errors, selected_errors, count_errors)
    ) and bool(projected_annotations)
    return {
        "schemaVersion": 1,
        "input": {"path": str(input_path), "sha256": VEP.sha256(input_path)},
        "annotatedVcf": {"path": str(annotated_vcf), "sha256": VEP.sha256(annotated_vcf)},
        "oracle": {"path": str(oracle), "sha256": VEP.sha256(oracle)},
        "variants": {"path": str(variants), "sha256": VEP.sha256(variants)},
        "consequences": {"path": str(consequences), "sha256": VEP.sha256(consequences)},
        "manifest": {"path": str(manifest_path), "sha256": VEP.sha256(manifest_path)},
        "contract": contract["report"],
        "projectedFields": list(fields),
        "projectionNormalizers": PROJECTION_NORMALIZERS,
        "vcfOnlyFields": VCF_ONLY_FIELDS,
        "annotationRows": {
            "oracle": sum(oracle_annotations.values()),
            "projected": sum(projected_annotations.values()),
            "missing": sum(missing.values()),
            "extra": sum(extra.values()),
        },
        "identityComparison": VEP.field_mismatches(adjusted, oracle_annotations, fields),
        "missingExamples": VEP.examples(missing, fields),
        "extraExamples": VEP.examples(extra, fields),
        "intergenicProjection": {"oracle": intergenic_oracle, "projected": intergenic_projected},
        "typedColumnErrors": typed_errors[:20],
        "identityErrors": identity_errors[:20],
        "representativeErrors": selected_errors[:20],
        "countReconciliation": {"observed": observed_counts, "errors": count_errors},
        "appliedAllowedExtraRows": applied_extras,
        "unusedAllowedExtraIdentities": unused_extras,
        "passed": passed,
    }


def write_parquet(connection, table, path):
    rendered = str(path).replace("'", "''")
    connection.execute(f"COPY {table} TO '{rendered}' (FORMAT PARQUET)")


def self_test():
    import duckdb

    values = dict.fromkeys(VEP.PRODUCTION_FIELDS, "")
    values.update(
        Allele="G",
        Consequence="missense_variant",
        IMPACT="MODERATE",
        SYMBOL="GENE1",
        Gene="ENSG1",
        Feature_type="Transcript",
        Feature="ENST1",
        BIOTYPE="protein_coding",
        CANONICAL="YES",
    )
    csq = "|".join(values[field] for field in VEP.PRODUCTION_FIELDS)
    header = (
        "##fileformat=VCFv4.2\n"
        + '##INFO=<ID=CSQ,Number=.,Type=String,Description="Format: '
        + "|".join(VEP.PRODUCTION_FIELDS)
        + '">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n'
    )
    consequence = {
        "variant_allele": "G",
        "consequence_terms": ["missense_variant"],
        "impact": "MODERATE",
        "gene_symbol": "GENE1",
        "gene_id": "ENSG1",
        "transcript_id": "ENST1",
        "feature_type": "transcript",
        "feature_id": "ENST1",
        "biotype": "protein_coding",
        "canonical": True,
    }
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        input_path = root / "input.vcf"
        annotated = root / "annotated.vcf"
        oracle = root / "oracle.vcf"
        variants = root / "variants.parquet"
        consequences = root / "consequences.parquet"
        manifest = root / "manifest.json"
        input_path.write_text(header.split('##INFO=<ID=CSQ')[0] + "#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n1\t10\t.\tA\tG\t.\tPASS\t.\n", encoding="utf-8")
        annotated.write_text(header + f"1\t10\t.\tA\tG\t.\tPASS\tCSQ={csq}\n", encoding="utf-8")
        oracle.write_text(annotated.read_text(encoding="utf-8"), encoding="utf-8")
        connection = duckdb.connect(":memory:")
        connection.execute(
            """CREATE TABLE variants AS SELECT
               '1:10:A:G'::VARCHAR allele_id, 1::BIGINT record_number, 1::INTEGER alt_index,
               1::INTEGER alternate_count, '1'::VARCHAR original_chromosome,
               10::BIGINT original_position, 'A'::VARCHAR original_reference,
               'G'::VARCHAR original_alternate, 'GENE1'::VARCHAR gene_symbol,
               'ENSG1'::VARCHAR gene_id, 'ENST1'::VARCHAR transcript_id,
               'missense_variant'::VARCHAR consequence, 'MODERATE'::VARCHAR impact,
               true::BOOLEAN canonical, NULL::VARCHAR mane_select"""
        )
        connection.execute(
            """CREATE TABLE consequences AS SELECT
               'c1'::VARCHAR consequence_id, '1:10:A:G'::VARCHAR allele_id,
               0::BIGINT ordinal, 'transcript'::VARCHAR feature_type,
               'ENST1'::VARCHAR feature_id, 'ENST1'::VARCHAR transcript_id,
               'ENSG1'::VARCHAR gene_id, 'GENE1'::VARCHAR gene_symbol,
               'protein_coding'::VARCHAR biotype,
               '[\"missense_variant\"]'::VARCHAR consequence_terms_json,
               'missense_variant'::VARCHAR primary_consequence, 'MODERATE'::VARCHAR impact,
               true::BOOLEAN canonical, NULL::VARCHAR mane_select,
               NULL::VARCHAR mane_plus_clinical, NULL::VARCHAR protein_id,
               NULL::VARCHAR exon, NULL::VARCHAR intron, NULL::VARCHAR hgvsg,
               NULL::VARCHAR hgvsc, NULL::VARCHAR hgvsp, NULL::BIGINT distance,
               NULL::INTEGER strand, ?::VARCHAR consequence_json, true::BOOLEAN selected""",
            [json.dumps(consequence, separators=(",", ":"))],
        )
        write_parquet(connection, "variants", variants)
        write_parquet(connection, "consequences", consequences)
        connection.close()
        manifest.write_text(
            json.dumps(
                {
                    "vcfRecordCount": 1,
                    "structuredRecordCount": 1,
                    "variantCount": 1,
                    "alleleCount": 1,
                    "csqEntryCount": 1,
                    "consequenceCount": 1,
                    "excludedAuxiliaryRecordCount": 0,
                    "excludedUncarriedAlleleCount": 0,
                }
            ),
            encoding="utf-8",
        )
        assert verify(input_path, annotated, oracle, variants, consequences, manifest, None)["passed"]
        oracle.write_text(
            annotated.read_text(encoding="utf-8").replace("missense_variant", "synonymous_variant"),
            encoding="utf-8",
        )
        assert not verify(input_path, annotated, oracle, variants, consequences, manifest, None)["passed"]
    print("AnnoCAT VEP projection verifier self-test passed")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", type=Path)
    parser.add_argument("--annotated-vcf", type=Path)
    parser.add_argument("--oracle", type=Path)
    parser.add_argument("--variants", type=Path)
    parser.add_argument("--consequences", type=Path)
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--contract", type=Path)
    parser.add_argument("--json", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    required = ("input", "annotated_vcf", "oracle", "variants", "consequences", "manifest")
    missing = [name.replace("_", "-") for name in required if getattr(args, name) is None]
    if missing:
        parser.error("required arguments: " + ", ".join(f"--{name}" for name in missing))
    report = verify(
        args.input,
        args.annotated_vcf,
        args.oracle,
        args.variants,
        args.consequences,
        args.manifest,
        args.contract,
    )
    rendered = json.dumps(report, indent=2, sort_keys=True)
    if args.json:
        args.json.write_text(rendered + "\n", encoding="utf-8")
    print(rendered)
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
