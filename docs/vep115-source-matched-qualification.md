# Source-matched Ensembl VEP 115.2 qualification

Status: Proposed qualification contract. A local, uncommitted GitHub Actions
draft exists, but it has not been run on GitHub and is not yet an approved
release gate.

Last updated: 2026-09-04

## Purpose

AnnoCAT uses fastVEP with transcript models built from the public Ensembl 115
GFF3 and the pinned GRCh38 FASTA. The archived Ensembl REST service uses a
different, richer Ensembl VEP transcript dataset. A direct comparison between
those two outputs can therefore mix together:

- differences in consequence and HGVS algorithms;
- differences in transcript membership;
- metadata available only in the Ensembl VEP cache or database; and
- actual fastVEP defects.

The release test must isolate implementation correctness from source-data
differences. It does this by running official Ensembl VEP 115.2 and candidate
fastVEP against the same GFF3, FASTA, variants, and options. Archived REST
comparison remains a separate compatibility lane.

This qualification does not install or invoke Ensembl VEP in AnnoCAT. The
official VEP oracle runs on a temporary GitHub-hosted Linux runner. Candidate
fastVEP testing also includes a Windows runner or equivalent controlled
Windows environment for the exact executable intended for the AnnoCAT release
ZIP.

## Relevance and proportionality for AnnoCAT

The requirements in this contract have different roles:

- **Direct annotation-correctness requirements** are mandatory for a release
  qualification. These include the source-matched VEP oracle, complete
  supported-consequence inventory, immutable field semantics, input-record
  projection and preservation, unresolved-difference handling, compatibility
  with caches from supported releases, and testing of the packaged Windows executable. Each
  can directly detect a user-visible annotation error in AnnoCAT.
- **Release-reproducibility requirements** support the correctness claim but
  do not independently establish biological correctness. These include action
  and toolchain identities, corpus provenance, and durable retention of the
  compact qualification record.
- **Out of scope for this contract** are independent clinical pathogenicity or
  causal-effect claims, running VEP on an end user's computer, retaining every
  large intermediate file indefinitely, and validating variant classes or
  annotation sources that AnnoCAT does not support.

This proportionality is intentional. AnnoCAT needs enough evidence to show
that its shipped fastVEP executable reproduces the declared VEP 115.2 behavior;
it does not need to become an independent replacement for biological or
clinical truth curation.

## Scope and scientific claim

Passing this qualification establishes implementation concordance with the
pinned official Ensembl VEP 115.2 executable for the declared inputs, source
files, semantic options, output fields, and sampled records. Official VEP is
the operational reference implementation, not an independent biological
ground truth.

The current qualification scope is:

- GRCh38 using the pinned no-alt reference FASTA;
- canonical chromosomes 1–22, X, Y, and MT;
- VCF SNVs, MNVs, short insertions and deletions, and split or unsplit
  multiallelic records defined by a versioned qualification input contract;
- transcript and intergenic consequences derived from the public Ensembl 115
  GFF3, including upstream and downstream consequences within 5,000 bases;
  and
- the exact output fields declared in the field contract below.

The claim excludes structural variants, CNVs, breakends, alternative and patch
contigs, RefSeq transcript models, regulatory and motif features, VEP plugins,
pathogenicity classification, phenotype ranking, and clinical validity unless
a later contract adds source-matched coverage for them. Supplementary-source
annotation such as ClinVar, gnomAD, dbNSFP, CADD, REVEL, and SpliceAI is
qualified by its own source-contract tests, not by this document.

No passing result may be described as proof that an annotation is biologically
causal, clinically pathogenic, or correct independently of VEP. A separate
expert-curated validation set is required before making a scientific claim
beyond VEP 115.2 concordance.

The versioned qualification input contract freezes the accepted VCF version,
canonical contigs, sequence-allele alphabet, insertion/deletion length bounds,
multiallelic handling, and treatment of null ALT (`.`), spanning deletion
(`*`), symbolic, structural, and breakend alleles. Changing the production
parser does not silently expand the qualification claim; the input contract,
corpora, and claim must be reviewed together.

## Oracle hierarchy

### Primary: source-matched official VEP

Official Ensembl VEP 115.2 is run in GFF mode with source-equivalent annotation
inputs. Official VEP requires a sorted, bgzipped, tabix-indexed GFF3, while
AnnoCAT currently gives fastVEP's production cache builder the original
downloaded Ensembl GFF3. The source-matched oracle lane therefore uses the
prepared canonical projection below, and a separate production-equivalence
lane proves that this preparation does not qualify a different transcript
model from the one users receive:

