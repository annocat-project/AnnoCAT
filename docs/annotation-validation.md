# Annotation validation

Status: Implemented for the legacy consequence-concordance corpus,
supplementary-source cache parity, genotype preservation, selected evidence,
and result projection. The 2026-09-04 local fastVEP candidate has zero
non-`FLAGS` field-value mismatches on the shared identities in both expanded
corpora. Full release qualification remains blocked by `FLAGS` metadata and
candidate-only transcript identities absent from the REST oracle.

Last updated: 2026-09-04

Applies to fastVEP output, OSA1 and OSA2 annotation caches, result conversion,
evidence selection, queries, Variant Details, online annotation fixtures, and
export.

AnnoCAT is for research and educational use only. For fastVEP-generated
annotations, the primary qualification target is compatibility with pinned
Ensembl VEP 115 under the same assembly, reference, transcript release, input
representation, and options. AnnoCAT does not attempt to replace VEP with an
independent consequence interpretation engine. Independent sequence or HGVS
analysis is used only to diagnose an apparent VEP inconsistency and to support
a narrowly reviewed exception; it is not a competing release oracle.

For supplementary sources, these checks show whether AnnoCAT faithfully
applies declared source records and selection rules. Neither validation lane
shows that an external assertion or predictor is biologically true.

## Validation model

Use the smallest coverage-driven corpus that exercises every supported
representation, evidence scope, source contract, and known failure mode. A
target number of variants or an aggregate match percentage is not an acceptance
criterion.

Expected fastVEP values come from the frozen Ensembl VEP 115 response produced
with the declared compatibility inputs and options. They must not be generated
by AnnoCAT, fastVEP, an AnnoCAT cache, or the parser under test. Expected
supplementary-source values must likewise come from an independently extracted,
pinned source record.

Validation has three lanes:

1. **Pull-request fixtures** project independently authored expected records
   through canonical result data, evidence selection, queries, filters, sorts,
   and Variant Details.
2. **VEP compatibility qualification** compares the pinned fastVEP build with
   frozen responses from Ensembl VEP 115 for public legacy, boundary, and
   reviewed-ClinVar corpora.
3. **Release validation** samples pinned raw source releases, projects their
   expected records through AnnoCAT, and checks packaged and aggregate
   whole-genome behavior.

[Source validation in GitHub Actions](github-actions-source-validation.md)
defines the proposed hosted release gate for the third lane.

`annocat results validate` remains an integrity check. It validates files,
schemas, and hashes without modifying the result. It is not a source-value or
biological validation command.

## Current coverage

### Ensembl VEP 115 compatibility

The proposed release qualification separates implementation agreement from
source-data agreement. Official Ensembl VEP 115.2 run against the same public
GFF3 and FASTA is the primary implementation oracle. Archived REST remains a
secondary compatibility lane because it uses a different transcript dataset.
The complete proposed contract and its current implementation status are in
[Source-matched Ensembl VEP 115.2 qualification](vep115-source-matched-qualification.md).

The manual `Annotation concordance` workflow builds the fastVEP revision pinned
by AnnoCAT. It also builds a transcript cache from the pinned Ensembl 115 GFF3
and GRCh38 FASTA, then requires cache-backed output to be byte-identical to
direct-GFF output.

The legacy 197-record corpus is submitted to the archived September 2025
Ensembl REST endpoint, which reports release 115. It covers chromosomes 21,
22, X, Y, and MT; SNVs, MNVs, insertions, deletions, and mixed multiallelic
records; coding, non-coding, UTR, intronic, splice, and intergenic effects; both
strands; mitochondrial start uncertainty; and a public SELENOO selenocysteine
substitution. For `ENST00000380903.7:c.2001A>C`, VEP 115 requires
`missense_variant`, `MODERATE`, `U/C`, and
`ENSP00000370288.2:p.Sec667Cys`.

The legacy corpus contains more than 3,400 comparable allele-transcript
identities. It has no missing identities, extra identities, or mapped-field
differences after two narrow input-contract rules are applied:

- Ensembl's public release-115 GFF3 does not contain the transcript-end
  completeness metadata needed for `cds_start_NF` and `cds_end_NF`, so `FLAGS`
  is excluded from the GFF-to-REST comparison.
- Three current Ensembl 115 transcripts present in the GFF3 and transcript
  lookup service are absent from the REST consequence response for one allele.
  The contract permits only those exact allele-transcript identities. It does
  not use gene-wide, transcript-wide, or wildcard suppression.

