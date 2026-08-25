# Source validation in GitHub Actions

Status: Proposed release gate

Last updated: 2026-08-24

This document defines how AnnoCAT can validate its supported annotation sources
entirely on GitHub-hosted Actions runners. It supplements
[Annotation validation](annotation-validation.md), which defines the scientific
acceptance rules.

## Decision

Use small, deterministic subsets of each supported source. Validate the sources
in parallel with a GitHub Actions matrix. Each source gets a fresh runner and
must pass a linked validation chain from the pinned raw records to AnnoCAT's
exported values.

A self-hosted runner and complete production caches are not required for this
release gate.

The required chain is:

```text
pinned raw subset
  -> OSA1 and OSA2 caches
  -> fastVEP structured output
  -> AnnoCAT production result conversion
  -> result Parquet
  -> selected evidence
  -> table, filter, sort, and Variant Details queries
  -> CSV export
```

The cache check, result-projection check, and packaged-application smoke check
are separate jobs linked by commit identities and file hashes. A test-only
source manifest must not replace or weaken AnnoCAT's production source
completeness checks.

The gate proves that AnnoCAT interprets representative records from each pinned
source correctly. It does not prove that every record in a complete upstream
release is scientifically correct or that a full upstream archive is complete.

## Current and proposed coverage

The current `fixtures/source-cache-parity/` corpus and
`verify-supplementary-cache-parity.py` script cover eight logical source
contracts. The script currently runs in the manually triggered
`annotation-concordance.yml` workflow; normal CI runs the Rust result-projection
test over the same expected fixture. The proposed release matrix extends those
checks to pinned raw subsets for the managed, cache-backed sources that AnnoCAT
can currently install:

- ClinVar;
- dbSNP;
- gnomAD exomes;
- gnomAD genomes;
- dbNSFP;
- CADD;
- PhyloP;
- REVEL; and
- SpliceAI.

gnomAD exomes and genomes are separate matrix entries because they are separate
installed releases, even though they use the same logical field contract.

Core Ensembl consequence generation remains in
`annotation-concordance.yml`. Catalog entries marked `catalog-pending`,
`adapter-required`, or `user-supplied-licensed` do not enter this release gate
until AnnoCAT has an implemented delivery and validation contract for them.

The HPO/MONDO knowledge resource and Reactome are managed resources, but they
are not fastVEP variant-annotation providers. Their ontology, gene-link, and
pathway contracts remain in the phenotype and gene-list validation suites.

FAVOR remains covered by frozen response fixtures because it is an online
service rather than a local cache. A live FAVOR response must not be a release
gate.

## Subset design

Subsets are coverage-driven, not random. Each source subset includes the
representations that its parser, cache contract, or evidence scope can change.
Across the matrix, the corpus covers:

- autosomes, X, Y, pseudoautosomal regions, and mitochondrial alleles;
- SNVs, insertions, deletions, MNVs, and multiallelic records;
- positive source matches and neighboring alleles with no match;
- missing values, zero values, signed values, and source boundary values;
- forward- and reverse-strand consequences;
- allele-level, position-level, gene-level, and transcript-level evidence;
- repeated allele keys and transcript vectors;
- VCF `Number=A` and `Number=R` fields;
- ambiguous or non-ACGT reference records where the source supports them;
- OSA chunk boundaries and duplicate-key behavior; and
- known regressions recorded as named fixtures.

Every expected value is extracted independently from the pinned raw subset. The
AnnoCAT parser, fastVEP output, or an existing AnnoCAT cache must not generate
the expected record.

Each subset has a manifest containing:

- source name and release;
- upstream location and retrieval date;
- publisher digest, ETag, or immutable parent-release identity;
- raw subset SHA-256;
- selection rule, genomic intervals, and extraction-tool identity;
- expected-record SHA-256;
- field-selection contract SHA-256;
- applicable source license or redistribution restriction; and
- the AnnoCAT fields exercised by the subset.

Every field from a matrix source that AnnoCAT exposes by default, permits
through field selection, uses in a summary, or includes in export must appear
in at least one subset expectation. A field inventory check fails when such a
field has no fixture coverage.

Redistributable subsets can be committed under `fixtures/`. Restricted data
must be fetched during a protected workflow or read from a private GitHub
repository when its terms permit that use. Restricted records must not be
uploaded as public artifacts or written to workflow logs.