| Input | Required identity |
| --- | --- |
| Official implementation | Ensembl VEP `release/115.2` container, pinned by image digest |
| Original transcript source | `Homo_sapiens.GRCh38.115.gff3.gz` |
| GFF3 SHA-256 | `1e553efa8496d662e7264061a5cecf3001eb9a1157aaa66d80cd7ac35841509c` |
| Oracle-prepared annotation | Canonical contigs only, deterministically sorted, bgzipped, and tabix-indexed by the workflow |
| Reference | `GCA_000001405.15_GRCh38_no_alt_analysis_set.fna.gz` |
| Reference archive SHA-256 | `fb4243ebb014caf27111f24dd62b7ce42160f28581da6f8fcd6cba5977778d02` |
| Uncompressed FASTA SHA-256 | `9cce8b926416dd96b152deea85188495b75f7ac8d634cc723a017067be8702b7` |
| Assembly | GRCh38 |
| Distance | 5,000 bases |
| Consequence terms | Sequence Ontology |
| HGVS | Enabled explicitly |

Chromosome synonyms are supplied explicitly for `1`/`chr1` through
`22`/`chr22`, `X`/`chrX`, `Y`/`chrY`, and `MT`/`chrM`.

The workflow must record a content hash of the prepared, uncompressed canonical
GFF3 records as well as hashes of the compressed GFF3 and index. It also
records the versions of the sorting, bgzip, tabix, and FASTA-indexing tools.
This distinguishes semantic source identity from compression bytes that may
vary between tool builds.

The production-equivalence lane builds one transcript cache from the
oracle-prepared GFF3 and a second cache through AnnoCAT's exact production path
from the original downloaded GFF3. It annotates every qualification corpus with
both and requires identical supported-contig annotation rows under the field
contract. Cache bytes need not match when serialization metadata differs; the
complete semantic output and transcript inventory must match. This lane is in
addition to direct-GFF versus cache-backed parity within the oracle-prepared
source.

The reference identity must also be enforced by AnnoCAT's production source
installation, not only by the qualification runner. New downloads verify the
pinned archive SHA-256 before preparation. An existing installation may be
accepted without downloading or rebuilding when its prepared FASTA has the
pinned uncompressed SHA-256; its manifest may then be upgraded in place to
record that identity. A different hash is not treated as the qualified
reference merely because the file decompresses and has valid FASTA structure.

This is the operational release oracle for consequence, impact, coordinate
projection, exon and intron numbering, HGVSc, HGVSp, amino acids, codons,
distance, strand, and transcript identity within the declared corpora. It is
not a biological or clinical truth set.

### Secondary: archived Ensembl REST

The archived release-115 REST service remains valuable for comparison with
the normal Ensembl VEP dataset. It is not the primary algorithm oracle because
its transcript records and auxiliary metadata are not identical to the public
GFF3.

The frozen and hashed request, response, and software metadata for the existing
197-record REST contract remain a reproducible regression gate. A live request
to the archived service is a diagnostic availability and compatibility check,
not a release gate: network failure or a later archive-side change must not
invalidate a source-matched qualification. Expanded boundary and ClinVar REST
comparisons are likewise diagnostic until every difference has been classified
using the source-matched result. A diagnostic failure must remain visible in
the workflow summary and uploaded report; it must not be silently discarded.

An expanded REST difference may become an accepted contract entry only when:

1. official VEP in source-matched GFF mode agrees with fastVEP;
2. the REST response differs because its transcript dataset or metadata
   differs;
3. the exact allele, feature type, and transcript identity are recorded; and
4. no gene-wide, transcript-wide, or wildcard exception is used.

If official source-matched VEP agrees with REST instead, the difference remains
a fastVEP defect and must be corrected.

## Frozen qualification corpora

Each corpus is compared and reported separately. Identity or field counts are
never added across corpora.

| Corpus | Records | SHA-256 | Purpose |
| --- | ---: | --- | --- |
| Legacy | 197 | `b846a65bf671ddc7afcf9e6729d35bd85c31dc3f089cee3a7ce81926fa39b9d5` | Existing public chromosome 21, 22, X, Y, and MT regression coverage |
| Transcript boundary | 1,262 | `ea912b66c7dfdc127c038faaffece824d78ed2873997d9cf7255290afdd58917` | Coding, noncoding, strand, exon, intron, CDS, start, stop, insertion, deletion, and MNV boundaries |
| Reviewed ClinVar | 400 | `3de468d953892d970c132a3fcb48f2e5e7e98345dc10ceb25874f624daf6d4de` | Deterministic clinically realistic benign and pathogenic consequence classes |

ClinVar significance selects realistic variants but is not used as the
consequence oracle.

Every frozen or generated corpus must have a committed machine-readable
manifest. The manifest records the upstream URL, release date or version,
source byte size and SHA-256, assembly, eligible-record count, inclusion and
exclusion rules, deduplication and clustering rules, sampling algorithm and
version, random seed when applicable, selected record or transcript IDs,
stratum counts, generator commit, output record count, and output SHA-256.
The generator and manifest are part of the qualification input and must be
reviewed with the VCF. A final VCF hash alone is not sufficient provenance.

## Coverage and confidence policy

