# fastVEP and Ensembl VEP 115 correctness work, 2026-09-04

Status: Implemented and verified in the local fastVEP working tree. Not yet
committed, pinned in AnnoCAT, packaged, or released.

This record documents the consequence, coordinate, and HGVS corrections made
on 2026-09-04. It records what changed, why it changed, what was compared, and
what remains before release qualification. It does not claim that Ensembl VEP
is an independent biological truth source. Under AnnoCAT's compatibility
policy, frozen Ensembl VEP 115 output is the required behavior unless an exact,
reviewed exception is approved.

## Working boundary

| Item | Value |
| --- | --- |
| fastVEP repository | `tools/fastVEP-concordance` |
| Branch | `codex/port-upstream-correctness` |
| Base commit | `78c870be81762f8cec0a020a76a0515cfdd1449c` |
| Primary oracle | Ensembl VEP release 115 frozen REST responses |
| Transcript source | Ensembl release 115 GRCh38 GFF3 |
| Reference | GRCh38 no-alt analysis-set FASTA |
| Transcript cache | `ensembl-115-candidate-rebuilt.cache` |
| Cache SHA-256 | `8A85E626F50CA61573403852576CE78D3244530ADDF806826679C02C762C78E4` |
| Local release binary SHA-256 | `04814149707F624A0D8CDD783721CEC85C18F0F6FC8DFB09CCB7DE17EAAF1248` |

The local release binary was built for verification only. The AnnoCAT fastVEP
pin and distributed binary were not changed.

The follow-up release strategy is now specified in
[Source-matched Ensembl VEP 115.2 qualification](vep115-source-matched-qualification.md).
That proposal makes official VEP 115.2 with the same GFF3 and FASTA the primary
implementation oracle and retains archived REST as a separate compatibility
lane. Its local workflow draft has not yet been run, so this record does not
claim source-matched qualification.

## Problems found

The released, pinned fastVEP build and the latest reviewed upstream build both
had material field-value differences from VEP 115. The differences were not a
single HGVSp defect. They involved region membership, consequence predicates,
coordinate projection, HGVS 3-prime normalization, and protein description.

Counts below are differing allele-transcript identities on the shared identity
set. They are not variant counts and must not be added across cohorts.

| Field | Boundary release-equivalent | Boundary upstream `8c6b179` | Boundary corrected | ClinVar release-equivalent | ClinVar upstream `8c6b179` | ClinVar corrected |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `Amino_acids` | 1,205 | 6 | 0 | 212 | 0 | 0 |
| `CDS_position` | 944 | 936 | 0 | 46 | 46 | 0 |
| `Codons` | 1,180 | 0 | 0 | 438 | 0 | 0 |
| `Consequence` | 2,519 | 397 | 0 | 181 | 29 | 0 |
| `EXON` | 783 | 783 | 0 | 51 | 51 | 0 |
| `HGVSc` | 4,486 | 849 | 0 | 365 | 10 | 0 |
| `HGVSp` | 2,539 | 2,717 | 0 | 379 | 1,186 | 0 |
| `IMPACT` | 1,345 | 92 | 0 | 61 | 18 | 0 |
| `INTRON` | 1,566 | 1,566 | 0 | 51 | 51 | 0 |
| `Protein_position` | 721 | 713 | 0 | 21 | 21 | 0 |
| `cDNA_position` | 1,229 | 1,154 | 0 | 59 | 59 | 0 |

`FLAGS` is discussed separately because the release-equivalent and upstream
comparison reports did not include it in the same mismatch table.

## Corrections made

### Region membership and exported positions

- Multi-base alleles now detect every intron they overlap rather than checking
  only the first genomic coordinate.
- An insertion is treated as the zero-length interval between its flanking
  bases. It receives an `EXON` or `INTRON` number only when both flanks belong
  to the same region. An insertion on an exon-intron boundary can still receive
  its coding and splice consequences without claiming membership in either
  region.
- cDNA, CDS, and protein output ranges are projected once from the normalized
  genomic endpoints using VEP's ordering rules. This corrects reverse-strand
  spans and insertion-boundary coordinates.
- Both the library annotation path and the CLI batch path use the same position
  projection and HGVSp functions. This removes two formerly duplicated output
  implementations that could drift independently.

### Consequence and impact predicates

- Coding, 5-prime UTR, and 3-prime UTR overlap tests are additive when a span
  reaches more than one region, including VEP's inclusive transcript-edge
  behavior.
- Same-length replacements crossing the stop/3-prime-UTR boundary follow VEP's
  generic `coding_sequence_variant` behavior rather than inferring a stop-loss
  protein result that VEP does not emit.
