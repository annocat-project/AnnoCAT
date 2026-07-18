# AnnoCat implementation TODO

## Product goal

Give a Windows user a complete, reproducible, local annotation application with
fastVEP, dbNSFP, ClinVar, gnomAD, source management, VCF/Parquet/TSV/CSV/HTML
exports, and an interactive results browser. The application must not require
Docker, WSL, Perl, Java, Rust, administrator access, or system-wide installation.

fastVEP is the annotation engine. Stored official Ensembl VEP fixtures are used as
a conformance oracle without installing or shipping a second annotation backend.
AnnoCat remains responsible for downloads, checksums, profiles, orchestration,
validation, provenance, exports, and the Windows user experience.

The phased product and engineering plan for the results case workspace, persistent
variant review, candidate triage, gene/phenotype workflows, and repaired outbound
links is in `docs/curation-triage-implementation-plan.md`.

## Target architecture

```text
Windows UI / CLI
       |
       v
AnnoCat setup, validation, profiles, and run orchestration
       |
       v
Pinned native fastVEP + transcript cache/FASTA + pinned fastSA sources
       |
       v
AnnoCat canonical typed results and immutable provenance
       |
       +--> VCF / Parquet / TSV / CSV / HTML
       +--> Local DuckDB-backed interactive results browser

Pinned reference fixtures (tests only)
```

## 1. Pin, build, and package fastVEP

- [x] Select and record an audited fastVEP commit; do not track a moving branch.
- [ ] Review its Apache-2.0 license, dependencies, security advisories, and source provenance.
- [ ] Build release-mode Windows binaries in a reproducible CI job.
- [x] Run the upstream locked workspace test suite on Windows before packaging.
- [ ] Sign the binary and publish SHA-256 for the small executable artifact.
- [ ] Install it portably at `tools/fastvep/fastvep.exe`; users must not need Rust.
- [x] Detect the managed binary, `ANNOCAT_FASTVEP`, or an existing PATH executable.
- [x] Report fastVEP readiness and version through CLI/JSON.
- [ ] Add explicit install, update, repair, rollback, and removal operations.
- [ ] Record the binary hash, fastVEP version/commit, build target, and installation time.

## 2. Resource profiles and source management

- [ ] Pin GRCh38 FASTA and Ensembl GFF3 releases; optionally add pinned RefSeq GFF3.
- [ ] Build and validate a binary transcript cache instead of parsing GFF3 for every run.
- [ ] Define minimal, practical, comprehensive, somatic, and ACMG-support profiles.
- [ ] Inventory fastSA schemas for ClinVar, gnomAD, dbSNP, dbNSFP, prediction scores,
      gene constraints, ClinGen, regulatory intervals, and licensed optional sources.
- [x] Bound the gnomAD exome cache to AF/AC/AN/homozygote counts and ancestry-specific
      AF fields; discard unrelated VCF INFO fields during the streaming build.
- [x] Preserve the authoritative dbNSFP 4.9a MD5 while recording a local manifest hash.
- [ ] Implement resumable authenticated downloads and authoritative checksum validation.
- [ ] Build `.osa2`/`.osi` files in staging, validate counts/schema/assembly, then promote atomically.
- [ ] Detect suspiciously empty or truncated fastSA builds and fail closed.
- [ ] Show compressed download, installed, staging, and safety-reserve disk requirements first.
- [ ] Install source versions side by side and never mutate resources used by completed runs.
- [ ] Record source URL, release, license, checksum, schema, record count, build command,
      fastVEP version, assembly, and verification result in a manifest.
- [ ] Support verify, repair, update, relocate, remove, and offline import/export bundles.

## 3. Consequence-engine contract and execution

- [ ] Define a Rust `ConsequenceEngine` interface for readiness, identity, annotation,
      progress, cancellation, warnings, and structured failures.
- [ ] Implement `FastVepEngine` with typed arguments and no concatenated shell commands.
- [ ] Use the pinned transcript cache, FASTA, fastSA directory, and explicit annotation profile.
- [ ] Capture stdout, stderr, exit status, timing, warnings, skipped variants, and sanitized command.
- [ ] Run in a restricted child process with bounded resources and no network during annotation.
- [ ] Terminate reliably on cancellation and never promote partial output.
- [ ] Use `--sa-only` for source-only refreshes when consequences are already valid.

## 4. Input identity and normalization

- [ ] Complete VCF syntax, assembly, REF, sort-order, sample, and contig validation.
- [ ] Complete left alignment and normalization against the selected FASTA.
- [ ] Split multiallelic records and correctly remap genotype and allele-indexed fields.
- [ ] Assign stable allele, source-record, and source-ALT identifiers.
- [ ] Preserve original coordinates, ID, QUAL, FILTER, INFO, FORMAT, genotypes, and phasing.
- [ ] Prove stable identifiers survive the complete fastVEP round trip.

## 5. Dynamic result ingestion

- [x] Read the fastVEP `CSQ` definition dynamically from each VCF header.
- [x] Parse all transcript consequences and preserve unknown future fields.
- [ ] Support tabular, VCF, and structured JSON inputs without hard-coded release schemas.
- [ ] Preserve Ensembl/RefSeq source, MANE, canonical, biotype, exon/intron, HGVS,
      protein, regulatory, mitochondrial, and SV consequence fields.
- [ ] Preserve all transcript consequences; display selection must not discard records.
- [ ] Namespace and type every supplementary annotation field.
- [ ] Validate output counts, allele coverage, schemas, warnings, and skipped records.

## 6. Canonical results, exports, and browser

- [ ] Define a versioned canonical schema with normalized alleles, nested consequences,
      source evidence, samples/genotypes, and provenance.
- [x] Write typed Parquet as the canonical analytics output.
- [x] Make the viewer query canonical typed Parquet through the local API; users choose an
      annotation/report, not a Parquet file or DuckDB connection, and browser code never reads
      an entire result file directly.
- [ ] Define a standard ZIP64 AnnoCat report containing `annocat-manifest.json`, checksummed
      typed Parquet, source/run provenance, schema metadata, and optional VCF/TSV/CSV/HTML
      exports. Use an ordinary `.zip` extension so recipients can inspect it with standard
      tools; import it atomically without requiring the databases used to create the report.
- [ ] Give every completed report a filesystem-safe basename
      `<report-name>--<YYYYMMDD-HHMMSS>--<short-run-id>` and use that exact basename for its
      local run directory and shared `.zip`; retain the unsanitized display name in the manifest.
- [ ] Export standards-compliant annotated VCF and streaming TSV/CSV.
- [ ] Generate a portable HTML report tied to the immutable run manifest.
- [ ] Add local DuckDB pagination, sorting, filtering, aggregation, and saved views.
- [ ] Add variant, transcript, clinical, population, prediction, gene, sample, and provenance views.
- [ ] Support a gene-list filter that accepts pasted or uploaded comma/newline-separated
      canonical symbols and Ensembl gene IDs, trims whitespace, matches symbols
      case-insensitively, reports unknown entries, and never interpolates input into SQL.
- [ ] Make gene-list interchange compatible with gene.iobio: accept its headerless
      comma-separated HGNC-symbol form, while also accepting newlines, HGNC IDs, Ensembl gene
      IDs, NCBI Gene IDs, previous symbols, and aliases for AnnoCat input.
- [ ] Resolve every gene token through a pinned/versioned HGNC complete set plus the matching
      transcript cache; retain original input, canonical approved symbol, stable identifiers,
      mapping status, and resolver version. Never guess ambiguous aliases.
- [ ] Export selected rows or the complete filtered result as a deduplicated, stable-order,
      comma-separated gene-symbol `.txt` file in addition to variant-level formats.
- [ ] Add a gene.iobio export preset that writes one headerless line such as
      `BRCA1,TP53,CFTR`, using current approved HGNC symbols, input/selection order, and no
      duplicates; offer a separate mapping report for renamed, ambiguous, and unknown tokens.