The qualification has two explicitly different outcomes:

- **Release regression qualification** requires the three compact frozen
  corpora, all known regression fixtures, field and input integrity, production
  source equivalence, supported-old-cache compatibility, and the packaged
  Windows executable to pass. Its claim is limited to the declared fields and
  tested inputs; it is not described as broad or genome-wide concordance.
- **Broad VEP 115.2 concordance qualification** additionally requires the
  untouched holdout, expanded boundary coverage, metamorphic cases, and large
  discovery cohort below. Its report states the sampled population and may use
  only the statistical interpretation permitted below.

The three compact frozen corpora contain 1,859 input records. They are
sufficiently diverse for a release regression gate and for preventing
recurrence of the known defects, but they do not establish comprehensive
genome-wide VEP 115.2 concordance. Each corpus remains an independently
reported test; the combined record count must not be presented as a count of
independent biological observations or allele-transcript comparisons.

Current reported coverage includes the following. Until the required
generation manifests are committed, only the record counts and VCF hashes are
independently reproducible; the asserted biological strata remain provisional:

- legacy chromosome 21, 22, X, Y, and mitochondrial cases, including indels,
  MNVs, multiallelic records, and selenocysteine;
- 1,262 generated boundary variants across 28 coding and noncoding
  transcripts, both strands, exon-count and CDS-phase groups, and autosomal,
  sex-chromosome, and mitochondrial loci; and
- 400 reviewed ClinVar variants, with 20 benign and 20 pathogenic examples in
  each of 10 consequence classes: frameshift, in-frame deletion, in-frame
  insertion, missense, splice acceptor, splice donor, start lost, stop gained,
  stop lost, and synonymous.

These ten ClinVar classes are coding-focused realism strata, not the complete
set of consequences in AnnoCAT's declared scope. A committed
supported-consequence manifest must enumerate every atomic Sequence Ontology
term in the declared configuration and a finite, predeclared set of observed
or high-risk compound-term patterns. It must not claim to enumerate every
theoretically possible combination. The required set includes applicable splice
region, UTR, intronic, noncoding transcript and exon, NMD transcript,
upstream/downstream, intergenic, retained start/stop, and other supported
terms. Each entry has one explicit status: qualified, exercised but not yet
qualified, or excluded with a reason. An unrepresented term cannot inherit a
qualification claim from a different consequence class.

A release report may claim only entries marked qualified. An entry marked
exercised but not yet qualified remains visibly outside that claim. Broad VEP
115.2 concordance cannot pass while an entry declared in scope remains
unqualified; it must either gain sufficient predeclared coverage or be moved
outside scope with a reviewed reason.

Coverage reports use two complementary units. The independently selected VCF
record or predeclared locus cluster is the unit for sampling and statistical
interpretation. Allele-feature-transcript rows, including compound consequence
terms and duplicate multiplicity, are the unit for completeness reporting and
exact output comparison. A large number of transcript rows from one variant
must not be counted as independent observations.

This design is stronger for regression detection than an equally sized
unstratified random sample because it exercises known consequence mechanisms.
It is still limited by the small number of boundary transcripts, only 40
ClinVar records per consequence class, dependence among variants and
transcripts, and selection informed partly by previously observed failures.
It therefore cannot measure a genome-wide error rate or reliably discover all
previously unknown divergence classes.

The qualification uses three distinct corpus roles:

- **Regression corpora** are the permanent small fixtures. They may be
  inspected while correcting code and must pass after every relevant change.
- **Qualification holdout** records are selected and frozen before candidate
  output is inspected. They are used for the final broader-concordance claim.
  If a candidate is changed in response to a holdout result, that holdout
  becomes regression data and a new untouched holdout must be selected for the
  final claim.
- **Discovery records** broaden failure discovery. They do not become an
  accuracy estimate merely because they are numerous.

Before describing fastVEP as broadly concordant with VEP 115.2, qualification
must add the following coverage:

1. Create an untouched reviewed-ClinVar qualification holdout with
   approximately 300 records per current coding realism stratum, or
   approximately 3,000 records across the current 10 strata. Select VCF records without
   replacement from a predeclared eligible source population and apply
   predeclared caps for repeated loci, genes, and transcript clusters. Maintain
   benign and pathogenic representation where available, but do not double the
   cohort solely by clinical significance because significance does not alter
   VEP consequence calculation. Assign each consequence stratum from frozen
   source-matched official VEP 115.2 output generated before any candidate
   output is inspected; ClinVar's molecular-consequence label is not the
   annotation oracle.
2. Expand boundary testing from 28 to approximately 150–200 deliberately
   stratified transcripts. Include coding and noncoding biotypes, both
   strands, exon-count and CDS-phase combinations, incomplete CDS annotations,
   NMD transcripts, pseudogenes, autosomes, X, Y, PAR boundaries, and
   mitochondrial transcripts where supported.