- VEP's stop-retained predicate for a length-changing replacement that keeps
  its first reference residue and introduces a later stop is reproduced. This
  can suppress an otherwise apparent frameshift because compatibility with VEP
  115 is the chosen product contract.
- `start_lost` and `start_retained_variant` are evaluated independently. An
  insertion can therefore report both when it displaces the active initiator
  but retains the original start codon in the edited sequence.
- The VEP-specific `c.1del` repeat case can report `frameshift_variant`,
  `start_lost`, and `start_retained_variant` together when the preceding UTR
  base preserves the CDS-length suffix.
- Unknown translated residues no longer erase a determinable missense or
  in-frame consequence. `coding_sequence_variant` is added independently when
  the remaining peptide content is unresolved.
- An annotated human mitochondrial initiator is treated as methionine in the
  reference protein even when its ordinary translation-table-2 residue would
  differ.

### HGVSc normalization

- Deletions are shifted on the genomic reference before the shifted endpoints
  are mapped back to cDNA, matching VEP's order of operations.
- Genomic 3-prime shifting can move a deletion between an exon and an intron,
  but it cannot incorrectly jump an intron by comparing two adjacent cDNA
  bases that are not adjacent in the genome.
- An indel whose maximal genomic 3-prime shift moves beyond the transcript is
  suppressed instead of being forced into a terminal transcript HGVS value.
- A normalized allele must fit completely inside the transcript slice before
  transcript HGVS is emitted.
- Non-coding transcripts still do not store spliced sequences in the transcript
  cache. When an insertion needs sequence for the HGVS 3-prime rule or
  duplication detection, the sequence is constructed for that annotation only
  and discarded. It is not written to the cache.
- A deletion that shifts from an exonic representation to an intronic one is
  routed to the intronic renderer. An unmapped deletion is not promoted into a
  later exon merely because a repeat exists there.

### HGVSp generation

- HGVSp generation is centralized in one shared function used by both
  annotation paths.
- Protein HGVS is normally emitted only when the normalized HGVSc reaches a
  coding coordinate. The required VEP exception for a one-sided reverse-strand
  insertion is retained.
- A start-loss result is rendered as an uncertain protein consequence such as
  `p.Met1?`, including when a cached peptide cannot safely normalize the edit.
- Stop-loss extensions apply the allele to CDS plus downstream sequence and
  count to the next translated stop. If no later stop is available, the result
  uses `extTer?`.
- A coding deletion that crossed a splice boundary and lacked the ordinary
  amino-acid window can derive its in-frame protein deletion from the shifted
  exonic span.
- Stop-retained frameshift-shaped windows that translate to stop on both sides
  use the synonymous terminator form.
- Terminal unknown residues are interpreted according to the VEP context:
  terminator-preserving windows use `Ter`, while a two-residue window with one
  real substitution is reduced to that substitution, for example
  `Phe170_Xaa171delinsLeuXaa` becomes `Phe170Leu`.
- Multi-residue synonymous windows use VEP 115's emitted form, such as
  `p.SerTer22=`.
- Mitochondrial frameshift HGVSp uses the mitochondrial genetic code for the
  reference residue and VEP's ordinary alternate-extension translation
  behavior for the shifted sequence. The special mitochondrial deletion of an
  annotated terminator is narrowly rendered as `delextTer?`; this rule is not
  applied to nuclear stop-codon deletions.

## Implementation map

- `crates/fastvep-genome/src/transcript.rs`: range-based intron overlap.
- `crates/fastvep-consequence/src/predictor.rs`: insertion-boundary region
  membership, additive consequence predicates, start/stop behavior, unknown
  coding residues, and mitochondrial initiators.
- `crates/fastvep-annotate/src/hgvs_normalize.rs`: genomic 3-prime shifting,
  exon/intron transitions, transcript-boundary suppression, and temporary
  non-coding transcript sequence construction.
- `crates/fastvep-annotate/src/lib.rs`: shared VEP-style position projection,
  centralized HGVSp selection, HGVSc eligibility, and splice-boundary protein
  deletion recovery.
- `crates/fastvep-hgvs/src/protein.rs`: stop-loss extension, mixed translation
  tables for VEP-compatible mitochondrial frameshifts, terminator-preserving
  output, and VEP multi-residue synonymous formatting.
- `crates/fastvep-hgvs/src/lib.rs`: exports for the shared protein-HGVS path.
- `crates/fastvep-cli/src/pipeline.rs`: use of the shared position and HGVSp
  functions in batch annotation.

## Verification performed

The final binary was invoked with explicit `--hgvs`; `--everything` alone does
not enable HGVS in this CLI.

```powershell
cargo test -p fastvep-genome -p fastvep-consequence -p fastvep-hgvs `
  -p fastvep-annotate -p fastvep-cli --no-fail-fast

