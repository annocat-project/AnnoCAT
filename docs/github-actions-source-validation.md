# Source validation in GitHub Actions

Status: Synthetic contract matrix implemented and passing; pinned release
subsets designed but not implemented

Last updated: 2026-08-25

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
contracts. `source-contract-validation.yml` runs each contract, the separate
gnomAD genomes identity, and the combined contract in parallel. Every matrix
entry builds and verifies OSA1 and OSA2, then passes the verified structured
output through AnnoCAT's production result conversion and query functions from
a test-only harness. The workflow builds that harness once, takes each enabled
source list from the verifier report, and requires exactly one matching
projection test before execution. Normal CI also runs the combined
result-projection test.

This implemented matrix uses synthetic source rows. The release matrix still
needs independently pinned raw subsets for the managed, cache-backed sources
that AnnoCAT can currently install:

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

The validation pack has one manifest containing:

- source name and release;
- upstream location and retrieval date;
- publisher digest, ETag, or immutable parent-release identity;
- raw subset SHA-256;
- selection rule, genomic intervals, and extraction-tool identity;
- expected-record SHA-256;
- field-selection contract SHA-256;
- applicable source license or redistribution restriction; and
- the AnnoCAT fields exercised by the subset.

Every field that AnnoCAT parses specially, renames, selects, calibrates,
summarizes, filters specially, or exports by default must appear in at least one
real-source expectation. Generic optional passthrough fields do not each need a
hand-reviewed real record: the field inventory checks their declared schema,
and the synthetic fixtures check the shared passthrough behavior.

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

## Validation-data delivery

Use one immutable validation pack assembled from the official pinned releases.
Routine GitHub runs must not download complete dbSNP, gnomAD, dbNSFP, CADD, or
SpliceAI releases.

Store each redistributable source-native subset in this repository under
`fixtures/source-release-subsets/`. For data that cannot be redistributed,
fetch only the pinned subset from the official source during a protected
release workflow. Use a private data repository only when indexed or streamed
subset retrieval is not practical and the publisher terms permit private
validation storage. Do not create that repository until a source demonstrates
this need. None of the currently managed variant sources requires one.

Use this layout for committed subsets and for temporary subsets fetched by the
workflow:

```text
fixtures/source-release-subsets/
  manifest.json
  query.vcf
  consequences.ndjson
  projection-expectations.json
  SOURCE/
    raw.vcf, raw.tsv, or another source-native input
    expected.ndjson
```

The raw file must come from the official release, not from fastVEP output, an
OSA cache, or an AnnoCAT result. Preserve the source records and required
headers. Record every deterministic extraction or formatting step in the
manifest. Derive and review `expected.json` independently from those raw
records.

Create the pack once when a pinned source changes:

1. Fetch selected intervals from indexed upstream files with `tabix` or
   `bcftools`, or download and stream the relevant archive once when indexed
   retrieval is unavailable.
2. Retain the smallest records that cover the source contract and known
   regressions.
3. Record the upstream identity, extraction command and tool version, source
   license, selection rule, and all hashes in `manifest.json`.
4. Review the raw records and expected values before publishing the new pack
   commit.

dbNSFP 4.9a does not require the complete archive. Its outer ZIP stores each
already-compressed chromosome file as an uncompressed ZIP member. Use the
pinned offsets, lengths, and CRC values in `config/dbnsfp-4.9a-members.json` to
fetch one or more chromosome members with HTTP Range requests, as AnnoCAT's
`stream_pinned_dbnsfp_member()` already does. Verify `206 Partial Content`,
`Content-Range`, byte count, and CRC before extracting the selected real rows.
The inner chromosome file is gzip rather than BGZF, so the retrieval unit is a
chromosome member, not an arbitrary genomic interval. This still avoids the
complete 39 GB archive and does not require a separate data repository.

All currently managed variant sources have a bounded official acquisition
path:

| Source | Validation input acquisition |
| --- | --- |
| ClinVar | Fetch selected intervals from the archived BGZF VCF, or use the complete pinned file because it is small enough for one runner. |
| dbSNP | Fetch selected intervals from the pinned tabix-indexed BGZF VCF. |
| gnomAD exomes and genomes | Fetch selected intervals from one pinned chromosome BGZF shard and its index. |
| dbNSFP | Fetch selected stored chromosome members from the pinned outer ZIP with HTTP Range requests. |
| CADD | Fetch selected intervals from the pinned SNV and indel tabix-indexed BGZF files. |
| PhyloP | Fetch one or more pinned chromosome gzip shards and retain selected source lines. |
| REVEL | Fetch one or more pinned chromosome ZIP archives and retain selected CSV records. |
| SpliceAI | Fetch selected intervals from the pinned tabix-indexed BGZF VCF. |