The corpus represents alternate alleles independently for HGVS comparison.
This follows AnnoCAT's one-row-per-alternate-allele result contract and avoids a
documented Ensembl VEP ambiguity for some unsplit multiallelic inputs.

This legacy result does not imply that the expanded qualification corpora pass.
The expanded frozen qualification set contains 1,262 transcript-geometry
boundary records and 400 stratified, reviewed ClinVar records. The 2026-09-04
local candidate has zero non-`FLAGS` field-value mismatches over 41,720 shared
boundary identities and 9,801 shared ClinVar identities. It is still not fully
VEP-qualified: `FLAGS` differs on 4,451 and 590 shared identities respectively,
and 16 boundary plus 52 ClinVar candidate identities are absent from the REST
responses. Those candidate-only identities also occur in the release-equivalent
and reviewed upstream outputs, so the current corrections did not introduce
them, but they still require resolution or exact reviewed contract entries.
The complete correction and verification record is
[fastVEP and Ensembl VEP 115 correctness work, 2026-09-04](fastvep-vep115-correctness-2026-09-04.md).
Counts from the three corpora must be reported separately; affected input
alleles, allele-transcript-field differences, and root-cause classes are
different measurements and must not be added together or described
interchangeably.

This lane verifies VEP 115 compatibility for its frozen public corpora. It does
not prove sex-aware genotype interpretation, complete PAR behavior, every
mitochondrial allele class, or correctness for every possible human variant.

### Difference reconciliation

Every VEP difference is handled as a compatibility defect until it is resolved
or explicitly excepted:

1. Freeze the candidate and release binaries, fastVEP revisions, transcript
   caches, Ensembl 115 GFF3, GRCh38 FASTA, VEP options, corpus, and response
   hashes.
2. Compare normalized allele and transcript identities field by field. Report
   affected alleles, differing identities, and root-cause classes separately.
3. Cluster a difference into a reproducible class and reduce it to the smallest
   representative allele without changing the behavior.
4. Add a failing fixture for the VEP 115 value, then correct the shared root
   cause rather than adding per-variant behavior.
5. Rerun the minimized fixture, every previously passing fixture, and all frozen
   release corpora.
6. Accept the change only when it introduces no new unexplained difference
   outside the reviewed expected set of coupled fields.

Consequence, `IMPACT`, amino-acid, codon, HGVSc, HGVSp, position, exon/intron,
and transcript-presence differences are all part of this gate. Aggregate match
percentages and provenance agreement with upstream fastVEP do not make a VEP
115 difference acceptable.

Official HGVS rules, VariantValidator, Mutalyzer, transcript sequence
reconstruction, and other independent analysis may explain why VEP produced a
result or identify a suspected VEP defect. By default, AnnoCAT still matches the
pinned VEP 115 result. A deliberate divergence requires an exact, reviewed
contract entry identifying the allele, transcript, field, observed VEP value,
AnnoCAT value, supporting evidence, and rationale. Wildcard, gene-wide, and
transcript-wide suppressions are prohibited.

### Supplementary-source parity

`fixtures/source-cache-parity` contains schema-faithful synthetic source rows
and independently authored expected records for:

- ClinVar;
- gnomAD;
- dbSNP;
- CADD;
- PhyloP;
- REVEL;
- SpliceAI; and
- dbNSFP.

The parity script builds temporary OSA1 and OSA2 caches with the pinned fastVEP
binary. Both readers must return the same logical source-record multiset and the
same selected evidence. The fixture includes repeated dbNSFP keys, transcript
vectors, Number=A and Number=R fields, signed SpliceAI positions,
ambiguous-reference keys, and neighboring alleles with no evidence.

Tiny synthetic caches exercise the production builders and readers. They do
not replace release sampling against pinned upstream source subsets.

### Result projection

Rust integration tests project the expected source records through:

- normalized allele identity;
- canonical variant, consequence, and evidence Parquet;
- representative gene and transcript selection;
- direct and selected evidence rows;
- table display values;
- filtering and sorting;
- Variant Details; and
- missing-versus-zero handling.

Export query behavior has separate result-query tests. A complete semantic
export fixture is not yet part of this corpus.

### Sample calls and recovery

