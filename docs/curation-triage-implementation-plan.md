# AnnoCAT curation and triage implementation plan

## Outcome

Turn the existing results viewer into a coherent local case workspace that supports
annotation review, candidate prioritization, gene-level exploration, case context,
and reproducible sharing. The implementation must preserve AnnoCAT's existing
properties:

- immutable canonical Parquet results and run provenance;
- bounded server-side queries that do not load a WGS result into the browser;
- useful operation without installed annotation databases when opening an imported
  report;
- local, privacy-preserving review by default; and
- dynamic evidence fields, including fields introduced by future source versions.

This plan defines the product structure and delivery order. The detailed checklist
in `TODO.md` remains the source of truth for lower-level annotation, source,
packaging, and release work.

## Product structure

Opening a card in **Browse Results** opens one **Case Workspace**. It is not a new
top-level sidebar section. The workspace header contains the report name, assembly,
completion date, review progress, case-context summary, and report actions.

The workspace has four views. **All variants is always the default view** when a
report is opened unless the user follows a stable link to a specific allele, gene,
or view:

1. **All variants** — the current high-scale results table, filtering, selection,
   details, and export experience.
2. **Candidates** — a persistent user-controlled shortlist. Variants enter it only
   through explicit manual addition, an explicit `Add filtered variants` action, or
   a prioritization preset the user deliberately reviews and runs. It never silently
   hides or changes the complete result.
3. **Overview** — case context, review progress, candidate counts, notable evidence,
   source coverage, and warnings.
4. **Genes** — genes represented in the result, their candidate variants,
   transcript context, case review state, and later phenotype rank.

The existing variant details drawer is shared by Candidates, All variants, and
Genes. A pinned review block appears at the top so a reviewer does not have to move
between unrelated screens to curate a variant.

## Core design decisions

### Immutable result plus mutable local overlay

Canonical Parquet, the run manifest, provenance, and the originally imported ZIP
remain immutable. User work is stored beneath:

```text
<ANNOCAT_HOME>/runs/.annocat-library/<run-id>/
    metadata.json
    case-notes.md
    case.sqlite

<ANNOCAT_HOME>/config/
    global-filter-presets.json
```

`case.sqlite` is a versioned, transactional local overlay. It is preferable to a
set of JSON files once variant reviews, tags, candidate memberships, and audit
events become numerous. It is never attached to DuckDB and is never allowed to
alter canonical result files.

Initial tables:

| Table | Purpose |
| --- | --- |
| `case_context` | Case summary, HPO terms, gene list, sample roles, pedigree metadata, and revision |
| `variant_review` | One current review state per stable allele ID |
| `variant_tag` | User-defined and controlled tags per allele |
| `triage_run` | Preset/rule version, thresholds, source versions, time, and result counts |
| `triage_result` | Candidate allele, rank, score, and structured reasons for a triage run |
| `workspace_state` | Active global-filter reference, columns, sorting, open view, and other report-local navigation state |
| `review_event` | Append-only bounded history for recovery and audit |

All mutations use transactions, a monotonic revision, bounded text/list sizes, and
optimistic concurrency. Migrations are forward-only, tested from every released
schema, and must not make the immutable report unreadable if migration fails.

Saved filter definitions are global, not case-owned. They are stored in one
versioned, atomically written `global-filter-presets.json` beneath `config/` and are
available from every compatible report. Rules identify canonical fields or dynamic
evidence by stable source ID and field path, never by a report-specific column index.
A report may remember which global preset was active, but it never stores or mutates
a private filter definition. When a preset references fields absent from the open
report, the UI lists those rules as unavailable and requires an explicit choice to
apply only the remaining rules; it never silently weakens a filter.

### Stable identity

All review state uses the canonical `alleleId`, never a row number, transcript row,
display coordinate, or current sort position. Gene state uses approved HGNC symbols
plus stable HGNC/Ensembl identifiers when available. A display transcript can change
without changing allele review state.

### Triage is explainable, not diagnostic

Triage rules are deterministic queries over typed canonical fields and dynamic
evidence metadata. Every candidate stores human-readable and machine-readable
reasons, thresholds, source/version provenance, and the rule-set version. Automated
results are labelled **prioritization** or **provisional evidence**, never reviewed
clinical interpretation.