3. Add focused representation cases for long indels, complex replacements,
   MNVs, repeat-associated left/right representations, padded and minimal
   equivalent alleles, and split and unsplit multiallelic records.
4. Add 200–500 predeclared metamorphic pairs. Each pair's manifest identifies
   its relation and the exact fields that must remain invariant or may differ.
   Required relations include split versus unsplit multiallelic records,
   minimal versus correctly padded equivalent alleles, and input-record order
   permutations. Consequence and HGVS expectations come from official VEP;
   do not assume that genomic coordinates, uploaded-allele fields, or
   `HGVS_OFFSET` remain identical when VEP intentionally normalizes or shifts a
   representation.
5. Run a deterministic 10,000–25,000-record discovery cohort before a broad
   VEP 115.2 concordance qualification and after a substantial
   consequence-engine change. Refresh
   the sampled records when ClinVar or the supported Ensembl release changes;
   do not rerun or refresh it on an arbitrary calendar schedule. Sample from
   reviewed ClinVar, GIAB difficult regions, and supported representation edge
   cases. During initial workflow development, execution may continue after a
   discovery mismatch so the complete report can be collected, but that run
   cannot qualify a release. Once reviewed, any unexplained source-matched
   difference within the declared scope blocks qualification. A difference
   proven to be outside scope or caused by a documented oracle-source
   distinction is recorded explicitly rather than silently ignored.
6. Reduce each newly discovered algorithmic difference to a small permanent
   regression fixture. The large discovery cohort complements the frozen
   release gate; it does not replace it.

The approximately 300-record target per consequence class is a coverage
planning target, not an accuracy guarantee. A one-sided binomial rule-of-three
bound may be reported only for the untouched holdout when the report defines
the target population, random sampling procedure, independent unit of
analysis, mismatch endpoint, and treatment of locus, gene, and transcript
clustering. Targeted regression and boundary records must never be included in
that confidence calculation. A zero-difference result otherwise establishes
concordance only for the declared sources, options, fields, and sampled inputs.

GIAB benchmark and difficult-region records supply realistic and challenging
variant representations. They are not an oracle for transcript consequence or
HGVS correctness; official source-matched VEP remains the oracle for those
outputs.

Any future addition of structural variants, CNVs, breakends, alternative
contigs, patches, RefSeq transcripts, regulatory features, or plugins requires
its own source-matched corpus, explicit options, required fields, and claim
amendment before it enters the qualification scope.

## Field contract

The checked-in source-matched contract, rather than a constant embedded only in
the comparator, owns the complete ordered field inventory. Every production
fastVEP CSQ field must have exactly one disposition:

- **exact**: compare every value and multiplicity with source-matched official
  VEP;
- **input-derived**: compare with an independent deterministic extractor that
  reads the pinned input VCF and shares no parsing implementation with fastVEP;
- **source-derived**: official GFF-mode VEP does not expose an equivalent
  value, so compare fastVEP with a reviewed deterministic extractor that reads
  the same pinned GFF3 and/or FASTA and shares no parsing implementation with
  fastVEP; or
- **excluded**: the field is outside this source-matched consequence contract
  and has a specific documented reason and separate validation owner.

The comparator fails if a declared exact, input-derived, or source-derived
field disappears, if the workflow's requested field list differs from the
contract, or if a production CSQ field has no disposition. A code change
therefore cannot make a test pass by silently removing a required field.

At minimum, exact comparison covers:

- allele, consequence, and transcript identity: `Allele`, `Consequence`,
  `IMPACT`, `Feature_type`, and `Feature`;
- gene and transcript context: `SYMBOL`, `Gene`, and `BIOTYPE`;
- coordinate projection: `EXON`, `INTRON`, `cDNA_position`, `CDS_position`,
  and `Protein_position`;
- sequence consequence: `HGVSc`, `HGVSp`, `Amino_acids`, and `Codons`;
- proximity and orientation: `DISTANCE` and `STRAND`; and
- transcript state: `FLAGS`, `CANONICAL`, and `TSL`.

The contract must additionally govern the product-visible or allele-mapping
fields `REF_ALLELE`, `UPLOADED_ALLELE`, `MANE`, `MANE_SELECT`,
`MANE_PLUS_CLINICAL`, `CCDS`, `ENSP`, and `HGVS_OFFSET`. The official lane
enables the VEP options required to emit each applicable field.
`REF_ALLELE` after minimization, `UPLOADED_ALLELE` before minimization, and
`HGVS_OFFSET` from VEP's HGVS shifting are exact comparisons when AnnoCAT emits
them; the input-derived disposition may additionally verify their relationship
to the submitted VCF. `ENSP` and `CCDS` are exact when the shared source exposes
them.

