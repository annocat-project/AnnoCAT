#!/usr/bin/env python3
"""Verify the pinned fastVEP executable's implicit 5 kb proximity boundary."""

from __future__ import annotations

import argparse
import json
import subprocess
import tempfile
from pathlib import Path


GFF3 = """##gff-version 3
chr1\tannocat\tgene\t10000\t11000\t.\t+\t.\tID=gene:ENSGPLUS;Name=PLUS;biotype=protein_coding
chr1\tannocat\tmRNA\t10000\t11000\t.\t+\t.\tID=transcript:ENSTPLUS;Parent=gene:ENSGPLUS;biotype=protein_coding;tag=Ensembl_canonical
chr1\tannocat\texon\t10000\t11000\t.\t+\t.\tID=exon:EXPLUS;Parent=transcript:ENSTPLUS;rank=1
chr1\tannocat\tgene\t50000\t51000\t.\t-\t.\tID=gene:ENSGMINUS;Name=MINUS;biotype=protein_coding
chr1\tannocat\tmRNA\t50000\t51000\t.\t-\t.\tID=transcript:ENSTMINUS;Parent=gene:ENSGMINUS;biotype=protein_coding;tag=Ensembl_canonical
chr1\tannocat\texon\t50000\t51000\t.\t-\t.\tID=exon:EXMINUS;Parent=transcript:ENSTMINUS;rank=1
"""

VCF = """##fileformat=VCFv4.2
##contig=<ID=chr1,length=100000>
#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO
chr1\t4999\tplus_left_5001\tA\tG\t.\tPASS\t.
chr1\t5000\tplus_left_5000\tA\tG\t.\tPASS\t.
chr1\t5001\tplus_left_4999\tA\tG\t.\tPASS\t.
chr1\t15999\tplus_right_4999\tA\tG\t.\tPASS\t.
chr1\t16000\tplus_right_5000\tA\tG\t.\tPASS\t.
chr1\t16001\tplus_right_5001\tA\tG\t.\tPASS\t.
chr1\t44999\tminus_left_5001\tA\tG\t.\tPASS\t.
chr1\t45000\tminus_left_5000\tA\tG\t.\tPASS\t.
chr1\t45001\tminus_left_4999\tA\tG\t.\tPASS\t.
chr1\t55999\tminus_right_4999\tA\tG\t.\tPASS\t.
chr1\t56000\tminus_right_5000\tA\tG\t.\tPASS\t.
chr1\t56001\tminus_right_5001\tA\tG\t.\tPASS\t.
"""

EXPECTED = {
    "plus_left_5001": (None, "intergenic_variant", None),
    "plus_left_5000": ("PLUS", "upstream_gene_variant", 5000),
    "plus_left_4999": ("PLUS", "upstream_gene_variant", 4999),
    "plus_right_4999": ("PLUS", "downstream_gene_variant", 4999),
    "plus_right_5000": ("PLUS", "downstream_gene_variant", 5000),
    "plus_right_5001": (None, "intergenic_variant", None),
    "minus_left_5001": (None, "intergenic_variant", None),
    "minus_left_5000": ("MINUS", "downstream_gene_variant", 5000),
    "minus_left_4999": ("MINUS", "downstream_gene_variant", 4999),
    "minus_right_4999": ("MINUS", "upstream_gene_variant", 4999),
    "minus_right_5000": ("MINUS", "upstream_gene_variant", 5000),
    "minus_right_5001": (None, "intergenic_variant", None),
}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--fastvep", required=True, type=Path)
    args = parser.parse_args()
    if not args.fastvep.is_file():
        parser.error(f"fastVEP executable does not exist: {args.fastvep}")

    with tempfile.TemporaryDirectory(prefix="annocat-fastvep-distance-") as temp:
        root = Path(temp)
        gff3 = root / "genes.gff3"
        vcf = root / "variants.vcf"
        output = root / "output.json"
        gff3.write_text(GFF3, encoding="utf-8")
        vcf.write_text(VCF, encoding="utf-8")
        command = [
            str(args.fastvep),
            "annotate",
            "--input",
            str(vcf),
            "--output",
            str(output),
            "--gff3",
            str(gff3),
            "--output-format",
            "json",
            "--no-progress",
        ]
        subprocess.run(command, check=True, timeout=60)
        rows = json.loads(output.read_text(encoding="utf-8"))

    actual = {}
    for row in rows:
        consequences = row.get("transcript_consequences") or []
        if len(consequences) != 1:
            raise AssertionError(
                f"{row.get('id')}: expected one consequence row, got {len(consequences)}"
            )
        consequence = consequences[0]
        terms = consequence.get("consequence_terms") or []
        if len(terms) != 1:
            raise AssertionError(
                f"{row.get('id')}: expected one consequence term, got {terms}"
            )
        symbol = consequence.get("gene_symbol")
        if symbol in (None, "", "-"):
            symbol = None
        actual[row["id"]] = (symbol, terms[0], consequence.get("distance"))

    if actual != EXPECTED:
        missing = sorted(EXPECTED.keys() - actual.keys())
        extra = sorted(actual.keys() - EXPECTED.keys())
        mismatched = {
            key: {"expected": EXPECTED[key], "actual": actual.get(key)}
            for key in EXPECTED.keys() & actual.keys()
            if actual[key] != EXPECTED[key]
        }
        raise AssertionError(
            f"implicit fastVEP distance contract changed; missing={missing}, "
            f"extra={extra}, mismatched={mismatched}"
        )

    print(
        "fastVEP omitted --distance contract passed: 4,999 and 5,000 bp admitted, "
        "5,001 bp rejected on both transcript strands"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