No prioritization runs automatically. AnnoCAT may ship versioned presets, but the
user chooses whether to run one, sees every rule and unavailable input beforehand,
and can change its thresholds. A global saved filter can also be used as the rule
set. Manual candidates have no computed priority: the user may leave priority unset
or assign High, Medium, or Low and provide a rationale. Filter/preset-added candidates
store the exact matched values as reasons; any displayed tier comes from a named,
visible rule and never from an opaque combined score.

## Delivery phases

### Phase 1 — Repair and harden variant-detail links

Do this first. The current `usefulVariantLinks` function in `web/src/app.js` builds
URLs in browser JavaScript from loosely validated display values and returns an
unstructured list. Replace it with backend-owned link descriptors returned by the
bounded allele-detail endpoint.

The response contract should be versioned and contain:

```json
{
  "id": "clinvar",
  "label": "ClinVar",
  "category": "variant",
  "url": "https://...",
  "identity": "VCV... or GRCh38 HGVS",
  "assembly": "GRCh38",
  "available": true,
  "unavailableReason": null
}
```

Generate a link only after validating and normalizing the required identifier:

- **Variant:** ClinVar Variation ID/VCV when present, otherwise an exact normalized
  GRCh38 HGVS/SPDI search; GeneBe normalized GRCh38 allele; dbSNP only for a valid
  `rs` identifier; gnomAD normalized allele; Ensembl and UCSC locus; and PubMed only
  for valid PMIDs present in source evidence.
- **Gene:** HGNC, NCBI Gene, Ensembl Gene, GeneCards, and Wikipedia search. OMIM is
  shown only when a corresponding MIM identifier is available; never infer one from
  a symbol.
- **Transcript/protein:** Ensembl Transcript/Protein and other identifier-specific
  destinations only when their identifiers are present and valid.

The drawer groups links under **Variant**, **Gene**, **Transcript**, and
**Publications**, displays the identifier and GRCh38 label, uses the common external
link icon, and offers a separate copy-identifier action. Unavailable links are
omitted by default; an optional explanatory row can state why an expected link is
unavailable. Do not display guessed direct Wikipedia pages or guessed database IDs.

Security and privacy requirements:

- generate URLs only for an explicit HTTPS host/path allowlist;
- percent-encode identifiers in the correct URL component;
- never accept an arbitrary URL from imported content or evidence values;
- keep `target="_blank"`, `rel="noopener noreferrer"`, and user-click-only navigation;
- do not prefetch links or send samples, phenotypes, genotypes, notes, or result lists;
- reject control characters, oversized identifiers, malformed coordinates, symbolic
  alleles where a destination cannot represent them, and unknown assemblies.

Tests cover SNVs, indels, chrX, chrY, MT, rsIDs, VCV/Variation IDs, absent symbols,
Ensembl IDs, special characters, structural alleles, malicious imported strings,
and accessible link rendering. Before implementing a destination, verify its current
official URL contract; treat a changed or undocumented route as unavailable rather
than inventing one.

Acceptance criteria:

- every rendered destination receives the intended canonical identifier;
- no link is assembled from raw imported HTML or an unvalidated arbitrary URL;
- a missing identifier produces no broken link;
- link unit tests do not require network access; and
- a small opt-in integration check can verify current destinations without making
  ordinary report viewing depend on the network.

### Phase 2 — Case Workspace shell and navigation

Refactor the open-report viewer without changing its bounded query behavior.

- Add Overview, Candidates, All variants, and Genes tabs inside the report.
- Open every report on All variants by default; do not persistently redirect ordinary
  report opening to Overview or Candidates based on the last visited tab.
- Keep report rename, case notes, share/export, and close actions in one report menu.
- Preserve table filters, loaded rows, selection, scroll position, and the open
  allele when switching views.
- Make report/view/allele state deep-linkable within the local app so stable links in
  notes can reopen a gene or allele.
- Use the same responsive layout and drawer at browser zoom levels and narrow widths.
- Put the same global saved-filter selector in Candidates and All variants. `Save`,
  `Update`, `Duplicate`, and `Delete` operate on the application-wide preset store;
  the current report stores only the active preset ID and unsaved working rules.

Acceptance criteria: local and imported reports use the same workspace; no tab loads
an entire Parquet table; browser refresh restores the open report and view; keyboard
and screen-reader navigation work throughout.

### Phase 3 — Persistent variant curation

Add the local case overlay and bounded APIs before adding automated prioritization.

Variant review fields:

- review state: `unreviewed`, `in_review`, `reviewed`;
- disposition: `candidate`, `uncertain`, `excluded`, or unset;
- reviewer interpretation: controlled values kept distinct from automated evidence;
- bookmark, tags, short rationale, private note, and updated time;
- optional exclusion reason from a controlled list plus free-text clarification.

The detail drawer's pinned review block supports autosave with visible saving/saved/
error state, explicit retry, keyboard operation, and revision-conflict recovery.
Candidates and All variants show compact review-state columns/chips sourced in
bounded batches, not one HTTP request per row.

API outline:

```text
GET/PATCH /api/runs/<run-id>/case-context
GET/PATCH /api/runs/<run-id>/reviews/<allele-id>
POST      /api/runs/<run-id>/reviews/batch-read
POST      /api/runs/<run-id>/reviews/batch-update
GET       /api/runs/<run-id>/review-summary
```

Every mutation uses the existing localhost origin/CSRF protections, validates the
run and allele IDs against the opened result, and enforces request/response bounds.

Acceptance criteria: edits survive restart; imported ZIPs and canonical files are
byte-identical; a failed write does not lose the previous revision; and review state
remains attached to an allele through filter, sort, transcript, and column changes.

### Phase 4 — Deterministic triage and Candidates

Implement a server-side rule engine over the existing DuckDB/Parquet query layer.
Running it is always an explicit user action. The prioritization dialog names the
preset, shows every rule/threshold and required source, identifies unavailable
evidence, and requires confirmation before querying the report. Users can choose a
shipped versioned preset or use one of their global saved filters as the rule set.
The first presets use data already available in AnnoCAT:

- ClinVar pathogenic/likely pathogenic and conflicting classifications;
- rare or absent in gnomAD exomes/genomes with configurable ancestry-aware AF;
- predicted loss-of-function and severe splice consequences;
- missense prioritization from REVEL, CADD, and retained dbNSFP fields;
- SpliceAI thresholds;
- phyloP conservation;
- QUAL/FILTER and missing-evidence warnings; and
- manual candidate inclusion or exclusion.

Presets are composable filters with explicit thresholds. Duplicate prediction scores
remain source/version-labelled; a preferred score may point to a source but never
discard the raw values. Candidate rows display concise reasons such as “rare in
gnomAD + canonical LoF” and allow the user to inspect each contributing value.

Users can also add one allele from its row/detail drawer or explicitly add all rows
matching the current global/working filter. Temporary checkbox selection remains an
export interaction and does not silently alter the persistent candidate shortlist.
Manual additions record `Added manually` plus the optional user rationale; filter
additions record the filter identity and matched field values.

Candidates are materialized as bounded `triage_result` rows so the queue remains
stable and reproducible even after settings change. Re-running creates a new triage
run and does not erase manual review state.

Acceptance criteria: identical report + rules + source versions produces identical
ranking/reasons; every reason links to its displayed evidence; users can add a
variant manually; and All variants always remains accessible.

### Phase 5 — Gene workspace and regional context

Build a bounded gene index from consequence Parquet plus the matching transcript
metadata. The Genes view shows variant/candidate counts, strongest consequence,
ClinVar evidence, review progress, and later phenotype rank.

Opening a gene shows:

- canonical/MANE transcript with an explicit alternate display-transcript selector;
- exon/protein consequence context and every case allele in the gene;
- candidate and reviewed variants first without hiding the rest;
- source/version-labelled evidence summaries; and
- nearby ClinVar variants from a new bounded regional query against the installed or
  report-contained ClinVar evidence, with distance and review status.

Do not duplicate the complete external-link list here; use the same validated link
descriptor service for gene/transcript destinations.

Acceptance criteria: gene counts agree with allele/consequence queries; switching
the display transcript does not alter review identity; nearby ClinVar is bounded and
clearly distinguishes case variants from reference-database variants.

### Phase 6 — Case context and inheritance

Add a compact case-context editor accessible from the workspace header/Overview:

- phenotype terms and free-text summary;
- candidate gene list with gene.iobio-compatible import/export;
- sample-to-role mapping, affected status, sex, and proband;
- optional PED import; and
- case-level notes and warnings.

Use retained VCF FORMAT/sample values to derive clearly labelled inheritance hints:
de novo, dominant, homozygous recessive, possible compound heterozygous, X-linked,
and unknown. Parse GT/GQ/DP/AD with explicit missing/invalid states. Compound
heterozygosity remains **possible** unless phase or family evidence supports it.