Prefer an immutable, pre-extracted validation pack so release validation does
not download a complete upstream archive. When a source supports indexed region
queries, the workflow can fetch the pinned intervals directly. Otherwise it can
stream the upstream file through an extractor without retaining the full file,
or use a private pre-extracted pack when the source terms permit it. Regenerate
and review the pack whenever the pinned source release changes.

## Workflow design

Keep the workflow structure small:

1. `ci.yml` keeps the current Rust and browser tests, including the synthetic
   source-contract result-projection test.
2. `annotation-concordance.yml` keeps the current synthetic OSA parity check and
   pinned Ensembl consequence oracle, and becomes callable with `workflow_call`.
3. `windows-release.yml` builds one candidate ZIP and exposes it as a workflow
   artifact without publishing it.
4. `source-release-validation.yml` runs raw-to-cache and result-projection
   checks as a parallel matrix on `windows-latest`.
5. A combined-source job tests source coexistence and both supported gnomAD
   profile choices.
6. A candidate smoke job checks the packaged executable, core annotation,
   result integrity, and export.
7. A final publish job releases that exact candidate only after CI, consequence
   concordance, every source entry, combined-source validation, and the smoke
   test pass.

Do not set `max-parallel` for the source matrix:

```yaml
strategy:
  fail-fast: false
  matrix:
    source:
      - clinvar
      - dbsnp
      - gnomad
      - gnomad-genomes
      - dbnsfp
      - cadd
      - phylop
      - revel
      - spliceai
```

GitHub provisions a clean runner and separate storage for each matrix entry.
Running entries together therefore does not combine their disk use. Each subset
only needs to fit its own runner. Separate runners also prevent one source's
files or environment from affecting another source.

The candidate AnnoCAT and pinned fastVEP binaries are built once and uploaded as
a small workflow artifact. Rebuilding them in every source job would add time
without increasing coverage. Publishing the same candidate that passed the
packaged-application smoke check avoids a build-after-validation gap.

Set `max-parallel` only if an upstream host, credential, or account imposes a
measured concurrency limit. Apply that limit to the affected acquisition job;
do not serialize unrelated source validation.

The release workflow calls the validation workflows as jobs and uses `needs` to
make publication structurally dependent on them. It must not infer release
readiness by querying the status of an earlier workflow run.

Give every network, build, and validation job an explicit `timeout-minutes` so
a hung download or parser fails instead of holding the release indefinitely.
Use `if: always()` only for the small diagnostic-report upload step.

## Per-source procedure

Each matrix entry performs the same operations:

1. Check out the exact AnnoCAT commit.
2. Download the candidate containing the fastVEP revision in
   `config/fastvep-pin.json`.
3. Obtain and verify the pinned source subset.
4. Build every supported cache format for that source.
5. Verify cache structure before annotation.
6. Annotate the common allele corpus.
7. Compare fastVEP's structured output with the independently authored source
   expectation.
8. Pass the verified structured output to AnnoCAT's existing production result
   conversion test harness.
9. Query table values, exact filters, sorts, Variant Details, and exports.
10. Compare every projected value with the independently authored expectation.
11. Upload only the validation report, hashes, versions, and failure summary.

Where OSA1 and OSA2 are both supported, their logical source-record multisets
and selected values must be equal. Differences caused by a declared lossy field
must stay within its documented bound and must be checked after cache lookup.

### Production source boundary

`annocat sources install` verifies complete pinned releases and all expected
chromosome shards. The subset workflow must not call it with an incomplete
release, inject an alternate production manifest, hand-write verified
checkpoints, or relax source readiness checks.

Instead, fastVEP's production `sa-build`, `sa-verify`, and `annotate` commands
validate raw-source parsing and cache lookup. The resulting structured output
then enters the same AnnoCAT conversion, evidence-selection, query, detail, and
export functions used by completed annotations. Existing preparation tests
remain responsible for archive handling, resume behavior, shard publication,
and complete-manifest enforcement.

This boundary tests annotation meaning without teaching a release binary to
accept incomplete annotation installations.

## Combined-source validation

Per-source jobs cannot detect source alias collisions, provider-order changes,
field-index collisions, or summary behavior that appears only when several
sources are selected. After the matrix passes, one job rebuilds the small caches
and annotates the common corpus with the supported sources together.

Run the combined check twice: once with gnomAD exomes and once with gnomAD
genomes. AnnoCAT intentionally does not select both releases in one annotation.
The combined check requires stable source identities, field catalogs, selected
evidence, summaries, filters, sorts, Variant Details, and exports.