- [ ] Provide typed filters for chromosome/range, variant type, gene, transcript,
      consequence/impact, MANE/canonical/biotype, ClinVar significance and review status,
      population frequency, source-specific predictor thresholds, inheritance, sample and
      genotype, QUAL/FILTER, source presence/missingness, warnings, and provenance/version.
  - [x] Wire the canonical-core filters already supported by the bounded Parquet query:
        chromosome/range, REF/ALT, variant ID, gene, transcript, consequence, impact,
        canonical, QUAL range, and VCF FILTER; apply the identical predicate to filtered export.
  - [ ] Add variant type, MANE, biotype, clinical/population/predictor, inheritance/sample,
        source-presence, warning, and provenance filters through the typed dynamic evidence layer.
- [ ] Generate column-header filters from canonical field metadata: numeric score columns
      support `=`, `!=`, `<`, `<=`, `>`, `>=`, inclusive ranges, and missing/present;
      categorical columns support searchable multi-select; text/identifier columns support
      exact, contains, prefix, and pasted-list matching; boolean columns support yes/no/missing.
- [ ] Support stable allele-level selection across sorting, filtering, pagination, and
      transcript expansion, with individual checkboxes, select-page, select-all-filtered,
      clear selection, selected-count feedback, and selection-scoped variant/gene exports.
  - [x] Preserve allele selections across search and unlimited-scroll page loads, support
        individual/loaded-page selection and clearing, show the selected count, and export
        selected genes or selected rows using the current visible-column order.
  - [x] Select all rows matching the active core filters without loading their identifiers into
        the browser, and stream the complete filtered row CSV or deduplicated gene list directly
        from the same parameterized Parquet query to a user-chosen file.
- [ ] Open a variant detail drawer from each row with normalized and original identity,
      transcript consequences, clinical evidence, population frequencies, prediction and
      conservation scores, genotypes/inheritance, warnings, and source/version provenance.
  - [x] Add bounded field-catalog and allele-detail APIs backed by the consequence/evidence
        Parquet files, then wire mouse and keyboard row activation to a responsive drawer.
  - [x] Show core allele identity, all returned transcript consequences, namespaced evidence,
        truncation notices, and URL-encoded ClinVar/GeneBe/gene links without prefetching them.
  - [x] Prioritize the MANE/canonical transcript, collapse other transcripts, group evidence into
        clinical, population, prediction/conservation, and other sections, and show canonical
        VCF/sample/provenance details even for reports without structured sidecars.
  - [ ] Split the drawer into the complete clinical, population, prediction, sample,
        provenance, and warning panes once their typed display metadata is available.
- [x] Ensure the browser never loads a complete WGS result into memory.
- [ ] Escape source content and prevent arbitrary SQL, shell, or HTML execution.

## 7. Validation gate before fastVEP becomes release-default

- [ ] Pin official VEP fixture provenance, flags, and expected output as the oracle.
- [ ] Compare all shared fields, not only the most severe consequence.
- [ ] Test GIAB HG002 plus curated ClinVar pathogenic and benign fixtures.
- [ ] Cover SNVs, MNVs, normalized and repeat indels, multiallelics, splice boundaries,
      LoF/NMD, overlapping transcripts, MANE/canonical selection, and noncoding variants.
- [ ] Cover mitochondrial circular coordinates and codon table 2.
- [ ] Cover DEL/DUP/INV/CNV/BND/INS/STR semantics and breakpoint edge cases.
- [ ] Verify HGVS 3-prime normalization against VEP and VariantValidator fixtures.
- [ ] Verify allele matching and complete schemas for ClinVar, gnomAD, and dbNSFP.
- [ ] Categorize every disagreement as fastVEP defect, VEP difference, transcript/source
      mismatch, normalization difference, or intentionally documented behavior.
- [ ] Define release thresholds and block promotion on unexplained high-impact differences.
- [ ] Upstream reproducible fastVEP defects and pin local workarounds only when necessary.

## 8. ACMG safety boundary

- [ ] Treat fastVEP ACMG output as computational evidence assistance, not a diagnosis.
- [ ] Expose each triggered rule, evidence input, threshold, source version, and uncertainty.
- [ ] Require phenotype, inheritance, disease mechanism, and expert-review-dependent rules
      to remain unset unless their required evidence is explicitly supplied.
- [ ] Validate trio and compound-heterozygous logic with independently reviewed fixtures.
- [ ] Clearly distinguish automated, provisional classification from reviewed interpretation.

## 9. Performance and reproducibility

- [ ] Benchmark 100k, 1M, and complete 4–5M variant inputs on representative Windows PCs.
- [ ] Measure cold/warm consequence annotation, each fastSA source, combined profiles,
      cache build, exports, wall time, CPU, peak memory, I/O, and installed size.
- [ ] Test HDD, SATA SSD, and NVMe behavior and establish minimum memory/disk guidance.
- [ ] Verify identical canonical results across repeated runs and supported Windows versions.
- [ ] Publish hardware, data identities, versions, configuration, and benchmark scripts.

## First deliverable

- [x] Add portable fastVEP readiness and provenance-oriented version detection.
- [ ] Pin and build one Windows fastVEP executable.
- [ ] Install and verify matching GRCh38 FASTA, Ensembl GFF3/cache, ClinVar, and dbNSFP.
- [ ] Run a normalized synthetic VCF through fastVEP with stable AnnoCat identifiers.
- [ ] Parse every CSQ record dynamically into typed Rust structures.
- [ ] Compare the fixture field-by-field with pinned official VEP output.
- [ ] Write verified results to VCF, Parquet, TSV, CSV, and HTML.
- [ ] Open the Parquet result in a local paginated DuckDB-backed variant table.

## Detailed fastVEP integration execution plan

This is the ordered implementation plan. A later phase must not be treated as
complete merely because fastVEP returned exit code zero; every phase includes
identity, schema, count, and output validation.

### A. Register the existing HG002 test VCF

The existing benchmark is currently located at
`target/debug/samples/HG002_GRCh38_1_22_v4.2.1_benchmark.vcf.gz`. AnnoCat's VCF
inspector reports GRCh38, sample `HG002`, 4,048,342 records, 4,096,123 alternate
alleles, 3,463,000 SNP alleles, 632,611 indel alleles, 512 other alleles, and
47,781 multiallelic records.

- [x] Inspect the existing VCF and record its assembly, sample, record, allele,
      SNP, indel, other-allele, and multiallelic counts.
- [ ] Move or re-register the VCF outside `target/`; build cleanup may delete its
      current location. Keep it outside Git because of its size.
- [ ] Locate or download its matching `.tbi`/`.csi` index and verify that the
      index opens and reports the expected contigs.
- [ ] Record the source URL, GIAB release (`HG002 v4.2.1`), retrieval date, file
      size, authoritative checksum when published, and a local SHA-256 identity.
- [ ] Add a test-data manifest containing the path relative to the configured
      resource directory rather than an absolute developer-machine path.
- [ ] Verify that the declared/reference contigs match the chosen GRCh38 FASTA.
- [ ] Preserve the full HG002 VCF as the WGS benchmark and regression corpus.
- [ ] Derive deterministic small fixtures from it only after recording the exact
      region/record selection command and parent-file checksum:
  - [ ] smoke fixture of approximately 100 records for installation checks;
  - [ ] mixed 10,000-allele fixture for routine integration tests;
  - [ ] chromosome 22 fixture for medium performance and VEP comparison;
  - [ ] curated multiallelic, repeat-indel, symbolic, filtered, and genotype cases.
- [ ] Ensure derived fixtures retain headers, sample FORMAT fields, phasing, QUAL,
      FILTER, INFO, IDs, and original record identifiers.

### B. Produce the pinned portable Windows binary

- [x] Pin fastVEP `0.2.0` source commit
      `7038e7c17708e7d2226149e78e0bb297bcc6d1d6` and its Cargo.lock identity.
- [x] Compile and run `cargo test --workspace --locked` successfully on Windows.
- [ ] Run dependency license and known-vulnerability audits and retain reports.
- [ ] Review all build scripts, native dependencies, network behavior, unsafe code,
      file writes, subprocesses, and environment-variable inputs used by the CLI.