For MANE, compare exactly the designation that official GFF-mode VEP can derive
from the shared GFF3. Do not synthesize or require the paired alternative
transcript accession that official VEP provides only from its cache or
database. Any product field carrying such an accession needs a separately
pinned source and validation owner. Fields must not be dropped merely to obtain
concordance. If AnnoCAT does not emit `ALLELE_NUM`, the comparator must still
map each CSQ row unambiguously to one input ALT index and fail an ambiguous
multiallelic mapping.

The following fields may be excluded from the GFF-to-GFF comparison only for
the stated source reasons:

- `SYMBOL_SOURCE`, because the public GFF3 supplies the symbol without the VEP
  cache provenance;
- `HGNC_ID`, because the GFF3 does not contain the VEP cache cross-reference;
- `APPRIS`, because fastVEP's GFF loader does not retain that cache metadata;
  and
- `SOURCE`, because it is an invocation label for the same GFF3 rather than a
  predicted consequence.

`FLAGS` is not excluded from the source-matched lane. If the shared GFF3
provides `cds_start_NF` or `cds_end_NF`, both implementations must interpret
and emit it consistently. The initial release gate requires focused real or
synthetic fixtures for both flags and both transcript strands, including the
expected suppression of invalid start- or stop-boundary consequences.

The GFF-to-REST lane may exclude `FLAGS` because the public GFF3 lacks some of
the completeness metadata returned by the normal VEP dataset. AnnoCAT must not
infer these flags from sequence length or transcript geometry.

The semantic-options manifest explicitly records all output-affecting options
and defaults. This includes Sequence Ontology terms, 5,000-base upstream and
downstream distance, HGVS generation, VEP's HGVS 3-prime shifting behavior,
allele minimization, transcript selection, canonical designation, transcript
numbers, protein identifiers, biotype, TSL, and chromosome synonyms. Official
VEP and fastVEP commands may use different syntax only when the manifest shows
that the semantics and resulting fields are equivalent.

## Reference and VCF integrity

Every positive-corpus record must be checked against the pinned GRCh38 FASTA
before annotation. The official lane enables VEP reference checking or
performs an equivalent fail-closed preflight. Any unexpected reference
mismatch, skipped record, unknown contig, or ambiguous ALT association in a
positive corpus fails qualification and appears in the complete report.

Before annotation, the qualification applies the versioned input contract to
create the same in-scope VCF projection used by AnnoCAT. It records the count
and ordered identity digest of retained records and alleles and, separately,
the count, reason, and identity digest for every skipped record or allele.

A separate record-preservation check removes only declared tool-added headers
and annotation fields, then requires the projected input and candidate output
to retain the same record count and the same `CHROM`, `POS`, `ID`, `REF`,
`ALT`, `QUAL`, `FILTER`, original `INFO`, `FORMAT`, sample names, genotypes,
and sample values. VCF fields are compared structurally according to their
declared semantics rather than by incidental header or key ordering. Any value
mutation remains a failure. Annotation concordance does not excuse alteration
or loss of an in-scope source record.

Focused negative fixtures verify fail-closed or explicitly documented behavior
for a reference mismatch, an unrecognized contig, a non-variant allele, and
each unsupported variant class. For the currently intentional removal of
non-variant records, the expected skipped count, reason, and identity digest
must agree with the projection record. Distance fixtures cover 4,999, 5,000,
and 5,001 bases on both transcript strands so the declared 5 kb boundary is
tested rather than merely configured.

## Supplementary annotation parity at practical scale

Supplementary providers such as dbNSFP, gnomAD, CADD, and SpliceAI can be tens
or hundreds of gigabytes. They are not downloaded or rebuilt by this VEP
qualification. Supplementary parity is a separate overall AnnoCAT release
prerequisite owned by
[Source validation in GitHub Actions](github-actions-source-validation.md).
That plan defines the synthetic contract matrix, pinned real-source subsets,
complete-cache integrity checks, AnnoCAT result projection, and the limits of
the resulting claim. Its status is reported alongside, but not merged into,
the VEP concordance verdict.

A supplementary-source failure may block the overall AnnoCAT release, but it
must not be described as a VEP consequence-concordance failure. This document
does not duplicate or override the source-validation plan.

## Required workflow lanes

The manual workflow accepts an optional fastVEP commit, tag, or branch. A blank
value resolves to `config/fastvep-pin.json`. The requested ref and resolved
commit are both recorded. A release qualification is valid only when the
resolved commit is the immutable commit later written to the AnnoCAT pin;
passing a mutable branch name alone is not an artifact identity.

The common lanes required for the release regression qualification are:

1. build the exact candidate fastVEP revision with its lockfile;
2. verify every downloaded source, corpus, generator, manifest, and contract
   hash;
3. validate every in-scope input REF allele against the pinned FASTA, verify
   retained and skipped projection identities, and run the VCF
   record-preservation gate;
4. run official VEP 115.2 with the pinned image digest and semantic-options
   manifest;
5. require byte-identical fastVEP direct-GFF and newly built transcript-cache
   output for all three corpora;