The validation workflow should reuse the release identities and range or shard
metadata already present in `config/source-catalog.json`,
`config/indexed-sources.json`, `config/wgs-streams.json`,
`config/dbnsfp-4.9a-members.json`, and `config/revel-1.3-archives.json`. Do not
create a second source catalog for validation.

Keep the expected pack root hash and every remote subset identity in
`fixtures/source-release-subsets/manifest.json`. The workflow verifies the pack
before building any cache. It must fail if a required subset, release identity,
or hash differs.

Direct upstream retrieval during a release run is a fallback for data that
cannot be retained with the source code or in approved private storage. It is
permitted only when the workflow
uses an immutable source identity, verifies the downloaded subset, and the
publisher terms permit automated retrieval. Do not use a rolling `latest` URL
as an oracle.

Any restricted-source credential is available only to protected manual and
release workflows. Do not run the real-source workflow on
`pull_request_target`, and do not expose credentials to fork pull requests.
Synthetic contract tests remain the pull-request gate. Restricted raw records,
cache files, structured output, and result tables must not be uploaded as
artifacts or written to logs; upload only hashes, versions, counts, and redacted
mismatch reports.

## Workflow design

Keep the workflow structure small:

1. `ci.yml` keeps the current Rust and browser tests, including the synthetic
   source-contract result-projection test.
2. `source-contract-validation.yml` runs the synthetic source matrix and the
   combined-source contract on separate `ubuntu-latest` runners.
3. `annotation-concordance.yml` keeps the current synthetic OSA parity check and
   pinned Ensembl consequence oracle, and becomes callable with `workflow_call`.
4. `windows-release.yml` requires the implemented synthetic source-contract
   matrix, then builds one candidate ZIP and exposes it as a workflow artifact
   without publishing it.
5. `source-release-validation.yml` will run pinned raw-subset and
   result-projection checks as a parallel matrix after those subsets exist. It
   obtains each subset from the committed pack or an approved immutable
   upstream fetch. Private storage is an optional per-source fallback.
6. A combined-source job tests source coexistence and both supported gnomAD
   profile choices.
7. A candidate smoke job checks the packaged executable, core annotation,
   result integrity, and export.
8. A final publish job releases that exact candidate only after CI, consequence
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

The pinned Linux fastVEP binary and AnnoCAT projection-test executable are built
once and uploaded as short-lived workflow artifacts. Rebuilding them in every
source job would add time without increasing coverage. The Windows candidate is
built once after these checks; publishing that same candidate after its smoke
check avoids a build-after-validation gap.

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
2. Download the pinned Linux fastVEP binary and AnnoCAT projection-test
   executable built for this workflow run.
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
it needs. Pin release-gating actions and every external source identity by
commit, release, digest, or equivalent immutable identifier.

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

## Required code changes

Keep the real-source lane separate from the existing synthetic matrix. A passing
synthetic run must not be reported as pinned-release validation.

### 1. Define one validation-pack manifest

Add `fixtures/source-release-subsets/manifest.json`. Each source entry contains:

- the source ID and existing catalog or manifest reference;
- the bounded acquisition method and selected chromosome, member, or intervals;
- the source-native output path used by `sa-build`;
- immutable upstream identity and any index identity not already pinned;
- expected byte count and SHA-256 of the extracted raw subset;
- paths and SHA-256 values for the common query VCF, expected structured output,
  and projection expectations; and
- redistribution status.

Do not repeat URLs or release names already present in
`source-catalog.json`, `indexed-sources.json`, `wgs-streams.json`,
`dbnsfp-4.9a-members.json`, or `revel-1.3-archives.json`. The validation manifest
references those contracts and adds only the selection and subset hashes.

### 2. Add the source-native validation pack

Add `fixtures/source-release-subsets/` with:

```text
manifest.json
query.vcf
consequences.ndjson
projection-expectations.json
SOURCE/raw.vcf, raw.tsv, raw.csv, or raw.wigFix
SOURCE/expected.ndjson
```

Commit only records whose terms permit redistribution. For a restricted source,
commit its manifest entry and expectations but materialize `SOURCE/raw.*` under
the runner's temporary directory. The common query is the union of the tested
alleles. Expected values are reviewed from the raw records and are never
generated by fastVEP or AnnoCAT.

Transcript-scoped REVEL and dbNSFP checks also need reviewed consequence records
for the same alleles. Store those records in the pack and merge them into the
supplementary structured output in the test harness. Core consequence
correctness remains the responsibility of `annotation-concordance.yml`; this
merge only supplies the selected-transcript identity needed to test evidence
resolution.