- [x] Build `fastvep-cli` using `--release --locked` for 64-bit Windows.
- [ ] Confirm the resulting executable runs on a clean Windows user account without
      Rust, Git, Python, Perl, Java, Docker, WSL, or administrator privileges.
- [x] Run cache building and a bundled tiny annotation; preserve its pinned fixture,
      expected VCF, checksums, source provenance, version, and command.
- [x] Store the executable at `tools/fastvep/fastvep.exe` in the portable layout.
- [x] Record its byte size and SHA-256 in a versioned installation manifest.
- [x] Verify the executable checksum before every annotation launch.
- [ ] Fail closed on an unknown version/checksum unless the user explicitly selects
      an externally managed executable and acknowledges the reproducibility warning.
- [ ] Add atomic install, repair, upgrade, rollback, and removal operations.
- [x] Bundle fastVEP with AnnoCat in one Windows ZIP and preserve its upstream
      Apache-2.0 license, pinned source identity, artifact manifest, and checksums.
- [ ] Generate and review the complete third-party dependency notice/license report.

### C. Implement the typed fastVEP engine boundary

- [ ] Define `ConsequenceEngine` and `FastVepEngine` types outside CLI presentation code.
- [ ] Define typed request fields for input/output paths, assembly, transcript cache,
      FASTA, fastSA directory, output format, threads, profile, HGVS, regulatory,
      merged gene models, source-only mode, and filters.
- [ ] Resolve every path to an absolute path and reject ambiguous or mismatched inputs.
- [ ] Construct `Command` arguments as separate values; never build a shell command string.
- [ ] Set stdin to null and capture stdout/stderr without risking pipe deadlock.
- [ ] Parse progress into structured events with stage, completed count, total when
      known, throughput, elapsed time, and message.
- [ ] Stream logs to the run directory and expose sanitized diagnostics to the UI.
- [ ] Implement cancellation that terminates fastVEP and waits for process cleanup.
- [ ] Run annotation fastVEP in a zero-network AppContainer plus a kill-on-close Job Object:
      stream the user VCF through inherited stdin, grant automatic read-only access only to
      AnnoCat-managed reference/transcript/fastSA resources, and constrain output to run staging.
- [ ] Run fastVEP cache/source builders in a separate zero-network preparation AppContainer;
      keep HTTP in AnnoCat's bounded bridge and grant write access only to app-managed staging.
- [ ] Run in a unique staging directory and atomically promote only validated results.
- [ ] Delete or quarantine partial outputs while preserving enough diagnostics to debug.
- [ ] Add timeout, nonzero-exit, missing-resource, corrupt-cache, disk-full, locked-file,
      malformed-output, and cancellation tests.
- [ ] Add `annocat fastvep run` initially, then connect the same engine to browser jobs.

### D. Pin and prepare the transcript/reference resources

- [ ] Choose one Ensembl release compatible with the pinned fastVEP revision.
- [x] Pin the Ensembl 115 GRCh38 GFF3 URL, exact remote size, and SHA-256 after
      a successful ranged download through AnnoCat's own downloader.
- [ ] Pin or formally approve the matching GRCh38 FASTA identity, license, and retrieval metadata.
- [ ] Decide whether the existing NCBI no-alt FASTA is sequence-identical for every
      required contig; do not mix it with Ensembl gene models until proven compatible.
- [ ] Download through AnnoCat's resumable downloader and verify before extraction.
- [ ] Build and verify the FASTA `.fai` without requiring an external `samtools` install.
- [ ] Build fastVEP's binary transcript cache in staging.
- [ ] Record transcript, gene, exon, coding, MANE, canonical, contig, and skipped-record counts.
- [ ] Reject unresolved primary contigs or unexpectedly empty sequence/transcript caches.
- [ ] Optionally add pinned RefSeq GFF3 plus a checked chromosome-synonym table.
- [ ] Test Ensembl-only before enabling merged Ensembl/RefSeq annotation.
- [ ] Version resources side by side and bind each run to immutable resource identities.

### E. Complete the first annotation round trip

- [x] Run the initial eight-record upstream smoke fixture without supplementary databases.
- [ ] Add a FASTA-backed fixture, then request HGVS; current no-FASTA smoke output
      verifies CSQ, canonical/MANE, source, consequence, and cache behavior.
- [ ] Verify input record/allele identities survive output, including multiallelics.
- [ ] Verify sample columns and GT/DP/GQ/AD values are preserved exactly where applicable.
- [ ] Verify output is complete VCF, has one unambiguous CSQ schema header, and contains
      no duplicate AnnoCat-owned INFO definitions.
- [x] Dynamically parse and validate the CSQ header field order without assuming
      fastVEP's current order; support plain and gzip-compressed VCF output.
- [ ] Parse multiple transcripts and multiple SO terms without discarding information.
- [ ] Preserve unknown CSQ fields in a namespaced extension map.
- [ ] Validate required fields, data types, escaping, missing values, and allele linkage.
- [ ] Compare output against pinned expected fastVEP output and stored official VEP fixtures.
- [ ] Categorize and document every consequence/HGVS/transcript disagreement.
- [ ] Promote this smoke round trip to an automated integration test.

### F. Replace the temporary dbNSFP implementation with fastSA

Implementation order: complete the shared streaming-to-OSA foundation before expanding
the dbNSFP schema. Do not add fields to the current whole-file `Vec` parser and then
rewrite it; the schema-preserving importer must be incremental from its first release.

- [ ] Complete these prerequisites in order:
  - [x] add stdin/generic-`Read` support to fastVEP `sa-build`; fork commit `6178fee` accepts
        non-seekable plain/gzip/BGZF stdin, preserves sniffed bytes, and passes the
        upstream streaming test suite;
  - [x] replace dbNSFP's whole-file `Vec` parser and final sort with an ordered,
        bounded-memory iterator feeding `SaWriter` directly; fork commit `50f13ff`
        preserves current fields, rejects unsorted input, removes partial output on
        failure, and produces a loadable allele-specific OSA fixture;
  - [x] add manifest-backed chromosome-sharded OSA loading as one logical provider;
        fork commit `ea33e54` validates schema/source metadata and relative paths,
        rejects duplicate chromosome aliases, fails closed when a required manifest
        is invalid, and dispatches each lookup/preload to exactly one shard;
  - [ ] prove atomic promotion, cancellation, completed-chromosome restart, and bounded
        memory/source-disk usage with fixtures;
  - [x] add versioned schema-preserving dbNSFP field selection; fork commit `231c192`
        consumes the 4.9a curated contract, preserves transcript-aligned values, and
        rejects headers missing any configured field;
    - [x] expose the curated groups in individual/profile install dialogs, keep linkage
          fields mandatory, persist the chosen subset before queuing work, and bind its
          fingerprint to every chromosome checkpoint through fork commit `36e5f29`;
    - [x] verify every cache block and record boundary after writing while parsing JSON
          and testing deterministic lookups at representative block edges, avoiding a
          redundant full JSON reparse of large dbNSFP shards;
  - [ ] complete OSA plus JSON/VCF/tabular round-trip validation for the curated schema.

- [ ] Retain the authoritative dbNSFP 4.9a MD5 already stored in the project.
- [ ] Verify the existing 4.9a archive before reading or converting it.
- [x] Determine which dbNSFP input file and columns fastVEP's `sa-build --source dbnsfp`
      accepts and map them to AnnoCat's required schema.
- [x] Confirm the pinned fastVEP dbNSFP parser is intentionally limited primarily to
      SIFT/PolyPhen review fields and does not replace AnnoCat's promised full schema.
- [x] Extend fastSA conversion (preferably upstream) or add a generic schema-preserving
      fastSA adapter before declaring dbNSFP migration equivalent.
- [x] Treat loss of any configured dbNSFP column as a release-blocking validation failure.
- [x] Define the complete desired dbNSFP field list before conversion; the versioned
      `config/dbnsfp-4.9a-curated-fields.json` contract retains transcript linkage plus
      high-impact coding predictors/conservation and explicitly excludes dedicated-source
      duplicates. Fork commit `231c192` consumes this contract, and the preparation identity forces
      legacy two-field preview shards to be rebuilt rather than reported as ready.