The current synthetic parity script already combines its eight logical source
contracts. The release-subset implementation extends that pattern to pinned raw
source samples rather than creating another integration model.

## Acceptance rules

A source passes only when:

- every expected allele and source record is present;
- no record is attached to another allele, gene, or transcript;
- every exact field matches its independent expected value;
- missing values remain missing and zero remains zero;
- transcript selection agrees in annotation, queries, details, and export;
- OSA1 and OSA2 are logically equivalent where both apply;
- table, filter, sort, detail, and export representations agree;
- individual-source and combined-source outcomes agree for the same field; and
- every difference is either fixed or named by a narrow reviewed contract
  entry.

There is no acceptable unexplained mismatch percentage. A failed source blocks
the release, but `fail-fast: false` lets the remaining source jobs run so one
workflow reports all failures.

## Artifacts

Every run retains:

- AnnoCAT and fastVEP commits and versions;
- source release and subset manifest;
- reference and transcript identities;
- all relevant SHA-256 hashes;
- cache format and builder identity;
- field-level comparison totals;
- exact mismatch records for redistributable fixtures, or redacted identifiers
  and hashes for restricted fixtures; and
- pass or fail status for each projection stage.

The report also records hashes that connect the raw subset, generated cache,
structured output, result tables, and exported values. These hashes make the
separate stages one reproducible validation chain.

The candidate ZIP is published only after these artifacts exist. GitHub
artifact attestation can record the provenance of the final ZIP; it does not
replace the annotation-validation reports.

## Cost and runner limits

Standard GitHub-hosted runners are free and unlimited for public repositories.
The design uses standard runners only; larger runners are not required.

Each matrix job has its own 14 GB temporary disk. The raw subset, generated
caches, result, and temporary files for one source must fit that runner. Parallel
jobs do not combine their disk use.

Do not persist raw subsets, generated caches, or generated results with
`actions/cache` or `upload-artifact`. Upload only the candidate binaries and
small validation reports. Give intermediate candidate artifacts short
retention so the repository's artifact-storage allowance is not consumed by
obsolete runs.

## Cadence

### Pull requests

Run committed synthetic fixtures and projection tests. These checks are fast,
offline, and available to pull requests from forks.

### Release tags

Run Ensembl consequence concordance and all source subsets before publication.
Build the candidate once, smoke-test that exact ZIP, and publish it without a
second build. Protected release jobs may use source credentials. Untrusted pull
requests must not receive those credentials. Put restricted-source credentials
in a protected GitHub environment and grant the workflow only the permissions
it needs. Pin release-gating actions by commit SHA.

### Scheduled checks

Run the same source matrix periodically to detect unavailable upstream subsets
or changed external contracts. Run the public GIAB benchmark separately for
whole-genome counts, recovery, memory, and performance. GIAB is not the oracle
for third-party source values.

## Not required

The required release gate does not need:

- a self-hosted runner;
- complete gnomAD, CADD, SpliceAI, or dbNSFP caches;
- a new validation database;
- random whole-source sampling on every run;
- patient or private variant data; or
- full source records in logs or public artifacts.

Complete-source validation can remain an optional maintenance check. Add it
only if a future source change cannot be represented by a deterministic subset.

## Implementation map

- `.github/workflows/ci.yml`: current pull-request tests
- `.github/workflows/annotation-concordance.yml`: current manual OSA parity and
  Ensembl consequence checks; proposed reusable release gate
- `.github/workflows/source-release-validation.yml`: proposed parallel source
  matrix
- `.github/workflows/windows-release.yml`: current bundle build; proposed
  candidate build and gated publication
- `fixtures/source-cache-parity/`: current synthetic source contracts
- `fixtures/source-release-subsets/`: proposed pinned source subsets
- `scripts/verify-supplementary-cache-parity.py`: current OSA parity checks
- `scripts/verify-source-release-subset.py`: proposed end-to-end source check
- `config/fastvep-pin.json`: fastVEP identity
- `config/source-catalog.json`: source release contracts

## GitHub references

- [GitHub-hosted runner specifications](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
- [GitHub Actions billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions)
- [Reusable workflows](https://docs.github.com/en/actions/concepts/workflows-and-actions/reusing-workflow-configurations)
- [Artifact attestations](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations)
- [Secure use of GitHub Actions](https://docs.github.com/en/actions/reference/security/secure-use)