6. build a cache through AnnoCAT's exact production path from the original GFF3
   and require semantic parity with the oracle-prepared cache for all corpora;
7. reproduce each supported previous-release cache with the exact released
   builder and pinned production inputs declared by the immutable cache-
   compatibility manifest, then run the candidate against that unchanged
   cache and require the same exact field contract;
8. require exact source-matched official-VEP agreement under the immutable
   field contract for all three corpora;
9. build or obtain the exact Windows fastVEP executable intended for the
   release ZIP, run it against the frozen official-VEP outputs, and require the
   same result as the Linux candidate;
10. require every comparator self-test, including missing field, changed field,
    missing row, extra row, duplicate multiplicity, ambiguous multiallelic
    mapping, reference mismatch, and input-record mutation; and
11. require the frozen archived-REST contract for the legacy corpus, and run a
    live archive refresh only as a diagnostic.

Broad VEP 115.2 concordance qualification additionally requires:

12. verify the manifests and hashes for the untouched ClinVar holdout,
    expanded transcript-boundary corpus, and metamorphic corpus;
13. run the applicable common source-matched, production-equivalence,
    supported-cache, field, input-integrity, and packaged-Windows lanes on those
    expanded corpora; and
14. run the discovery cohort and prevent broad qualification after any
    source-matched discrepancy until it has been reviewed and either resolved
    or demonstrated to be outside the declared scope.

A release regression qualification does not inherit the broad-concordance
claim. Conversely, a broad qualification cannot omit any of its additional
lanes merely because all compact regression corpora pass.

The overall AnnoCAT release also requires the separately reported
supplementary-source prerequisite described above. A failure there can block
the product release, but it is not reported as a VEP consequence-concordance
failure.

The checked-in cache-compatibility manifest is the only authority for the term
"supported previous cache." Each entry records the AnnoCAT release, immutable
release-bundle URL and SHA-256, path and SHA-256 of the fastVEP executable in
that bundle, fastVEP commit, transcript-source and reference identities, exact
production cache-build command, cache schema version, expected cache byte size
and SHA-256, and support status. A cache is enrolled only after two independent
builds with those identities produce the expected byte hash. Removing support
requires an explicit, user-reviewed migration or support-policy decision; it
cannot be accomplished by deleting a failing test entry.

### Previous-release cache construction in GitHub Actions

AnnoCAT does not need to collect a cache from an end user or permanently store
a complete transcript-cache artifact. For each supported release, the manual
release-qualification workflow runs a Windows compatibility job that:

1. downloads the immutable released AnnoCAT ZIP and verifies its manifest
   SHA-256;
2. extracts and verifies the released `tools/fastvep/fastvep.exe`;
3. downloads or restores the exact pinned Ensembl GFF3 and GRCh38 reference,
   verifies their source and prepared-content identities, and prepares them by
   the released production procedure;
4. invokes the released executable with AnnoCAT's exact production cache-build
   arguments to create the previous-release baseline cache;
5. requires the baseline cache size, SHA-256, schema, and structural
   verification report to match its compatibility-manifest entry;
6. records the baseline hash, gives the cache to the candidate as read-only
   input, and runs candidate `cache-verify` and annotation over every required
   frozen corpus;
7. requires candidate output from the previous-release cache to equal candidate
   output from the newly built production cache under the complete field
   contract, and requires both to agree with source-matched official VEP; and
8. hashes the baseline again and fails if the candidate changed it.

Creating the baseline with the released executable is not a candidate cache
rebuild or migration. It deterministically reproduces the cache that the
supported release installed. After that construction step, the candidate may
only read the baseline. It must not rebuild, convert, repair, replace, or
modify it. Output produced by the old executable is retained as diagnostic
evidence, not used as the correctness oracle, because an intentional candidate
fix may differ from the old implementation.

The official VEP lane may run on Linux and transfer only its compact VCF output
and manifests to the Windows compatibility job. The complete GFF3, FASTA, and
previous-release cache remain temporary runner files. GitHub's download cache
may accelerate immutable source downloads, but it is not the source of truth
for compatibility and is never accepted without the pinned content checks.

Ordinary pull requests may use a compact cache fixture built by the released
executable to detect schema and reader regressions quickly. The full Ensembl
115 cache construction is mandatory in the manually triggered release-
qualification workflow; the compact fixture cannot replace it.

If a supported installed cache cannot produce the required results with the
candidate, the change is cache-incompatible. Qualification fails until the
implementation is made backward-compatible or AnnoCAT adopts the explicit
migration policy. The candidate must not silently rebuild the old cache and
call that compatibility.

The expanded REST differential is recorded but does not become a release gate
until its source differences have exact reviewed contracts.

The job uploads:

- official VEP and fastVEP versions and immutable identities;
- requested and resolved fastVEP refs;
- Linux and Windows candidate executable SHA-256 values and build provenance;
- all input, source, cache, output, and contract hashes;
- corpus selection and semantic-options manifests;
- source-matched comparison reports;
- REST request, response, software-version, and comparison reports; and
- official VEP, direct fastVEP, new-cache, previous-release-cache, and packaged
  Windows fastVEP VCF outputs.

The qualification records the resolved Rust toolchain, operating-system image,
compiler and linker identity, container digest, and versions of source-
preparation utilities. Third-party GitHub Actions used by the release gate are
pinned to immutable commits where available, and their resolved identities are
recorded. This is release-process hardening: it protects reproducibility and
supply-chain integrity but is not presented as independent biological
validation.

The large FASTA, GFF3, newly built fastVEP transcript cache, and baseline cache
reproduced by a previous release are temporary runner data and are not uploaded
as release artifacts. The report still records their identities and the
identity of each compatibility cache used.

Ordinary workflow artifacts may expire. Each qualified release therefore keeps
a durable compact evidence record as a GitHub release asset or committed
release record. It contains the final machine-readable report, manifests,
hashes, resolved commits and tool identities, cache identities, packaged
Windows executable checksum, and workflow run URL. Large reproducible
intermediate VCFs and source files do not need permanent retention when their
pinned inputs, generation procedure, and hashes are preserved.

## Pass and failure interpretation

A source-matched lane passes only with:

- no reference-invalid, unexpectedly skipped, missing, extra, or mutated
  in-scope projected records;
- no missing or extra allele/feature/transcript identities;
- no differing required field values;
- no missing required output field;
- no unclassified production field; and
- no unexplained duplicate-row or row-multiplicity difference.

The comparator records deterministic, machine-readable lists of every missing,
extra, and differing annotation row, including the complete field values and
multiplicity for duplicate identities. It also records every missing, extra,
or changed input record. A short summary may show ten examples, but the
uploaded report cannot truncate the authoritative difference list. Field-level
analysis must handle multirow identities instead of skipping them.

The comparator's own version or commit and SHA-256 are part of the report. Its
self-tests deliberately introduce every failure category listed in the
workflow contract and must demonstrate that each causes a nonzero result.

If source-matched VEP and fastVEP disagree, reduce the difference to a minimal
fixture, correct the shared root cause, rerun every previously passing fixture,
and rerun all three frozen corpora. Do not accept an aggregate percentage.

If source-matched VEP and fastVEP agree but REST differs, retain the fastVEP
result and classify the REST difference as a source-data difference. It may be
contracted only by exact identity after review.

## Release artifact binding

The qualification record binds together:

- the immutable fastVEP source commit and lockfile hash;
- the official VEP image digest;
- the source, corpus, contract, and semantic-options hashes;
- the Linux candidate executable hash;
- the packaged Windows executable hash;
- each current and previous-release transcript-cache identity;
- the GitHub workflow run URL and final status; and
- the durable compact evidence-record location and checksum.

The AnnoCAT release pin must equal the qualified fastVEP commit. The release
ZIP must contain the qualified Windows executable hash. A rebuild with a
different hash requires either a reproducible-build identity showing it is the
same artifact or a rerun of the packaged-binary qualification lane.

## Compatibility and user impact

- The workflow does not change AnnoCAT annotation commands.
- The workflow does not add VEP to the AnnoCAT release ZIP.
- The workflow does not change the fastVEP transcript-cache schema.
- Existing installed transcript caches are considered readable and usable only
  after the previous-release-cache lane reproduces the baseline with the exact
  old release and proves that the candidate reads it without modifying or
  rebuilding it.
- End users do not download or run official VEP.
- The official VEP container and large reference inputs exist only on the
  temporary GitHub runner and its validation-download cache.

## Implementation order

1. Review and approve this qualification contract.
2. Commit the corpus generators and manifests, supported-consequence and input
   contracts, cache-compatibility manifest, immutable field and semantic-
   options contracts, production-GFF equivalence and reference checksum checks,
   full-diff comparator, and comparator self-tests.
3. Finish and locally validate the GitHub Actions workflow syntax and helper
   script tests.
4. Commit the fastVEP corrections on `codex/vep115-concordance` and push that
   candidate branch without changing AnnoCAT's pin.
5. Run the manual workflow with the candidate commit as `fastvep_ref` and
   inspect every complete source-matched, compatibility, and REST report.
6. Fix any source-matched defects and add exact REST contracts only for proven
   source differences. Any inspected qualification holdout then becomes
   regression data; generate a new untouched holdout before a broader
   concordance claim.
7. Rerun until every release-gating lane, including the supported-old-cache and
   packaged-Windows-binary lanes, passes.
8. Update AnnoCAT's fastVEP pin to the exact qualified commit, build the release
   ZIP, verify that it contains the qualified executable hash, and run the
   packaged end-to-end gate.
9. Retain the workflow run URL and complete artifact-binding record with the
   release evidence.