Unit and integration tests cover genotype allele indexes, phasing, partial and
missing calls, haploid calls, multiple samples, sex-chromosome representation,
and fresh-versus-resumed logical row equivalence. Aggregate GIAB runs cover
throughput, memory, recovery, and result invariants without making GIAB an
oracle for third-party annotation values.

## Acceptance gates

The semantic corpus passes only when:

1. normalized allele identity agrees for every supported allele;
2. every expected consequence exists and no unsupported extra consequence is
   silently promoted;
3. every exact source field matches its independently extracted value;
4. every quantized field stays within its declared raw bound and is exact after
   cache lookup;
5. no allele, gene, transcript, or source record leaks into another identity;
6. no missing value becomes zero, an empty value, or a display label;
7. selected transcript and selected evidence identities agree;
8. direct, selected, detail, query, and export values are semantically equal;
9. OSA1 and OSA2 produce the same logical records where both are supported;
10. fresh and resumed annotations produce equivalent logical rows; and
11. every VEP 115 difference is fixed or named by a narrow reviewed
    compatibility-contract entry; and
12. no previously concordant identity gains an unexplained difference outside
    the reviewed expected set of coupled fields.

There is no acceptable unexplained mismatch percentage. Aggregate agreement
rates can summarize a run, but they do not replace field-level gates.

## Test cadence

### Pull requests

- Run the committed source-contract result-projection tests.
- Run result integrity, evidence resolver, sample-call, and presentation tests.
- Run minimized fixtures for every reconciled VEP difference class.
- Do not require large installed sources, live APIs, or whole-genome fixtures.

### Release candidates

- Build the exact fastVEP revision in `config/fastvep-pin.json`.
- Run the legacy 197-record corpus and the frozen 1,262-boundary and
  400-ClinVar VEP 115 corpora.
- Require field-level VEP compatibility; do not substitute an aggregate match
  threshold.
- Build and verify the small OSA1 and OSA2 source-contract caches.
- Sample pinned raw source releases against independently authored expected
  records.
- Run the packaged end-to-end corpus.
- Freeze and replay online-source responses instead of using a live response as
  a release gate.

### Quarterly compatibility sweep

- Run a larger reviewed-ClinVar differential corpus and representation-pair,
  boundary, and metamorphic probes against frozen VEP 115 responses.
- Triage every newly observed difference and promote each new root-cause class
  into a minimized offline fixture.

### Whole-genome benchmark

- Use an approved public GIAB input for throughput, memory, resume, and
  aggregate result invariants.
- Compare deterministic aggregate counts and sampled annotations.
- Do not place whole-genome row values in logs, review reports, or model
  context.

## Reproducibility record

Each semantic validation record includes:

- AnnoCAT and fastVEP versions and commits;
- candidate, prior-release, and packaged executable identities and hashes;
- reference, transcript, and source release identities and hashes;
- Ensembl VEP release, endpoint or local build, invocation options, and frozen
  response hash;
- field-selection and evidence-calibration contract hashes;
- OSA format and converter identity;
- normalization and representative-selection versions;
- fixture and VEP compatibility-reference identities;
- fresh, resumed, cache-backed, and direct-GFF outcomes; and
- exact allowed-difference contract entries.

The record identifies what was tested. It does not validate the source's
scientific assertions.

## Implementation map

- `crates/annocat-core/src/normalization.rs`: allele normalization
- `crates/annocat-core/src/sample_call.rs`: genotype and ploidy preservation
- `crates/annocat-cli/src/results.rs`: result projection and query assertions
- `fixtures/source-cache-parity/`: supplementary-source contracts
- `fixtures/fastvep/ensembl-115-xy-mt.vcf`: consequence boundary cases
- `scripts/verify-supplementary-cache-parity.py`: OSA1 and OSA2 parity
- `scripts/compare-vep-concordance.py`: exact Ensembl comparison
- `.github/workflows/annotation-concordance.yml`: reproducible manual gate
- `config/fastvep-pin.json`: annotation engine identity
- `config/source-catalog.json`: source releases and scope
- `config/evidence-calibrations.json`: interpretation identities and thresholds
- `web/tests/evidence-display-policy.test.mjs`: presentation rules

## Outside this claim

- Source curation and predictor truth
- Diagnostic or patient-care validity
- Exhaustive structural-variant or copy-number annotation
- Complete sex-aware ploidy, PAR, and mitochondrial coverage
- Live-service availability or stability
- Restricted source content not represented by an approved fixture
- Phenotype, condition, gene-identity, and pathway curation beyond their own
  source-contract tests