- [ ] Measure conversion staging space, peak memory, wall time, and final fastSA size.
- [ ] Build fastSA in staging and verify assembly, record counts, chromosomes, allele
      matching, schema, random lookups, first/last records, and expected file magnitude.
- [ ] Compare fastSA annotations with the current streaming dbNSFP implementation on a
      deterministic mixed fixture before deleting the temporary implementation.
- [ ] Replace `query-dbnsfp`, `annotate-dbnsfp`, DuckDB preparation, and their browser
      endpoints with generic fastSA source build/verify/status operations.
- [ ] Remove the obsolete dbNSFP-specific Rust annotation code and dependencies only
      after equivalence and restart/cancellation tests pass.

### G. Add supplementary sources incrementally

- [x] Add versioned retained-field contracts for every actionable supplementary
      fastVEP source, preserve each source's existing fields as its default selection,
      expose the editors on source cards and profile review, reject unsupported parser
      fields, and fingerprint custom schemas so incompatible shards cannot be resumed.
- [ ] ClinVar first:
  - [ ] pin GRCh38 VCF and checksum;
  - [ ] build fastSA and validate variation IDs/accessions, clinical significance,
        review status/stars, conditions, submitter conflicts, dates, and allele matching;
  - [ ] retain conflicting assertions rather than flattening to one label.
- [ ] gnomAD second:
  - [ ] select genomes/exomes and exact release;
  - [ ] show its large download, staging, and installed-size estimates before consent;
  - [ ] validate global and population AC/AN/AF, homozygotes, filters, sex chromosomes,
        multiallelics, and normalized repeat indels.
- [ ] Add a dedicated **WGS profile** in which dbNSFP remains the coding/missense
      aggregation source while genome-wide annotations use dedicated resources:
  - [x] define stable `standard` and `wgs` profile/source membership in the Rust core
        and expose it through `/api/profiles`;
  - [x] full CADD v1.7 SNV/indel scores, retaining raw and PHRED values, with tabix-guided
        chromosome range streaming and no retained source tables;
  - [x] a pinned genome-wide hg38 PhyloP 100-way track streamed as chromosome wigFix shards;
  - [x] dedicated gnomAD genomes/sites v4.1 chromosome data with AF, AC, AN, homozygotes, populations,
        filters, and explicit absence-versus-missing semantics;
  - [x] public Ensembl MANE Select v1.4 masked SpliceAI GRCh38 SNVs with gene,
        DS_AG/AL/DG/DL, and DP_AG/AL/DG/DL preserved per allele, using tabix-guided
        chromosome range streaming and no retained source VCF;
  - [ ] optionally support the separate account-gated Illumina GRCh38 indel scores
        without making them a WGS profile requirement.
- [ ] Keep the smaller standard profile based on transcript resources, dbNSFP, ClinVar,
      and gnomAD; never describe dbNSFP-only coverage as complete for WGS.
- [ ] Allow WGS resources to be selected independently and show download, staging,
      installed, and temporary disk estimates before starting.
- [ ] Namespace overlapping dbNSFP and dedicated values by source/release, define the
      dedicated WGS value as preferred, and never silently overwrite or merge them.
- [ ] Verify dedicated WGS coverage on coding, splice-region, deep-intronic, regulatory,
      and intergenic variants; report inapplicable, absent, and failed lookup distinctly.
- [ ] Benchmark the WGS profile on smoke, 10k, chromosome 22, and full HG002 inputs,
      recording per-source time, memory, I/O, index size, and annotation yield.
- [ ] Evaluate the following exact URLs recovered from Vera's active configuration as
      candidate AnnoCat catalog entries; do not promote them until availability, license,
      assembly, publisher checksum, byte size, schema, index requirements, and redistribution
      terms have been independently verified:
  - [x] represent unverified single-file and chromosome-template resources through a
        non-downloadable Rust candidate catalog and `/api/resources/catalog-candidates`;
  - [ ] ClinVar GRCh38 VCF:
        `https://ftp.ncbi.nlm.nih.gov/pub/clinvar/vcf_bgzip/grch38/clinvar.vcf.gz`
        and index `https://ftp.ncbi.nlm.nih.gov/pub/clinvar/vcf_bgzip/grch38/clinvar.vcf.gz.tbi`;
  - [ ] ClinVar supporting evidence:
        `https://ftp.ncbi.nlm.nih.gov/pub/clinvar/tab_delimited/variant_summary.txt.gz`,
        optional `https://ftp.ncbi.nlm.nih.gov/pub/clinvar/tab_delimited/submission_summary.txt.gz`,
        optional `https://ftp.ncbi.nlm.nih.gov/pub/clinvar/xml/mini_clinvar_hgvs.xml.gz`, and
        `https://ftp.ncbi.nlm.nih.gov/pub/clinvar/xml/ClinVarRCVRelease_00-latest.xml.gz`;
  - [ ] HGNC complete set:
        `https://ftp.ebi.ac.uk/pub/databases/genenames/new/tsv/hgnc_complete_set.txt`;
  - [ ] MANE GRCh38 v1.5 GFF:
        `https://ftp.ncbi.nlm.nih.gov/refseq/H_sapiens/mRNA_Prot/mane/MANE.GRCh38.v1.5.refseq_genomic.gff.gz`;
  - [x] default gnomAD exomes v4.1.1 per chromosome (185.56 GiB verified total):
        `https://gnomad-public-us-east-1.s3.amazonaws.com/release/4.1.1/vcf/exomes/gnomad.exomes.v4.1.1.sites.chr{chrom}.vcf.bgz`;
  - [x] expose gnomAD genomes v4.1.1 (526.80 GiB verified network total) as an explicit
        mutually-exclusive optional stream, not a Standard or WGS profile default;
  - [ ] gnomAD v4.1 gene constraint:
        `https://gnomad-public-us-east-1.s3.amazonaws.com/release/4.1/constraint/gnomad.v4.1.constraint_metrics.tsv`;
  - [x] reject Vera's configured SpliceAI URL
        `https://zenodo.org/record/3363083/files/spliceai_scores.v1.3.{chrom}.masked.vcf.gz`:
        Zenodo concept record `3363083` resolves to unrelated nematode imagery, not
        SpliceAI, so the URL has been removed from AnnoCat's candidate catalog;
  - [x] use Ensembl's public MANE Select v1.4 masked GRCh38 SpliceAI SNV VCF and
        tabix index, pinned by publisher metadata and index checksum;
  - [x] REVEL v1.3 per-chromosome segments (24 publisher archives totaling
        667,188,638 bytes, pinned with publisher MD5s and verified with the real
        chromosome Y archive):
        `https://zenodo.org/records/7072866/files/revel-v1.3_segments_chrom_{chrom_padded}.zip`;
  - [x] CADD v1.7 GRCh38 whole-genome SNVs:
        `https://krishna.gs.washington.edu/download/CADD/v1.7/GRCh38/whole_genome_SNVs.tsv.gz`;
    - [x] verify publisher filenames and MD5s for SNV scores/index and gnomAD v4.0
          indel scores/index, and record all four artifacts in the candidate catalog;
    - [x] verify exact HTTP sizes and byte-range support without downloading the payloads:
          SNVs 87,473,403,655 bytes, SNV index 2,761,840 bytes, indels
          1,257,151,321 bytes, and indel index 1,899,705 bytes;
  - [ ] dbNSFP 4.9a archive:
        `https://usf.box.com/shared/static/0tq7q3b8ucaxxkmfyvnb0ss7g58ptgcl`;
  - [ ] optional UCSC segmental duplications and RepeatMasker:
        `https://hgdownload.soe.ucsc.edu/goldenPath/hg38/bigZips/hg38.genomicSuperDups.bed.gz`
        and `https://hgdownload.soe.ucsc.edu/goldenPath/hg38/bigZips/hg38.repeatMasker.bed.gz`;
  - [ ] optional ClinGen gene curation:
        `https://ftp.clinicalgenome.org/free/tier_clinical_evidence_db.tab`.