cargo build --release -p fastvep-cli

fastvep annotate --input <corpus.vcf> --output <candidate.vcf> `
  --transcript-cache <ensembl-115.cache> --fasta <grch38.fna> `
  --everything --hgvs --no-progress

python scripts/compare-vep-concordance.py <candidate.vcf> `
  <frozen-vep-115-response.json> --oracle-format rest-json --json <report.json>
```

Results:

| Check | Result |
| --- | --- |
| Affected Rust test suite | 276 passed, 0 failed |
| Release build | Passed |
| `git diff --check` | Passed |
| Boundary corpus | 1,262 records; 41,720 shared identities; zero non-`FLAGS` field mismatches |
| Reviewed ClinVar corpus | 400 records; 9,801 shared identities; zero non-`FLAGS` field mismatches |
| Provenance comparison | Every changed shared field was a correction relative to the frozen VEP result; no current-only wrong field category was observed |
| Final output stability | Final allocation cleanup produced byte-identical VCFs |

The final reports are stored locally under
`target/fastvep-boundary-clinvar-oracle-20260904/`:

- `boundary-hgvsp-final-analysis.json`
- `clinvar-reviewed-hgvsp-final-analysis.json`
- `boundary-hgvsp-stage8-provenance.json`
- `clinvar-reviewed-hgvsp-stage8-provenance.json`

Artifact identities:

| Artifact | SHA-256 |
| --- | --- |
| `boundary.vcf` | `EA912B66C7DFDC127C038FAAFFECE824D78ED2873997D9CF7255290AFDD58917` |
| Boundary VEP 115 response | `F597493CCAC6329BC0E3628E264F124D384639678111A04807F284B66B1F982B` |
| Final boundary VCF | `3BBF7B32DBB864E257FC1108AD878E250030CE971D5805EC7ECC81D866767E7D` |
| `clinvar-reviewed.vcf` | `3DE468D953892D970C132A3FCB48F2E5E7E98345DC10CEB25874F624DAF6D4DE` |
| ClinVar VEP 115 response | `FD1E0C3A9CD5DE6ADD2D021C165FB1E7AC0A2EB684D30DBD238A6BAE10406CBA` |
| Final ClinVar VCF | `2AB22476B1D4A9FD52356A5E67D1B3DC45197252022D414F72B75F50F6565FAA` |

The affected-package `cargo clippy --all-targets` run completed with the
repository's existing warnings. Treating every warning as an error remains
blocked by pre-existing Clippy findings outside this correction set, beginning
with the existing `Allele::from_str` API warning. Those warnings were not
changed as part of the VEP-concordance work.

## Compatibility and performance effects

- No transcript-cache schema or serialization field changed. Existing AnnoCAT
  transcript caches remain readable and can be used for new annotations.
- No OSA/OSA2 supplementary-source schema, source lookup, FAVOR annotation, or
  result-file schema changed.
- Existing result files remain readable and are not rewritten. Reannotation is
  required to receive the corrected values.
- The only new on-demand sequence construction is limited to a non-coding
  transcript insertion that needs sequence-based HGVS normalization and lacks a
  cached spliced sequence. The temporary sequence is released after the
  annotation and is never persisted.
- The terminal-residue comparison uses an iterator and does not allocate a
  temporary result list.
- The fastVEP distance option and AnnoCAT invocation were not changed.

## Remaining release blockers and limits

This working tree is not yet fully VEP-qualified.

1. `FLAGS` still differs on 4,451 boundary and 590 ClinVar shared identities.
   The public Ensembl 115 GFF3 does not carry all transcript-completeness
   metadata represented by the VEP response. The decision for this work was not
   to change the transcript-cache schema or require users to rebuild installed
   data. `FLAGS` therefore remains unresolved rather than being silently
   treated as concordant.
2. The candidate has 16 boundary and 52 ClinVar allele-transcript identities
   that are absent from the frozen REST responses. The same identities are
   present in the release-equivalent and reviewed upstream outputs, so these
   corrections did not introduce them. They still require either root-cause
   resolution or exact reviewed compatibility-contract entries before a strict
   identity-presence gate can pass.
3. Passing the frozen corpora shows compatibility for their covered variants;
   it is not proof of correctness for every possible human variant.
4. No AnnoCAT pin, packaged executable, release ZIP, commit, or remote branch
   was updated by this work.

## Release follow-up

Before adoption, commit the fastVEP changes, update the AnnoCAT pin and binary
identity through the normal reviewed procedure, repeat direct-GFF versus cache
parity, run the packaged AnnoCAT end-to-end gate, and resolve or formally
contract the two remaining qualification items above.