Acceptance criteria: trio and single-sample fixtures cover phased, unphased,
multiallelic, missing, low-quality, and sex-chromosome cases; no inheritance claim is
made without its required sample roles and genotype evidence.

### Phase 7 — Offline phenotype prioritization

Add small, versioned source packages/adapters for the HPO ontology and documented
phenotype-to-gene associations. Evaluate current licensing and official distribution
URLs when implementing; do not make a live third-party phenotype request the default.

The UI normalizes terms, exposes ambiguity before applying them, stores original and
normalized terms, and ranks only genes represented in the current result. Ranking
shows score components, source/model versions, and coverage limitations. It feeds
the Candidates and Genes views but does not automatically hide variants.

Acceptance criteria: ranking is offline, deterministic, versioned, and explicitly
labelled research prioritization; unknown/ambiguous terms require resolution; no
phenotype or case data leaves the machine without a separate explicit future opt-in.

### Phase 8 — Curation-aware export and sharing

Keep format export separate from report sharing. Add selected/filtered variant and
gene exports using visible selected columns, then allow the user to include a
portable curation snapshot in a shared report.

- Case notes, phenotype, pedigree, sample roles, and review data are excluded by
  default.
- Sharing any of them requires an itemized privacy confirmation.
- Portable curation data uses versioned, checksummed JSON/TSV entries rather than
  sharing the local SQLite database.
- The manifest records the curation schema, included fields, rule/source versions,
  and whether genotype/sample data is present.
- Import creates a new local overlay from the portable snapshot only after validation;
  it never gives imported data authority to write outside that report.

Acceptance criteria: default shared ZIP contains no local notes or phenotype data;
included curation round-trips with stable allele IDs; older AnnoCAT versions can
still identify unsupported optional curation entries without corrupting the report.

### Phase 9 — Optional read-level evidence (post-core)

Add local BAM/CRAM access only after the VCF/Parquet curation workflow is complete.
It requires an index, reference compatibility checks, bounded regional reads, and a
read-only local pileup/coverage component. It is not a downloaded WGS annotation
source and does not block the first curation release.

## Cross-cutting work

### Dynamic schemas and future sources

Filters, columns, evidence summaries, and triage rule inputs consume canonical field
metadata rather than a hard-coded list of every database field. Unknown fields are
displayable as bounded text by default and become numeric/categorical controls only
when validated metadata declares that type. A new source cannot inject HTML, SQL,
URLs, CSS classes, or executable expressions.

### Performance budgets

- Browser requests remain paginated/bounded and cancellable.
- Review overlays for visible rows are fetched in one batch.
- Counts and summaries run server-side with a time/memory/thread budget.
- Triage jobs expose progress and cancellation in Logs and atomically publish only a
  complete result set.
- Benchmarks progress from the existing smoke/500-row fixtures to 10k, chromosome 22,
  and full HG002 before release claims.

### Privacy and security

- Treat notes, phenotype, pedigree, sample identities, and genotypes as sensitive.
- Keep all curation local by default and show what a share/export operation includes.
- Escape imported/source content; bind all DuckDB/SQLite values as parameters.
- Apply existing report ZIP validation before parsing optional curation entries.
- Never create outbound links from arbitrary imported URLs or automatically contact
  an external interpretation site.

### Testing layers

1. Rust unit tests for identity validation, links, migrations, review state, rules,
   and inheritance.
2. API tests for bounds, authorization/origin, invalid run/allele IDs, conflicts,
   cancellation, and atomic failure recovery.
3. Browser tests for workspace navigation, filtering, drawer/review interaction,
   link accessibility, selection, refresh restoration, zoom, and keyboard use.
4. Golden fixtures for triage reasons, phenotype ranks, gene aggregation, and shared
   curation packages.
5. Scale and recovery tests on supported Windows 10/11 ordinary-user installations.

## Recommended implementation order

The critical path is:

```text
validated links
  -> workspace shell
  -> local case overlay and variant review
  -> deterministic candidate queue
  -> gene workspace and regional ClinVar
  -> sample roles and inheritance
  -> offline phenotype ranking
  -> curation-aware sharing
  -> optional BAM/CRAM evidence
```

The first releasable curation milestone ends after Phase 4: users can open a report,
review every variant, build an explainable candidate list from existing AnnoCAT
sources, retain their work locally, and export the selection. Phases 5–8 expand that
workflow toward gene.iobio-style breadth without making the initial design depend on
new large data sources or alignment files.