- [x] Add the official UCSC GRCh38 phyloP100way BigWig candidate catalog entry and its
      publisher MD5 separately; Vera contains a `PhyloPSource` implementation but its
      active configuration does not provide a URL.
  - [x] verify its exact HTTP size (9,870,053,206 bytes) and byte-range support;
- [ ] Define and verify explicit chromosome expansion tables per resource rather than
      copying Vera's generic substitution: its active gnomAD template receives bare
      `1..22/X/Y` values although publisher filenames may require `chr`, while its REVEL
      implementation integer-pads autosomes and cannot use that path for X/Y filenames.
  - [x] correct the gnomAD v4.1 templates to `sites.chr{chrom}.vcf.bgz` plus `.tbi`;
        verify chr1, chrX, chrY objects and byte-range support, and confirm Vera's bare
        `sites.1.vcf.bgz` expansion returns 404;
- [x] verify the official gnomAD bucket contains exactly 24 chromosome VCFs and 24
        indexes totaling 563,052,329,190 bytes, and expose that aggregate download size;
- [ ] Implement **chromosome-at-a-time resumable streaming preparation** for large WGS
      resources. The default may retain only the incomplete current chromosome/range
      part while feeding those same bytes to fastVEP; verified source parts are deleted.
      Keep pure streaming as an optional lowest-disk mode:
  - [x] confirm fastVEP's `SaWriter` and `run_streaming_sa_build` already parse and write
        bounded blocks incrementally; retain fastSA as the installed representation rather
        than introducing a second Parquet/database copy of annotation resources;
  - [x] add a fastVEP library/CLI input boundary that accepts stdin or a generic `Read`
        stream for `sa-build`, while retaining transparent gzip/BGZF decoding and byte-count
        progress; do not pass remote URLs to a shell; implemented as fork commit `6178fee`;
  - [x] have AnnoCat open each pinned chromosome URL and stream
        `HTTP response -> compressed-byte counter -> decoder -> fastVEP parser -> fastSA writer`;
    - [x] implement the bounded process/HTTP bridge: validate success, Content-Length,
          pinned ETag/Last-Modified, pipe the response directly to `fastvep sa-build -i -`,
          log stderr to disk without pipe deadlock, enforce cancellation/byte totals, and
          remove only incomplete OSA outputs on failure;
    - [x] connect the bridge to catalog/profile jobs and expose live progress/cancellation APIs;
      - [x] add a fail-closed catalog job boundary for pinned single-object fastSA sources,
            including direct ClinVar streaming, structured byte/throughput progress, and
            resource/profile status, start, and cancellation routes;
      - [x] add pinned per-chromosome identities for gnomAD/PhyloP and tabix-derived CADD
            chromosome identities, then sequence all unattended profile members;
  - [x] bind each preparation job to source ID/release, assembly, chromosome, URL, expected
        compressed byte count, ETag/Last-Modified when trustworthy, schema version, selected
        fields, fastVEP commit, and output-format version; implemented in the versioned
        `PreparationIdentity` checkpoint contract;
  - [x] write each chromosome to isolated `.partial` fastSA/index files and atomically
        promote it only after HTTP length/EOF, decompressor CRC/EOF, parse counts, writer
        close, index reopen, chromosome bounds, and deterministic random lookups pass;
    - [x] implement the local lifecycle foundation: isolated partial directory, non-empty
          OSA/index and source-byte/count gates, verified marker, and atomic directory rename;
    - [x] connect decompressor EOF/CRC, index reopen, bounds, JSON, and deterministic
          lookup validators through pinned fastVEP commit `0d8087e`; AnnoCat promotes only
          after the machine-readable verifier reports nonzero blocks and records;
  - [x] retain already verified chromosome shards across cancellation, network failure,
        application restart, or reboot; delete/quarantine only the incomplete current shard;
  - [x] add the optional hybrid source-part lifecycle, exact Range continuation, identity
        validation, replay of the retained prefix into a fresh fastVEP process, and automatic
        deletion after verified promotion; keep direct pure streaming as the default;
  - [x] apply the lifecycle to generic chromosome objects, dbNSFP ZIP members, REVEL ZIPs,
        indexed SpliceAI/CADD ranges, and indexed dbSNP chromosome ranges;
        the preparation-state tests prove restarting chr2 preserves promoted chr1;
  - [x] restart a failed compressed chromosome stream from byte zero. Do not claim arbitrary
        byte-range resume is safe unless BGZF virtual offsets, parser state, and appendable
        fastSA checkpoints are implemented and independently validated; matching partial
        checkpoints select `RestartCurrentChromosome`, while stale identities fail closed;
  - [x] add a versioned shard manifest so fastVEP loads all completed chromosome shards as
        one logical source without duplicate fields or ambiguous precedence; prefer a native
        sharded provider over concatenating or rewriting verified shards; implemented for
        OSA v1 as strict `*.osa-shards.json` manifests with shard files in a subdirectory;
  - [x] make profile progress use the catalog's aggregate compressed byte totals and expose
        current chromosome, network bytes, parsed records, prepared bytes, throughput, and
        completed/remaining chromosomes separately;
  - [x] estimate required disk from prepared-shard growth plus one bounded writer buffer,
        metadata, and safety reserve—not from compressed source size plus a full staging copy;
  - [x] implement bounded channels/backpressure so a slow disk or parser cannot cause the
        downloader to buffer an entire chromosome in RAM; the HTTP reader and fastVEP stdin
        use one synchronous fixed 1 MiB buffer, so a blocked pipe stops further network reads;
  - [x] test truncated HTTP bodies, incorrect Content-Length, changed ETag, corrupt gzip/BGZF,
        malformed and out-of-order records, schema drift, disk-full, locked output, cancellation,
        restart, and a server that ignores Range requests;
  - [x] prove with instrumentation that peak temporary source-disk usage remains bounded and
        that no source chromosome VCF appears in the resource or temporary directories; the
        direct-copy test observes a single 1 MiB read buffer and the disk plan fixes staged
        source usage at zero;
  - [x] record compressed input, prepared OSA, and prepared index bytes per verified shard so
        installed-size ratios are measured rather than inferred from another release/schema;
  - [ ] validate the design first on a tiny local HTTP fixture (completed with the real pinned
        ClinVar parser and atomic promotion), then one small chromosome,
        chromosome 22, and finally the complete 24-chromosome gnomAD resource.
- [ ] Add dbSNP identifiers and validate merged rsIDs and multiallelic records.
- [x] Add REVEL v1.3 as an optional, separately versioned managed source with direct
      chromosome-ZIP streaming, transcript IDs, restartable shards, and publisher
      MD5/ZIP CRC verification. Keep it out of both one-click profiles because dbNSFP
      already carries REVEL fields; users can select standalone REVEL for either
      profile when the complete release is preferred.
- [ ] Keep GERP, PrimateAI, DANN, and other dedicated predictors as optional,
      separately versioned sources when dbNSFP coverage/version is insufficient.
- [ ] Add gnomAD gene constraint and ClinGen gene-disease/dosage evidence.
- [ ] Treat OMIM and COSMIC as licensed user-supplied sources; never redistribute them.
- [ ] Add regulatory GFF3/interval resources and validate promoter/enhancer/CTCF/TFBS output.
- [ ] Allow custom VCF/BED sources with schema preview, assembly checks, and safe namespaces.

### H. Canonical storage and provenance

- [x] Define a versioned canonical Rust schema for variants, alleles, transcripts,
      consequences, samples, supplementary evidence, genes, warnings, and provenance.
- [x] Preserve every transcript in a normalized consequence Parquet table, typed namespaced
      source fields in an evidence Parquet table, and the complete structured consequence JSON
      rather than destructively flattening to one selected transcript or dropping unknown fields.
