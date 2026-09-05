#!/usr/bin/env python3
"""Validate qualification VCF reference alleles and record identities."""

import argparse
import hashlib
import json
import tempfile
from collections import Counter
from pathlib import Path


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


class IndexedFasta:
    def __init__(self, path):
        self.path = path
        self.handle = path.open("rb")
        self.index = {}
        index_path = Path(str(path) + ".fai")
        for line_number, line in enumerate(index_path.read_text().splitlines(), 1):
            columns = line.split("\t")
            if len(columns) < 5:
                raise ValueError(f"{index_path}:{line_number}: invalid FASTA index row")
            name = columns[0]
            if name in self.index:
                raise ValueError(f"{index_path}:{line_number}: duplicate contig {name}")
            self.index[name] = tuple(int(value) for value in columns[1:5])

    def close(self):
        self.handle.close()

    def resolve(self, name):
        candidates = [name]
        if name.startswith("chr"):
            candidates.append(name[3:])
        else:
            candidates.append(f"chr{name}")
        if name in {"M", "MT", "chrM", "chrMT"}:
            candidates.extend(("MT", "M", "chrM", "chrMT"))
        for candidate in candidates:
            if candidate in self.index:
                return candidate
        raise ValueError(f"reference FASTA has no contig matching {name}")

    def fetch(self, name, start, length):
        resolved = self.resolve(name)
        contig_length, offset, line_bases, line_width = self.index[resolved]
        if start < 1 or length < 1 or start + length - 1 > contig_length:
            raise ValueError(f"{name}:{start}-{start + length - 1} is outside the reference")
        zero_based = start - 1
        remaining = length
        chunks = []
        while remaining:
            line_position = zero_based % line_bases
            take = min(remaining, line_bases - line_position)
            byte_offset = offset + (zero_based // line_bases) * line_width + line_position
            self.handle.seek(byte_offset)
            chunk = self.handle.read(take)
            if len(chunk) != take:
                raise ValueError(f"cannot read {name}:{start}-{start + length - 1}")
            chunks.append(chunk)
            zero_based += take
            remaining -= take
        return b"".join(chunks).decode("ascii").upper()


def validate_vcf(path, fasta):
    records = 0
    validated = 0
    skipped = Counter()
    identity = hashlib.sha256()
    skipped_identity = hashlib.sha256()
    mismatches = []
    with path.open(encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, 1):
            if line.startswith("#"):
                continue
            columns = line.rstrip("\r\n").split("\t")
            if len(columns) < 8:
                raise ValueError(f"{path}:{line_number}: invalid VCF row")
            chrom, pos_text, identifier, reference, alternate = (
                columns[0], columns[1], columns[2], columns[3], columns[4]
            )
            records += 1
            record_identity = f"{chrom}\t{pos_text}\t{identifier}\t{reference}\t{alternate}\n"
            identity.update(record_identity.encode())
            alts = alternate.split(",")
            if all(alt in {".", "*", "<NON_REF>"} for alt in alts):
                skipped["non-variant"] += 1
                skipped_identity.update(record_identity.encode())
                continue
            if any(alt.startswith("<") or "[" in alt or "]" in alt for alt in alts):
                skipped["unsupported-symbolic"] += 1
                skipped_identity.update(record_identity.encode())
                continue
            try:
                position = int(pos_text)
                observed = fasta.fetch(chrom, position, len(reference))
            except ValueError as error:
                mismatches.append({"line": line_number, "record": record_identity.rstrip(), "error": str(error)})
                continue
            if observed != reference.upper():
                mismatches.append(
                    {
                        "line": line_number,
                        "record": record_identity.rstrip(),
                        "expectedReference": observed,
                        "submittedReference": reference,
                    }
                )
                continue
            validated += 1
    return {
        "path": str(path),
        "sha256": sha256(path),
        "records": records,
        "validatedReferenceRecords": validated,
        "skipped": dict(sorted(skipped.items())),
        "recordIdentitySha256": identity.hexdigest(),
        "skippedIdentitySha256": skipped_identity.hexdigest(),
        "referenceMismatches": mismatches,
        "passed": not mismatches and validated + sum(skipped.values()) == records,
    }


def self_test():
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        fasta_path = root / "reference.fa"
        fasta_path.write_bytes(b">1\nACGT\nACGT\n")
        Path(str(fasta_path) + ".fai").write_text("1\t8\t3\t4\t5\n", encoding="ascii")

        valid_vcf = root / "valid.vcf"
        valid_vcf.write_text(
            "##fileformat=VCFv4.2\n"
            "#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n"
            "chr1\t2\tvalid\tC\tT\t.\tPASS\t.\n"
            "1\t3\tnonvariant\tG\t.\t.\tPASS\t.\n",
            encoding="ascii",
        )
        invalid_vcf = root / "invalid.vcf"
        invalid_vcf.write_text(
            "##fileformat=VCFv4.2\n"
            "#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n"
            "1\t2\tbad-ref\tG\tT\t.\tPASS\t.\n",
            encoding="ascii",
        )

        fasta = IndexedFasta(fasta_path)
        try:
            valid = validate_vcf(valid_vcf, fasta)
            invalid = validate_vcf(invalid_vcf, fasta)
        finally:
            fasta.close()

        assert valid["passed"]
        assert valid["validatedReferenceRecords"] == 1
        assert valid["skipped"] == {"non-variant": 1}
        assert not invalid["passed"]
        assert invalid["referenceMismatches"][0]["expectedReference"] == "C"
    print("VEP qualification input validator self-test passed")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("vcf", nargs="*", type=Path)
    parser.add_argument("--fasta", type=Path)
    parser.add_argument("--json", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return
    if not args.vcf or args.fasta is None or args.json is None:
        parser.error("VCF inputs, --fasta, and --json are required")

    fasta = IndexedFasta(args.fasta)
    try:
        reports = [validate_vcf(path, fasta) for path in args.vcf]
    finally:
        fasta.close()
    report = {
        "schemaVersion": 1,
        "fasta": {"path": str(args.fasta), "sha256": sha256(args.fasta)},
        "inputs": reports,
        "passed": all(item["passed"] for item in reports),
    }
    args.json.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2, sort_keys=True))
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