10. Align the fastVEP fixture README and all dependent validation documents so
    they identify source-matched official VEP as the primary implementation
    oracle and archived REST as the compatibility oracle.

## Current implementation status

As of 2026-09-04:

- the local fastVEP branch is named `codex/vep115-concordance`;
- fastVEP correctness changes remain uncommitted;
- earlier forms of the Annotation concordance workflow are committed; the
  expanded local draft and the three compact frozen VCF inputs have been
  created, but the current draft and new inputs are uncommitted;
- the supported-consequence manifest and versioned qualification input
  contract do not yet exist;
- the expanded ClinVar, transcript-boundary, metamorphic, and discovery
  cohorts are documented requirements but have not been generated or added to
  the workflow;
- the current field contract does not yet own the complete production field
  inventory, and the current workflow omits product-visible fields including
  `ENSP`, `CCDS`, MANE metadata, and `HGVS_OFFSET`;
- committed corpus-generation manifests, reference and complete VCF integrity
  checks, complete row-level difference output, and the expanded comparator
  self-tests do not yet exist;
- the supported-previous-cache and packaged-Windows-executable qualification
  lanes do not yet exist;
- the immutable cache-compatibility manifest listing every supported release
  cache and its identities does not yet exist;
- the current `v0.1.0` GitHub release contains the Windows release ZIP and its
  checksum but no transcript-cache asset; the planned compatibility lane will
  therefore reproduce its baseline cache temporarily with the verified
  released executable and pinned production inputs;
- the production-equivalence lane does not yet compare a cache built from the
  original GFF3 through AnnoCAT's exact production path with the
  oracle-prepared cache;
- the production source catalog pins the GFF3 checksum but does not yet pin the
  GRCh38 reference archive checksum used by this qualification;
- the current legacy REST lane fetches the archived service live; its frozen,
  hashed regression response and separate diagnostic refresh do not yet exist;
- the workflow still uses version tags for GitHub Actions, retains ordinary
  artifacts for 30 days, and does not yet publish the durable compact release
  evidence record;
- the expanded workflow draft has not been committed, pushed, or run on
  GitHub;
- the supplementary-source workflow currently proves synthetic OSA1/OSA2 and
  AnnoCAT projection parity; independently pinned real-source subsets are not
  yet implemented;
- the fastVEP fixture README still describes archived REST as the consequence
  oracle and must be aligned with this source-matched hierarchy;
- no source-matched official VEP results exist yet; and
- no AnnoCAT pin, packaged executable, or release has changed.

## Scientific and technical references

- [Ensembl release 115 VEP annotation sources](https://sep2025.archive.ensembl.org/info/docs/tools/vep/script/vep_cache.html)
  documents GFF/GTF use, sorting and indexing, and the FASTA requirement for
  transcript construction and offline HGVS.
- [Ensembl release 115 VEP command-line options](https://sep2025.archive.ensembl.org/info/docs/tools/vep/script/vep_options.html)
  defines reference checking, allele handling, HGVS shifting, distance,
  transcript metadata, and output options.
- [Ensembl VEP consequence definitions](https://sep2025.archive.ensembl.org/info/genome/variation/prediction/predicted_data.html)
  enumerate the transcript, noncoding, proximity, and intergenic consequence
  terms that the supported-consequence manifest must classify.
- [Official Ensembl VEP 115.2 container](https://hub.docker.com/layers/ensemblorg/ensembl-vep/release_115.2/images/sha256-ed6b660e278458109afc00d5e730aee240a3bf77e817796386033e1a18963152)
  records the pinned Linux AMD64 image digest.
- [McLaren et al., *The Ensembl Variant Effect Predictor*](https://pmc.ncbi.nlm.nih.gov/articles/PMC4893825/)
  describes VEP's consequence model and reproducibility purpose.
- [Hanley and Lippman-Hand, *If Nothing Goes Wrong, Is Everything All Right?*](https://jhanley.biostat.mcgill.ca/c607/ch08/zero_numerator.pdf)
  defines the rule of three and its sampling interpretation.
- [Chen et al., metamorphic testing for bioinformatics](https://pmc.ncbi.nlm.nih.gov/articles/PMC2657898/)
  supports predeclared property-based relations for scientific software.
- [Dwarshuis et al., GIAB genomic stratifications](https://pmc.ncbi.nlm.nih.gov/articles/PMC11489684/)
  describes difficult-region stratification and its role in benchmark
  interpretation.
- [Tuteja et al., clinical annotation-tool comparison](https://pmc.ncbi.nlm.nih.gov/articles/PMC9577137/)
  demonstrates both the value and limits of using VEP relative to a manually
  curated HGVS set.
- [GitHub Actions artifact retention](https://docs.github.com/en/actions/tutorials/store-and-share-data#configuring-a-custom-artifact-retention-period)
  documents that workflow artifacts expire according to their configured
  retention period, motivating a compact durable release record.