- [ ] Record input/test-data checksum, fastVEP binary checksum, commit/version, FASTA,
      transcript cache, every fastSA resource, profile, arguments, timings, and outputs.
- [ ] Include source licenses and restricted-data flags in the run manifest.
- [x] Write manifest and outputs to staging, validate them together, then promote atomically.
- [ ] Bind resumable checkpoints to all relevant identities and reject stale checkpoints.
- [ ] Add schema migration/version rejection behavior for future AnnoCat releases.

### I. Exports and interactive browser

- [x] Produce annotated VCF while preserving headers and original sample data.
- [ ] Produce canonical typed Parquet plus documented TSV/CSV flattening views.
- [ ] Generate portable HTML tied to the immutable run manifest.
- [x] Query Parquet through DuckDB with typed parameters, pagination, and bounded memory.
- [x] Atomically publish a completed run manifest only after result validation; discover those
      manifests automatically on Browse Results, sort newest first, and open each card into
      its annotation table without treating resource jobs or incomplete directories as runs.
- [ ] Add `Rename` to each Browse Results card and the open report menu. Store the new name as
      local library metadata keyed by immutable run ID; validate length/characters, prevent
      confusing case-insensitive duplicate display names, and update cards/search immediately.
- [ ] Keep the original run manifest, provenance, result directory, run ID, and externally
      imported ZIP immutable when renaming. Show the original report name in Provenance and
      provide `Reset to original name`; never attempt to rename a ZIP in Downloads or on USB.
- [ ] When re-sharing a renamed report, default the new ZIP basename to
      `<current-display-name>--<original-completion-time>--<short-run-id>.zip` while preserving
      the original report name and complete rename history in package provenance.
- [ ] Add a `Share report` flow from completed results: choose a destination ZIP and whether to
      include annotated VCF, TSV, CSV, and portable HTML in addition to the required manifest
      and Parquet; show resulting size and an explicit sample/genotype privacy summary.
  - [x] Create the canonical Parquet-only report through a Results-page Save dialog, use a
        report-name/time/run-ID filename, write atomically with ZIP64-capable stored entries,
        validate the finished ZIP through the importer, and show its actual size.
  - [ ] Add optional VCF/TSV/CSV/HTML choices and sample/genotype privacy confirmation.
- [ ] Let recipients open or drag in one AnnoCat report `.zip`, validate its manifest/schema and
      every checksum before exposure, reject archive traversal/symlinks/decompression bombs,
      import to staging, and atomically add it to Browse Results. Viewing must work without
      local reference, transcript, or annotation-source installations.
  - [x] Validate ZIP structure, manifest coverage, declared sizes, and SHA-256 in a dedicated
        zero-capability Windows AppContainer worker passed only an inherited read-only archive
        handle; package and checksum the worker without requiring user ACL changes.
  - [ ] Validate canonical Parquet schemas in a bounded DuckDB worker before extraction and
        publication; disable external access and extension loading and constrain its readable
        files, memory, threads, returned rows, and execution time.
- [ ] Import a validated report to `<ANNOCAT_HOME>/runs/<report-basename>/` (the `runs` path
      reported by `/api/paths`). Never move or delete the user-selected ZIP. If the same run ID
      and checksums already exist, open the existing import; if the ID matches but content does
      not, reject it as a conflict instead of overwriting or inventing a misleading duplicate.
  - [x] Extract declared canonical files to staging, recheck sizes/hashes and required Parquet
        schemas/allele references, atomically publish into runs, preserve the source ZIP, open
        the result automatically, make repeat imports idempotent, and reject run-ID conflicts.
- [ ] Before extraction, show required/free disk space and explain that the source ZIP remains
      where the user opened it and may be deleted manually after a successful import.
- [ ] Keep report sharing and format export distinct: the report ZIP always contains the
      canonical data required by AnnoCat, while VCF/TSV/CSV/HTML are optional conveniences so
      WGS reports do not contain several large duplicate representations by default.
  - [x] Record exact per-table, canonical-total, and annotated-VCF byte counts in every completed
        run manifest and expose canonical result size on Browse Results.
  - [x] Keep annotated VCF off by default in CLI and New Annotation, use it only as a temporary
        validated conversion input, and retain it only when the user explicitly requests it.
  - [ ] Benchmark stored ZIP64 overhead plus selected, filtered, and full flattened TSV/CSV sizes
        at 100k, 1M, and complete HG002 scale before setting export warnings or disk guidance.
- [ ] Add variant, transcript, clinical, population, prediction, gene, sample, and
      provenance panes with configurable columns and saved filters.
- [ ] Base the table interaction on fastVEP Web, then add server-side multi-column sorting,
      typed filter groups, saved column/filter presets, row selection, select-page and
      select-all-filtered behavior, and exports scoped to selected, filtered, or all rows.
- [ ] Put a type-aware filter control in each filterable column header and keep active filters
      visible as removable chips. Derive controls, validation, source/version labels, and
      numeric score bounds from the result schema so new source fields work without bespoke UI.
  - [x] Group core and dynamic evidence columns by source in the column selector and fetch only
        the selected evidence fields for visible result pages, with a 32-column display bound.
- [ ] Allow multiple column filters to combine with explicit AND/OR groups, show the filtered
      row count before export, preserve filters while changing visible columns, and apply the
      identical typed predicate to table pagination, summaries, selections, and exports.
- [ ] Make the selection checkbox identify the canonical allele rather than a displayed
      transcript row; retain selection while paging or opening details and require an explicit
      choice between selected, filtered, and all variants before every bulk export.
- [ ] Add an accessible click/keyboard-opened side drawer for variant details with collapsible
      Identity, Consequences, Clinical, Population, Predictions, Samples, Provenance, and
      Warnings sections; preserve the user's table position and selected rows when it closes.
  - [x] Implement the bounded detail request, accessible row activation, close/reopen behavior,
        responsive side panel, transcript expansion, evidence list, and safe outbound links.
- [ ] Add a report-level `Case notes` pane to the results viewer with a large plain-text/
      Markdown editor, autosave, saved/unsaved/error status, keyboard undo/redo, timestamps,
      and explicit Save now/Revert controls. Keep annotation-table navigation responsive.
  - [x] Add open-report Rename and a local plain-text case-notes editor with autosave,
        Save now/Revert, size bounds, and visible save/error status; keep both outside the
        immutable run manifest and imported ZIP.
- [ ] Store case notes as atomic local library metadata keyed by immutable run ID, separate
      from checksummed annotation results and the original imported ZIP. Preserve a bounded
      local revision history so an accidental edit or failed write can be recovered.
- [ ] Let users insert stable links to selected alleles/genes into notes and reopen those links
      in the table or variant drawer without copying mutable row numbers or filter positions.
- [ ] Treat case notes as potentially identifying clinical data: display a clear local-storage
      notice, render Markdown with HTML disabled and links sanitized, never send note content
      to external links/services, and include no remote spellcheck, analytics, or editor assets.
- [ ] Exclude case notes from shared report ZIPs by default. Add an explicit `Include case
      notes` option with a privacy confirmation; when selected, checksum `case-notes.md`, mark
      its presence in the package manifest, and clearly notify the recipient on import.
- [ ] Generate outbound links only from validated, URL-encoded canonical identifiers: ClinVar
      Variation ID/VCV, GeneBe GRCh38 normalized allele, dbSNP rsID, gnomAD GRCh38 allele,
      Ensembl/UCSC locus, and relevant PubMed IDs; gene links include HGNC/NCBI Gene,
      GeneCards, Wikipedia, and OMIM only when a corresponding identifier is available.
- [ ] Never prefetch outbound interpretation links or send samples, phenotypes, genotypes, or
      a result list to a third party. Open links only after a user click with `noopener` and
      `noreferrer`, label the destination, assembly, and identifier, and disable links whose
      required identity cannot be constructed safely.
- [ ] Keep semantically duplicated scores in source/version-labelled columns; an optional
      preferred-value column must expose its selected source and must not discard raw values.
- [ ] Add a comma/newline-separated gene-list filter with paste, file import, normalized
      preview, unknown/ambiguous-entry feedback, clear/reset, and exact canonical matching.
