#!/usr/bin/env python3
"""Build a deterministic VCF from Ensembl transcript geometry, not fastVEP calls."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path


PRIMARY = {str(i) for i in range(1, 23)} | {"X", "Y", "MT"}
TRANSCRIPT_FEATURES = {
    "mRNA",
    "transcript",
    "lnc_RNA",
    "ncRNA",
    "miRNA",
    "snRNA",
    "snoRNA",
    "rRNA",
    "scRNA",
    "tRNA",
}


def attributes(raw: str) -> dict[str, str]:
    result = {}
    for item in raw.rstrip().split(";"):
        if "=" in item:
            key, value = item.split("=", 1)
            result[key] = value
    return result


def transcript_id(value: str | None) -> str | None:
    if not value:
        return None
    value = value.split(",", 1)[0]
    return value.removeprefix("transcript:") if value.startswith("transcript:") else None


def score(value: str) -> int:
    return int.from_bytes(hashlib.sha256(value.encode()).digest()[:8], "big")


@dataclass
class Transcript:
    id: str
    chrom: str
    start: int
    end: int
    strand: str
    biotype: str
    exons: list[tuple[int, int, int]] = field(default_factory=list)
    cds: list[tuple[int, int, int]] = field(default_factory=list)
    start_codons: list[tuple[int, int]] = field(default_factory=list)
    stop_codons: list[tuple[int, int]] = field(default_factory=list)

    @property
    def kind(self) -> str:
        return "coding" if self.cds else "noncoding"

    @property
    def exon_bin(self) -> str:
        return "1" if len(self.exons) == 1 else "2-5" if len(self.exons) <= 5 else "6+"

    @property
    def phases(self) -> str:
        values = sorted({phase for _, _, phase in self.cds if phase >= 0})
        return "-".join(map(str, values)) if values else "none"


class IndexedFasta:
    def __init__(self, fasta: Path):
        self.handle = fasta.open("rb")
        self.index = {}
        with Path(str(fasta) + ".fai").open(encoding="utf-8") as handle:
            for line in handle:
                name, length, offset, line_bases, line_width, *_ = line.rstrip().split("\t")
                self.index[name] = tuple(map(int, (length, offset, line_bases, line_width)))

    def base(self, chrom: str, position: int) -> str | None:
        item = self.index.get(chrom)
        if not item:
            return None
        length, offset, line_bases, line_width = item
        if position < 1 or position > length:
            return None
        zero = position - 1
        byte = offset + (zero // line_bases) * line_width + zero % line_bases
        self.handle.seek(byte)
        value = self.handle.read(1).decode("ascii").upper()
        return value if value in "ACGT" else None

    def sequence(self, chrom: str, start: int, end: int) -> str | None:
        values = [self.base(chrom, position) for position in range(start, end + 1)]
        return "".join(values) if all(values) else None


def read_candidates(gff: Path, modulus: int) -> list[Transcript]:
    selected: dict[str, Transcript] = {}
    opener = gzip.open if gff.suffix == ".gz" else open
    with opener(gff, "rt", encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("#"):
                continue
            columns = line.rstrip().split("\t")
            if len(columns) != 9 or columns[0] not in PRIMARY:
                continue
            chrom, _, feature, start, end, _, strand, phase, raw_attrs = columns
            attrs = attributes(raw_attrs)
            if feature in TRANSCRIPT_FEATURES:
                tid = transcript_id(attrs.get("ID"))
                if tid and (chrom in {"Y", "MT"} or score(tid) % modulus == 0):
                    selected[tid] = Transcript(
                        tid,
                        "chrM" if chrom == "MT" else f"chr{chrom}",
                        int(start),
                        int(end),
                        strand,
                        attrs.get("biotype", feature),
                    )
                continue
            tid = transcript_id(attrs.get("Parent"))
            transcript = selected.get(tid or "")
            if transcript is None:
                continue
            span = (int(start), int(end))
            if feature == "exon":
                transcript.exons.append((*span, int(attrs.get("rank", "0"))))
            elif feature == "CDS":
                transcript.cds.append((*span, int(phase) if phase in {"0", "1", "2"} else -1))
            elif feature == "start_codon":
                transcript.start_codons.append(span)
            elif feature == "stop_codon":
                transcript.stop_codons.append(span)
    return [item for item in selected.values() if item.exons]


def choose_transcripts(candidates: list[Transcript]) -> list[Transcript]:
    quotas = {
        ("coding", "+"): 8,
        ("coding", "-"): 8,
        ("noncoding", "+"): 4,
        ("noncoding", "-"): 4,
    }
    chosen = []
    for bucket, quota in quotas.items():
        members = [item for item in candidates if (item.kind, item.strand) == bucket]
        groups: dict[tuple[str, str, str], list[Transcript]] = defaultdict(list)
        for item in members:
            groups[(item.chrom, item.exon_bin, item.phases)].append(item)
        for values in groups.values():
            values.sort(key=lambda item: score(item.id))
        keys = sorted(groups, key=lambda value: score("|".join(value)))
        while len([item for item in chosen if (item.kind, item.strand) == bucket]) < quota:
            progressed = False
            for key in keys:
                if groups[key]:
                    chosen.append(groups[key].pop(0))
                    progressed = True
                    if len([item for item in chosen if (item.kind, item.strand) == bucket]) == quota:
                        break
            if not progressed:
                raise RuntimeError(f"not enough transcripts for {bucket}: wanted {quota}")
    for chrom in ("chrY", "chrM"):
        additions = sorted(
            (item for item in candidates if item.chrom == chrom and item not in chosen),
            key=lambda item: (item.kind != "coding", score(item.id)),
        )[:2]
        chosen.extend(additions)
    return chosen


def representative_spans(spans: list[tuple[int, ...]]) -> list[tuple[int, ...]]:
    if len(spans) <= 3:
        return spans
    ordered = sorted(spans)
    return [ordered[0], ordered[len(ordered) // 2], ordered[-1]]


def boundary_points(transcript: Transcript) -> dict[int, set[str]]:
    points: dict[int, set[str]] = defaultdict(set)
    points[transcript.start].add("transcript_start")
    points[transcript.end].add("transcript_end")
    for start, end, *_ in representative_spans(transcript.exons):
        points[start].add("exon_start")
        points[end].add("exon_end")
    for start, end, *_ in representative_spans(transcript.cds):
        points[start].add("cds_segment_start")
        points[end].add("cds_segment_end")
    if transcript.cds:
        low = min(start for start, _, _ in transcript.cds)
        high = max(end for _, end, _ in transcript.cds)
        coding_start, coding_stop = (low, high) if transcript.strand == "+" else (high, low)
        points[coding_start].add("coding_start_boundary")
        points[coding_stop].add("coding_stop_boundary")
    for label, spans in (("start_codon", transcript.start_codons), ("stop_codon", transcript.stop_codons)):
        for start, end in spans:
            points[start].add(f"{label}_start")
            points[end].add(f"{label}_end")
    return points


def add_variant(variants: dict, chrom: str, pos: int, ref: str | None, alt: str | None, note: dict):
    if not ref or not alt or ref == alt:
        return
    key = (chrom, pos, ref, alt)
    variants.setdefault(key, []).append(note)


def build_variants(transcripts: list[Transcript], fasta: IndexedFasta) -> dict:
    variants: dict[tuple[str, int, str, str], list[dict]] = {}
    alternate = {"A": "C", "C": "G", "G": "T", "T": "A"}
    for transcript in transcripts:
        for boundary, labels in boundary_points(transcript).items():
            common = {
                "transcript": transcript.id,
                "strand": transcript.strand,
                "biotype": transcript.biotype,
                "kind": transcript.kind,
                "exonBin": transcript.exon_bin,
                "phases": transcript.phases,
                "boundaries": sorted(labels),
            }
            for delta in (-2, -1, 0, 1, 2):
                position = boundary + delta
                ref = fasta.base(transcript.chrom, position)
                add_variant(
                    variants,
                    transcript.chrom,
                    position,
                    ref,
                    alternate.get(ref or ""),
                    {**common, "shape": "snv", "offset": delta},
                )
            ref = fasta.base(transcript.chrom, boundary)
            add_variant(
                variants,
                transcript.chrom,
                boundary,
                ref,
                (ref or "") + alternate.get(ref or "", ""),
                {**common, "shape": "insertion", "offset": 0},
            )
            deletion = fasta.sequence(transcript.chrom, boundary - 1, boundary)
            add_variant(
                variants,
                transcript.chrom,
                boundary - 1,
                deletion,
                deletion[:1] if deletion else None,
                {**common, "shape": "deletion", "offset": 0},
            )
            ref2 = fasta.sequence(transcript.chrom, boundary, boundary + 1)
            alt2 = "".join(alternate.get(base, "") for base in ref2 or "")
            add_variant(
                variants,
                transcript.chrom,
                boundary,
                ref2,
                alt2,
                {**common, "shape": "mnv2", "offset": 0},
            )
    return variants


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--gff", type=Path, required=True)
    parser.add_argument("--fasta", type=Path, required=True)
    parser.add_argument("--vcf", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--sample-modulus", type=int, default=128)
    args = parser.parse_args()

    candidates = read_candidates(args.gff, args.sample_modulus)
    transcripts = choose_transcripts(candidates)
    fasta = IndexedFasta(args.fasta)
    variants = build_variants(transcripts, fasta)

    ordered = sorted(variants, key=lambda item: (PRIMARY_ORDER.get(item[0], 99), item[1], item[2], item[3]))
    with args.vcf.open("w", encoding="utf-8", newline="\n") as handle:
        handle.write("##fileformat=VCFv4.2\n")
        handle.write("##reference=GRCh38\n")
        handle.write("##source=annocat-independent-transcript-geometry-20260904\n")
        handle.write("#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n")
        for index, (chrom, pos, ref, alt) in enumerate(ordered, 1):
            handle.write(f"{chrom}\t{pos}\tBOUNDARY{index}\t{ref}\t{alt}\t.\tPASS\t.\n")

    manifest = {
        "schemaVersion": 1,
        "selection": {
            "sampleModulus": args.sample_modulus,
            "candidateTranscripts": len(candidates),
            "selectedTranscripts": len(transcripts),
            "algorithm": "SHA-256 transcript sample, then balanced coding/strand/geometry strata",
        },
        "transcripts": [
            {
                "id": item.id,
                "chromosome": item.chrom,
                "start": item.start,
                "end": item.end,
                "strand": item.strand,
                "biotype": item.biotype,
                "kind": item.kind,
                "exonCount": len(item.exons),
                "exonBin": item.exon_bin,
                "cdsSegmentCount": len(item.cds),
                "phases": item.phases,
            }
            for item in transcripts
        ],
        "variantCount": len(ordered),
        "variants": [
            {
                "chromosome": chrom,
                "position": pos,
                "reference": ref,
                "alternate": alt,
                "probes": variants[(chrom, pos, ref, alt)],
            }
            for chrom, pos, ref, alt in ordered
        ],
    }
    args.manifest.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"transcripts": len(transcripts), "variants": len(ordered)}, indent=2))


PRIMARY_ORDER = {f"chr{i}": i for i in range(1, 23)} | {"chrX": 23, "chrY": 24, "chrM": 25}


if __name__ == "__main__":
    main()