### 3. Add one validation-only retriever

Add `scripts/prepare-source-release-subset.py`. It reads the validation manifest
and existing source manifests, accepts `--source` and `--output`, and supports
only the acquisition methods currently needed:

- remote tabix intervals for dbSNP, gnomAD, CADD, and SpliceAI;
- an HTTP Range ZIP member for dbNSFP;
- a chromosome gzip shard for PhyloP;
- a chromosome ZIP archive for REVEL; and
- the pinned ClinVar file or its indexed intervals.

Use Python's standard library for HTTP Range, gzip, ZIP, CRC, and hashing. Invoke
the runner-provided `tabix` for indexed BGZF queries. The script verifies remote
identity, response ranges, extracted byte count, and final SHA-256 before making
the subset available. It must not derive expected annotations.

### 4. Reuse and extend the current parity verifier

Extend `scripts/verify-supplementary-cache-parity.py`; do not add a second cache
verification implementation. Add a manifest mode that:

1. resolves source file paths from
   `fixtures/source-release-subsets/manifest.json`;
2. verifies every raw and expected-file hash;
3. builds and structurally verifies OSA1 and OSA2 with the pinned fastVEP;
4. annotates the common query VCF;
5. requires OSA1, OSA2, and `expected.ndjson` to be logically equal; and
6. writes the verified OSA2 structured output for AnnoCAT projection tests.

Keep the current synthetic defaults unchanged. Add a field-inventory check
against `supplementary-source-fields.json` and
`dbnsfp-4.9a-curated-fields.json`. Require a real expectation for every field
with AnnoCAT-specific semantics and schema coverage for generic optional
passthrough fields.

### 5. Add a data-driven AnnoCAT projection test

Add `crates/annocat-cli/src/results/source_release_validation.rs` as a
test-only child module of `results.rs`. The test reads paths from environment
variables, runs the existing production conversion functions, and consumes
`projection-expectations.json` instead of hard-coded positions.

For each declared expectation it checks:

- canonical evidence type, scope, allele, gene, and transcript identity;
- table display and missing-versus-zero behavior;
- exact categorical and numeric filters;
- ascending and descending sorts;
- Variant Details evidence; and
- CSV export values.

Mark this test ignored for ordinary `cargo test`; the release-subset workflow
invokes it explicitly with the required files. Do not add a production CLI
command or an incomplete-source mode.

### 6. Add the real-source workflow and release gate

Add `.github/workflows/source-release-validation.yml` with
`workflow_call` and `workflow_dispatch`. It:

1. builds the fastVEP revision from `fastvep-pin.json` and the ignored AnnoCAT
   projection-test executable once, then uploads both as short-lived artifacts;
2. runs the nine source entries and the combined entry as a parallel matrix;
3. prepares and verifies the applicable subset;
4. runs the parity verifier in manifest mode;
5. runs the ignored AnnoCAT projection test; and
6. uploads only hashes, counts, versions, and mismatch summaries.

Use `fail-fast: false`, explicit timeouts, `contents: read`, and no shared
`actions/cache` for source records or generated caches. Keep
`.github/workflows/source-contract-validation.yml` as the fast synthetic
pull-request gate.

Make `.github/workflows/annotation-concordance.yml` callable with
`workflow_call`. Initially run the real-source workflow manually. After its
retriever reproducibility checks and first complete pass succeed, update
`.github/workflows/windows-release.yml` so bundle creation depends on synthetic
contracts, pinned consequence concordance, and the real-source matrix.
Publishing remains blocked until the exact bundle has also passed its packaged
smoke test.

### 7. Required checks before enabling the release gate

- Run the retriever twice and require byte-identical subsets.
- Deliberately change each pinned hash and require fail-closed behavior.
- Run every source entry and the combined entry on GitHub-hosted runners.
- Confirm no restricted row appears in logs or uploaded artifacts.
- Confirm the existing synthetic workflow and normal CI remain unchanged.
- Record the first passing real-source workflow run before describing the
  release as source-concordant.

### Files that do not change

The implementation must not change:

- production source installers in `preparation.rs`;
- source readiness or complete-shard checks;
- fastVEP cache formats or source parsers;
- annotation and result schemas;
- result viewer behavior; or
- local release packaging behavior outside the added release dependencies.

## GitHub references

- [GitHub-hosted runner specifications](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
- [GitHub Actions billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions)
- [Reusable workflows](https://docs.github.com/en/actions/concepts/workflows-and-actions/reusing-workflow-configurations)
- [Using secrets in GitHub Actions](https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/use-secrets)
- [Artifact attestations](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations)
- [Secure use of GitHub Actions](https://docs.github.com/en/actions/reference/security/secure-use)