- [ ] Include a gene.iobio-compatible mode in that filter: parse a headerless comma-separated
      HGNC-symbol list, normalize unambiguous historical aliases to approved symbols, display
      every conversion before applying it, and allow the normalized list to be copied back.
- [ ] Export genes from selected rows or all filtered rows as one comma-separated `.txt`
      list, deduplicated across variants/transcripts with deterministic ordering and an
      explicit empty-selection error rather than silently exporting every result.
- [ ] Provide `Gene list for gene.iobio (.txt)` as an explicit export choice and test round-trip
      import/export using current symbols, case differences, whitespace, duplicates, previous
      symbols, aliases, Ensembl/HGNC/NCBI IDs, unknown genes, and ambiguous mappings.
- [ ] Add optional phenotype-driven gene ranking in the Results viewer: accept selected HPO
      terms and plain-language phenotype text, show term normalization/ambiguity before use,
      rank genes already present in the result, and expose rank/score as sortable and filterable
      columns without automatically hiding variants. Allow the ranked genes to feed the existing
      gene-list filter and gene.iobio-compatible export.
- [ ] Keep phenotype ranking reproducible and privacy-preserving: record normalized terms,
      ontology/model/data versions and scores in local case metadata, support an offline data
      package, never send phenotypes to a remote service without explicit consent, and label the
      feature as research prioritization rather than a diagnosis.
- [ ] Cover location and variant type; gene/transcript/consequence/impact/MANE/biotype;
      ClinVar; population AF/AC/AN; predictor and conservation thresholds by source;
      inheritance/sample/genotype/quality; evidence presence; warnings; and provenance.
- [ ] Display all transcript consequences while allowing a separate display transcript.
- [ ] Escape source content and prevent arbitrary SQL, HTML, file, or shell execution.
- [ ] Verify all exports/browser views agree on stable allele identity and core values.

### J. Scale from fixtures to the complete existing HG002 VCF

- [ ] Pass the smoke fixture, then 10k fixture, then chromosome 22 before full WGS.
- [ ] Run one cold-cache and at least two warm-cache full HG002 annotations.
- [ ] Record wall time, CPU utilization, peak working set, disk throughput, temporary
      space, output sizes, and per-source overhead.
- [ ] Confirm all 4,048,342 records and 4,096,123 alternate alleles are accounted for,
      with explicit handling/reporting of the 512 other alleles.
- [ ] Confirm the 47,781 multiallelic records retain correct ALT/genotype linkage.
- [ ] Compare deterministic output identities across repeated runs.
- [ ] Verify cancellation, restart, resume, insufficient disk, locked destination,
      corrupted resource, and application restart during a full-WGS job.
- [ ] Establish supported Windows hardware guidance and practical/comprehensive targets.
- [ ] Do not enable fastVEP as release-default until unexplained high-impact, splice,
      HGVS, mitochondrial, SV, or allele-matching discrepancies meet release thresholds.

### K. Remove code and dependencies superseded by fastVEP

- [ ] Inventory every AnnoCat crate, module, command, API route, JavaScript workflow,
      dependency, fixture, resource format, and documentation section by owner and purpose.
- [ ] Classify each item as retained AnnoCat responsibility, delegated to fastVEP,
      temporary migration/validation code, or unused/dead code.
- [ ] Produce a removal matrix showing replacement, equivalence evidence, affected
      commands/routes/tests, storage impact, binary-size impact, and deletion order.
- [ ] Review native annotation logic for duplication with fastVEP, including:
  - [ ] dbNSFP streaming lookup, chromosome Parquet conversion, DuckDB joins, and benchmarks;
  - [ ] consequence-related normalization or allele transformation duplicated by fastVEP;
  - [ ] source-specific matching/parsing that fastSA now performs;
  - [ ] transcript selection, HGVS, consequence parsing, or annotation-field assumptions;
  - [ ] preparation queues and resource states tied to obsolete output formats.
- [ ] Retain independent VCF syntax, assembly, REF, sample/genotype, stable identity,
      provenance, output-validation, and security checks where they protect the boundary
      around fastVEP rather than duplicate its biological annotation implementation.
- [ ] Compare AnnoCat normalization with fastVEP normalization on repeat indels,
      multiallelics, symbolic alleles, and genotype-indexed fields before choosing one owner.
- [x] Remove `query-dbnsfp`, `annotate-dbnsfp`, `prepare-parquet`, and
      `benchmark-parquet`; retain the verified source archive for fastSA conversion.
- [x] Remove dbNSFP-specific DuckDB/Parquet preparation code and generated schema logic.
- [x] Remove bundled DuckDB from the current CLI after measuring it as the dominant
      release-build cost; add it back behind a results/browser boundary when required.
- [ ] Audit Cargo dependencies with `cargo tree`, feature graphs, duplicate versions,
      license reports, vulnerability reports, and release-binary contribution measurements.
- [ ] Remove unused direct dependencies and disable unnecessary default features.
- [ ] Check whether `zip`, `flate2`, `md5`, `sha2`, `reqwest`, `fs2`, `rfd`, and
      `windows-sys` remain necessary and narrowly feature-scoped after source migration.
- [ ] Remove obsolete fixtures, generated Parquet resources, debug-only commands,
      VEP/Docker remnants, and stale UI states after their replacements are tested.
- [ ] Ensure large generated tools/resources remain excluded from source control while
      required licenses, pins, manifests, schemas, and small deterministic fixtures remain.
- [ ] Run formatting, strict Clippy, all tests, smoke annotation, bundle creation, and
      full resource verification after each deletion batch.
- [ ] Record before/after source lines, dependency count, compile time, bundle size,
      runtime memory, installed data size, and annotation equivalence.

### L. Safe GitHub release updates

Release updating is an application-binary operation, not a resource update. It must
never overwrite, relocate, reinterpret, or delete user configuration, annotation
runs, imported reports, downloads, installed source caches, or local case metadata.
The first release should check automatically but install only after explicit user
approval and a verified download.

#### L1. Establish the compatibility and storage boundary

- [ ] Document the immutable user-data roots as `config/`, `runs/`, `resources/`, and
      `downloads/`, including a resource directory outside `ANNOCAT_HOME`; exclude all
      of them from every application-update manifest and replacement operation.
- [ ] Define an allowlist of replaceable bundle files: `annocat.exe`,
      `annocat-report-worker.exe`, `tools/fastvep/fastvep.exe`,
      `launch-annocat.cmd`, `bundle-manifest.json`, README, and bundled licenses.
      Reject an update containing an absolute path, traversal, symlink/reparse point,
      duplicate case-insensitive path, alternate data stream, or unlisted destination.
- [ ] Version `config/annocat.json` and replace field-specific reconstruction with an
      atomic read-modify-write configuration store that preserves all supported settings.
      Back up the last valid config before migration and recover from an interrupted write.
- [ ] Keep browser-only display preferences backward compatible, but store update policy,
      last-check time, ignored version, and release channel in the versioned application
      config so they survive browser storage clearing and future UI packaging changes.
- [ ] Define additive, idempotent config migrations and tests from every released config
      version. Never silently reset an unknown future config; open read-only or fail with a
      recoverable diagnostic and retain the original bytes.
- [ ] Add a release compatibility contract containing AnnoCat config reader range,
      canonical report reader range, report-package reader range, resource-cache format,
      fastVEP cache compatibility identity, and minimum supported Windows version.
- [ ] Keep completed run manifests and Parquet results immutable. New readers must open all
      supported older schemas; any future local library-metadata migration must be atomic and
      independently reversible without rewriting annotation results or imported ZIPs.
- [ ] Treat a changed fastVEP/cache compatibility identity separately from an AnnoCat binary
      update. Reuse compatible installed caches; otherwise warn before installation and mark
      the affected source as requiring a rebuild without deleting its existing files.

#### L2. Publish verifiable release metadata

- [ ] Extend `scripts/package-windows.ps1` to generate a versioned release manifest with
      product, semantic version, channel, platform, published time, release notes URL,
      compatibility contract, asset size/hash, bundle file allowlist, and per-file SHA-256.
- [ ] Publish the portable Windows ZIP, its SHA-256, and the release manifest as GitHub
      Release assets from CI; make releases immutable after all assets are attached.
- [ ] Generate and publish GitHub artifact attestations for the Windows bundle, retain the
      source commit and workflow identity, and add Windows Authenticode signing when a code-
      signing certificate is available. Verification failure must be fatal, not a warning.
- [ ] Use stable semantic versions and separate stable/prerelease channels. The automatic
      stable check must ignore drafts and prereleases and must never downgrade the app.
- [ ] Add release CI checks that unpack the asset, compare every file to the release manifest,
      run the packaged smoke test, and test updating from each supported prior release fixture.

#### L3. Add a bounded update service and API

- [ ] Add a dedicated Rust update module with typed `UpdatePolicy`, `ReleaseIdentity`,
      `Compatibility`, `UpdateState`, and structured failure types; keep application updates
      separate from existing data-source update routes and preparation jobs.
- [ ] Query GitHub's latest-release API over HTTPS without authentication, send a versioned
      user agent, use conditional requests (`ETag`/`If-None-Match`), and check at most once per
      24 hours unless the user selects `Check now`. Failure must not delay application startup.
- [ ] Compare semantic versions strictly and reject malformed tags, unexpected repositories,
      redirects to disallowed schemes, unsupported platforms, oversized metadata, duplicate
      assets, and releases whose compatibility declaration cannot be understood.
- [ ] Expose bounded localhost APIs for current app identity, preferences, manual check,
      download, cancel-download, discard-download, and apply-on-restart. Protect every mutating
      route with the same CSRF/origin requirements as other local API mutations.
- [ ] Store update staging beneath `ANNOCAT_HOME/updates/`, never beneath `resources/` or
      `runs/`. Use `.partial` downloads, a small explicit size ceiling, bounded buffers, and an
      atomic rename only after the exact GitHub asset size and SHA-256 both verify.
- [ ] Persist enough verified download state to reuse a completely downloaded asset after an
      app restart. Initially restart interrupted partial downloads instead of claiming byte
      resume until validator-bound HTTP range resume is implemented and tested.
- [ ] Validate the ZIP and release manifest before extraction, extract only into a new staging
      directory, then re-hash every extracted file. Never execute a binary from an unverified
      archive or accept files absent from the allowlist.
- [ ] Block `Apply update` while annotation, report import/export, resource download, or cache
      preparation is active. Downloading may occur concurrently, but applying must wait until
      all jobs have reached a safe terminal or paused state.
- [ ] Record a bounded local update log containing versions, timestamps, verification stages,
      replaced application paths, health-check result, and rollback result without user data,
      genomic paths, report names, or case-note contents.

#### L4. Implement the Windows replacement helper and rollback

- [ ] Add a minimal `annocat-updater.exe` with no web server or annotation dependencies.
      Pass it an authenticated one-time update plan file rather than shell-concatenated paths.
- [ ] Have the main process write and fsync the plan, spawn the helper, stop accepting new
      jobs, close file handles, and exit. The helper waits for the exact parent process to exit
      before touching any application file.
- [ ] Resolve every source and destination beneath their expected roots immediately before
      use and reject links/reparse points or changed identities. Do not recursively delete or
      extract over `ANNOCAT_HOME`.
- [ ] Copy the currently allowlisted application files into a versioned rollback directory,
      install staged files with same-volume atomic renames where possible, and restore the old
      set if any replacement fails. Retain no more than one confirmed rollback version by default.
- [ ] Handle an unwritable installation directory explicitly: leave the verified update staged,
      explain that the portable folder is not writable, and offer `Open download page` or a
      user-approved elevated retry. Never request administrator access during ordinary startup.
- [ ] Restart AnnoCat on the same localhost port when available, pass the stable
      `ANNOCAT_HOME`, and perform a bounded `/api/health` check that verifies executable version,
      config readability, runs discovery, resource-directory discovery, and bundled fastVEP
      identity without starting an annotation or rebuilding data.
- [ ] Mark the update confirmed only after the health check. On launch failure, timeout, invalid
      config migration, or failed health response, restore the previous application files and
      restart the previous version with a visible rollback diagnostic.
- [ ] Keep a manual recovery path: the user can run the previous launcher/helper or copy the
      rollback application files without modifying any data directory.

#### L5. Add coherent update UI states

- [ ] Add an `Application updates` card to Settings, separate from Data Sources. Show the
      installed AnnoCat version, bundled fastVEP version/commit, channel, last successful check,
      automatic-check toggle, `Check now`, and a link to release notes.
- [ ] Default to `Automatically check for stable updates` enabled and `Automatically install`
      unavailable/off. Explain that checks are small and no annotation data is uploaded.
- [ ] Show a nonmodal global banner only when an applicable newer release exists:
      `AnnoCat <version> is available`, with `What's new`, `Download update`, and `Later`.
      Do not use browser alerts or interrupt the first-run setup flow.
- [ ] During download, show one compact application-update card with bytes, percentage,
      transfer speed, verification stage, `Cancel`, and `Install later`. Keep it visually
      distinct from database downloads so users cannot mistake it for a WGS source install.
- [ ] After verification, change the primary action to `Restart and update`; also provide
      `Later` and `Discard download`. Never restart automatically while the user is viewing
      results, editing case notes, configuring sources, or preparing an annotation.
- [ ] If jobs are active, replace the action with `Update ready — waiting for jobs` and list
      the blocking job categories. Allow cancellation of the update download without cancelling
      annotation or source jobs.
- [ ] Before restart, show a concise confirmation: application files will be replaced;
      configuration, completed/imported results, case notes, installed databases, queued job
      state, and the configured resource directory will remain untouched.
- [ ] On the next launch, show a dismissible `Updated to <version>` message with release notes.
      After rollback, show `The update could not start, so AnnoCat restored <old-version>` plus
      a bounded diagnostic and retry/download-page actions.
- [ ] Display compatibility warnings before download when meaningful. If a release cannot use
      an installed cache, name the affected source and expected rebuild, offer `Skip this
      version`, and never imply that the database has already been removed.
- [ ] Add accessible live status, keyboard focus behavior, reduced-motion-safe progress, and
      responsive layouts for the Settings card, banner, confirmation, failure, and rollback UI.

#### L6. Validate preservation and failure recovery

- [ ] Build update fixtures representing a fresh portable install and prior installs containing:
      custom resource paths; all supported config versions; installed core/supplementary caches;
      partial/paused preparation state; completed and imported runs; renamed reports; saved
      filters/columns; case notes/history; and a verified pending update.
- [ ] Byte-compare every protected user file before and after successful update, failed update,
      rollback, update cancellation, and repeated application. Confirm custom external resource
      directories are never traversed by the updater.
- [ ] Test locked executables, locked rollback files, no write permission, paths with spaces and
      non-ASCII characters, antivirus-delayed file access, low disk, truncated ZIP, wrong size,
      wrong hash, malformed manifest, traversal/reparse attacks, network loss, app crash, helper
      crash, power-loss simulation, unavailable port, and failed new-version health check.
- [ ] Test update checks offline, behind common proxies, with GitHub rate limiting, HTTP 304,
      deleted releases, prereleases, malformed versions, a newer incompatible release, and an
      already ignored version. All failures must leave normal annotation and result browsing usable.
- [ ] Test supported Windows 10 and 11 versions from an ordinary non-admin account and from a
      portable folder on another local drive. Document that network shares/removable media may
      prevent atomic replacement and provide a safe manual-update fallback.
- [ ] Prove downgrade protection and idempotence: applying the same verified update twice does
      nothing, stale helpers cannot replace newer files, and a rollback cannot silently downgrade
      user data or config into a representation the older app cannot read.
- [ ] Release in stages: ship check-only UI first, then verified background download, then opt-in
      restart/install after update fixtures and rollback tests pass. Do not add silent unattended
      installation in the first release.
