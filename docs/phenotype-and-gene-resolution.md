# Phenotype, condition, pathway, and gene resolution

This document defines how AnnoCAT turns Human Phenotype Ontology (HPO)
features, Mondo Disease Ontology (MONDO) conditions, Reactome pathways, and
gene identifiers into a gene list and supporting evidence. It is the
maintained contract for the Genes popover, result filtering, and the **Gene
matches** result column. Gene-profile evidence is not displayed as a separate
section in Variant Details.

AnnoCAT uses these selections to organize and filter a result. A match is not
a diagnosis, a pathogenicity classification, or proof that a variant causes a
phenotype.

## Document status

This is a specification, defect record, and implementation record, with status
updated on 2026-08-25. **Required** describes the target contract. The local
implementation status below does not mean the behavior has been published in a
release.

| Scope inspected on 2026-08-25 | Exact identity | Conformance to this contract |
|---|---|---|
| Public Windows release | GitHub release [`v0.1.0`](https://github.com/annocat-project/AnnoCAT/releases/tag/v0.1.0), release record published 2026-08-20; tag commit `795b5161cda0b814f27b886dc17f242546256bbe`; asset `AnnoCat-0.1.0-windows-x86_64.zip`, uploaded 2026-08-23, SHA-256 `fb27c464e07d6ec59152079dfc4b7fe80d0fbf116eaefd31d73821d6ca9c327a` | Uses the defective `hpo-lin-query-v4` behavior and does not conform to the corrected contract |
| Local tracked baseline | Commit `40582038aa95eca0446a4eddc9b21f18862261c1` | Contains the zero-overlap Apply guard and this specification, but not the remaining corrected contract |
| Local working tree | The tracked baseline plus the uncommitted implementation inspected and tested on 2026-08-25 | Implements the functional, schema, scientific-method, and existing-UI contract described here; it is not committed, pushed, or published |
| Target corrected evidence | Profile schema `6`, catalog schema `2`, evidence contract `gene-profile-evidence-v2` | Implemented in the local working tree; it must not be described as published until a release commit, workflow result, asset, and digest are recorded here |

Local verification on 2026-08-25 used the pinned source manifests and produced
the following evidence:

| Check | Local result |
|---|---|
| Full Rust workspace, including the standalone report worker and AppContainer tests | 381 passed, 5 intentionally ignored, 0 failed |
| Browser-module suite | 36 passed, 0 failed |
| Pinned HPO known-case gate | SCN1A/OMIM:607208 rank 1 and CACNA1A/OMIM:108500 rank 1; each tie group ends at 1 in a 4,805-gene denominator |
| Pinned HPO association sanity queries | Seizure 1,575; Short stature 987; Atrial septal defect 355; all distinct and non-universal |
| Isolated local UI smoke test | Existing Genes popover retained; entered HNF1A resolved to 1 of 1 result gene; **Gene matches** displayed `HNF1A`, no `true` value or **Gene associations** section appeared, and the compact tooltip worked by keyboard and Escape |
| Static and repository checks | Rust formatting, edited JavaScript syntax, and `git diff --check` passed; the final UI detector reported only two pre-existing width-transition warnings outside the changed CSS block |

These are local results, not GitHub Actions or release evidence. The workflows
are changed to run the pinned HPO gate, but that remote execution cannot occur
until the implementation is committed and pushed.

The public defect counts below are observations from that exact `v0.1.0`
release with the stated installed data, not timeless properties of AnnoCAT.
Release acceptance must use checked-in fixtures and recorded source hashes
rather than relying on prose counts alone.

The defective build has a composite phenotype algorithm value
`hpo-lin-query-v4`. Corrected evidence separates gene membership from semantic
ranking with `geneSetAlgorithmVersion: "hpo-association-query-v5"` and
`phenotypeRankingAlgorithmVersion: "resnik-query-disease-v1"`. Semantic
similarity never determines gene membership.

The required behavior was reviewed on 2026-08-25 against the official HPO,
MONDO, Reactome, and HGNC documentation; the Phenomizer, LIRICAL, and semantic-
similarity literature; and the documented presentation patterns of VarSome
Clinical, VarSeq, Illumina Emedgene, and Fabric GEM.
Commercial products are UI and workflow comparisons, not scientific validation
of AnnoCAT's algorithm.

UI concordance was reviewed on 2026-08-25 against the supplied Genes-popover
screenshot, a locally launched current build, `web/src/app/phenotypes.js`,
`web/src/app.js`, and `web/src/fluent-components.css`. The corrected contract
reuses the existing Genes popover, its inline scope/message area, the existing
absent-gene inspection dialog, the Results **Columns** menu, and compact result
cells. The only new visible field is the **Phenotype rank** result column, with
the checkbox and tooltip that the existing dynamic-column system generates for
it. It does not require a new panel, mode selector, dedicated ranking control,
or Variant Details replacement section.

## Product purpose and terminology

AnnoCAT's Genes popover is primarily a search and result-filtering tool. It has
one unified **Search names or identifiers** box rather than separate feature,
condition, pathway, and gene modes. Each autocomplete result identifies its
type; selecting that result is the only way the user chooses the operation. A
user may search one HPO feature, add more features incrementally, select a
condition, pathway, or gene, or use a mixture of those items. Positive HPO
features selected in one Genes search are interpreted as findings from one
patient, but the search does not require the user's complete phenotype profile.
The current implementation persists that search for the result/run and does not
bind it to an individual sample when a result contains multiple samples; the UI
must not claim sample-specific application unless that binding is added.

`profile` is the existing persistence and API name for a saved Genes search. It
does not mean a complete clinical intake record. Likewise, the request field
`observed` contains the patient's positive HPO features currently selected for
the search. Schema 5 also contains `excluded`, but the current Genes popover
has no control for adding an explicitly absent feature. It is legacy state,
not a user-facing search option, and it does not participate in the
corrected gene-membership or candidate-ranking rules. AnnoCAT may therefore
search a partial or comprehensive positive patient feature set, but it is not a
patient-phenotyping or clinical-intake system.

To avoid ambiguity, this document calls that internal object the **saved Genes
query record** except when it names the actual `PhenotypeProfile` code/API type
or a version field. A **disease profile** is different: it is the set of HPO
annotations for one disease identifier used by the ranking algorithm. Neither
term describes a separate patient-profile screen or upload workflow.

Multiple positive HPO selections retain the fixed union search behavior defined
in this document even though they belong to the same patient. This supports
incremental searching and avoids requiring a complete profile or a strict
intersection. There is no association-versus-ranking mode and no **Match any**
or **Match every** choice in the Genes popover. Selecting multiple terms always
previews the deterministic union of their association-derived gene sets.

### Genes popover interaction contract

The visible layout remains the current one: the heading **Add a feature,
condition, pathway, or gene**; one **Search names or identifiers** input;
selected-item chips; the gene-only textarea; the saved-list selector with
**Use list**, **Save list**, and **Delete**; the existing inline scope/message
area; and the footer actions **Clear** and **Apply**. The visible controls have
the following fixed meanings:

| User action | Meaning |
|---|---|
| Choose a result labeled **Feature** | Add that positive HPO feature to the patient phenotype query |
| Choose a result labeled **Condition** | Add that MONDO condition and use the documented condition-to-gene expansion |
| Choose a result labeled **Pathway** | Add that Reactome pathway and use its installed human gene set |
| Choose a result labeled **Gene** | Add the resolved gene directly |
| Paste genes in the textarea | Resolve gene identities only and switch to the manual-list workflow |
| Choose **Use list** | Load the selected saved list as manual genes; do not restore ontology selections |
| Choose **Save list** | Save the currently resolved genes as a manual gene list, not as a saved HPO/MONDO/Reactome query |
| Choose **Delete** | Delete only the selected saved manual gene list |
| Choose **Clear** | Clear the current Genes query and active result filter without deleting saved manual gene lists |
| Choose **Apply** | When the current preview has at least one resolved gene present in the result, apply the fixed union gene set and show those rows |

Autocomplete result labels and canonical identifiers make the chosen entity
type visible. Selected items appear as labeled chips. The popover must not gain
extra controls for association mode, semantic-similarity method, **any**,
**every**, or a complete-patient-profile mode.

A gene chosen from autocomplete can coexist with feature, condition, and
pathway chips. Selecting an HPO feature, MONDO condition, or Reactome pathway
starts a typed preview and may populate the textarea with the genes resolved
from that selection. Those generated lines are a preview of the resulting gene
set; they do not mean that the textarea accepts HPO, MONDO, or Reactome IDs.
Typing in the **Or paste genes here** area or using a saved list instead switches
the popover to its gene-only manual-list workflow and replaces the ontology
selections. The specification does not imply that pasted lists and ontology
chips can be combined in the present UI. **Save list** stores only the resolved
gene identities currently shown by this workflow. If those genes were generated
from HPO, MONDO, or Reactome selections, loading the saved list later restores
the genes, not the source chips, rank query, or ontology provenance.

Phenotype rank is result presentation, not another Genes search option. Its
new structured field is `phenotypeRank`; the old `phenotypeRelevance` score is
not reinterpreted. If the Resnik implementation passes validation and is
enabled in a future corrected build, the user first applies positive HPO
selections normally. With one positive HPO feature, **Phenotype rank** is
available through the existing Results **Columns** menu but is initially
unchecked when the user has no saved column choice. With two or more positive
HPO features, it is initially checked and visible beside **Gene matches** when
the user has no saved choice. No recommendation message, badge, or ranking
selector is added to the Genes popover.

**Gene matches** remains a recommended column in every applied Genes search
because it explains why a result gene matched an HPO feature, MONDO condition,
Reactome pathway, or entered gene. It is not forcibly re-enabled when a user
has hidden it in **Columns**. **Phenotype rank** does not replace it: rank
supports HPO prioritization, while Gene matches explains deterministic
inclusion. A query with no positive HPO features does not offer or display
Phenotype rank.

## Confirmed defects

### Every HPO feature returns 4,804 genes

The defect was reproduced against public release `v0.1.0` with HPO release
2026-06-23:

| Selected feature | Preview gene count | Genes present in the inspected result |
|---|---:|---:|
| `HP:0001250` Seizure | 4,804 | 4,753 |
| `HP:0004322` Short stature | 4,804 | 4,753 |
| `HP:0001631` Atrial septal defect | 4,804 | 4,753 |

The requests produced different fingerprints, proving that the selected HPO
identifiers reached the server and that this is not a stale browser response.
The installed source data also differs by feature. For the same release, 1,276,
897, and 319 Mendelian genes, respectively, are linked through diseases
annotated exactly to the three terms. These are disease-mediated, exact-term
counts, not direct gene annotations and not expected semantic-expansion counts.

The original reproduction did not preserve a shareable result fixture or the
installed source-file hashes. These numbers are therefore a defect observation,
not a release acceptance fixture and not a promised corrected count. The
regression suite must reproduce the failure shape and corrected non-universal
behavior from checked-in synthetic/public fixtures with recorded hashes.

The root cause is a mismatch between ranking and gene inclusion:

1. `rank_disease` creates one `PhenotypeMatch` for every observed query and
   every disease profile, including pairs whose Lin similarity is exactly zero.
2. `write_gene_evidence` treats any nonempty `matched_phenotypes` collection as
   a feature link.
3. It records the selected HPO identifier in `selected_matches` for those zero
   matches.
4. The `any` inclusion rule sees at least one selected match and includes the
   gene.

Whenever at least one observed feature is selected, every disease receives a
nonempty match-object collection. Many of those objects have a zero score, but
the inclusion code tests only whether the collection is nonempty. Consequently,
nearly the complete eligible HPO Mendelian gene universe is included for every
selected feature.

### Variant Details leaks internal gene-match fields

The generated allele evidence writes a `geneMatch` boolean with value `true`
whenever an allele has match details. That value is redundant: the presence of
`geneMatches` and `geneMatchDetails` already proves the match.

The field catalog currently marks `geneMatch`, `matchedSelectedItems`, and
`matchedItemTypes` as selectable. The Variant Details renderer hides only the
two JSON detail fields and sends the remaining phenotype-domain fields through
the generic evidence renderer. Consequently, **Gene associations** can display
repeated rows such as `Gene match: true`, along with other implementation
fields. The section can also appear when only support fields are available.
The information duplicates the **Gene matches** result column and does not
justify a separate Variant Details section.

### Gene matches over-counts one selected item on multi-gene alleles

The current allele aggregation keeps separate rows for each matched gene and
selected item. The compact formatter then counts those rows as if they were
different user selections. For example, if one allele is linked to two genes
that are both in one selected Reactome pathway, the cell can show
`Example pathway +1` even though the user selected only one pathway.

The compact count must be deduplicated by selected item identity
`(itemType, selectedId)`. The structured evidence must still retain one row per
`(allele, gene, selected item)` so no gene-level provenance is lost. In the
example above the cell says only `Example pathway`; its tooltip may name both
matched genes.

### Zero-overlap Apply is accepted as a no-op

The public `v0.1.0` browser enables **Apply** when it has positive input,
a current preview fingerprint, and no unresolved paste entries. It calculates
`includedGenesInResult` for the scope message but does not require that value to
be greater than zero. The published server then accepts a zero-overlap apply,
publishes an active generation, and silently changes `showMatchesOnly` to
`false`. The result therefore remains unfiltered even though the user chose
**Apply**.

This is not the same as waiting for the textarea to be populated. A typed HPO,
MONDO, or Reactome selection starts an asynchronous gene preview, and that
preview normally also writes its resolved genes into the textarea. The preview
fingerprint, not the presence of textarea text itself, is the current enablement
condition. The corrected behavior must explicitly require at least one resolved
gene in the current result.

### Paste resolution loses HPO, MONDO, and Reactome types

The current `POST /api/phenotypes/terms` implementation first tries gene
resolution and then also searches HPO, MONDO, and Reactome for exact matches.
The browser maps every recognized response to `{id, label}`, discards its item
type, clears the typed ontology selections, and submits the values as manual
genes. An exact `HP:*`, `MONDO:*`, or `R-HSA-*` entry can therefore be recognized
by the paste endpoint and then fail the gene preview because it is not a gene.

The textarea is visibly labeled for genes, so this extra ontology resolution is
both unnecessary and unsafe. HPO, MONDO, and Reactome identifiers must be
selected through the unified autocomplete, which retains their type. The paste
and saved-list workflow must recognize genes only.

### Portable phenotype writer and importer require different schemas

The current phenotype writer uses `PROFILE_SCHEMA_VERSION = 5` and packages
that saved Genes query as `phenotypes.json`. The portable-result preflight
validator in `report_import.rs` instead requires `schemaVersion == 4` for the
nested phenotype group. A result exported with current phenotype evidence can
therefore be rejected during import even though the base result is otherwise
valid.

The corrected writer, phenotype validator, and portable-result preflight
validator must agree on schema 6. A valid archive containing an older nested
Genes query opens without that query or filter, as defined below. This fallback
does not waive archive security: path, size, declared-file, and checksum
failures still reject a corrupt or tampered result.

## Installed knowledge and provenance

Resolution uses versioned local resources installed from **Data sources**:

- HPO ontology terms from `hp.obo`;
- HPO disease annotations from
  [`phenotype.hpoa`](https://obophenotype.github.io/human-phenotype-ontology/annotations/phenotype_hpoa/);
- HPO disease-gene associations from
  [`genes_to_disease.txt`](https://obophenotype.github.io/human-phenotype-ontology/annotations/genes_to_disease/);
- MONDO terms and relationships from `mondo.json`;
- human Reactome pathway gene sets; and
- approved and withdrawn HGNC identities from `hgnc_complete_set.txt` and
  `withdrawn.txt`.

The generated gene-evidence catalog records the release label and SHA-256 of
every source asset actually read: `hp.obo`, `phenotype.hpoa`,
`genes_to_disease.txt`, `mondo.json`, the installed human Reactome gene-set
file, `hgnc_complete_set.txt`, and `withdrawn.txt`. A release label alone is
not sufficient because a file can be republished without a new label.

The preview and evidence fingerprint includes the normalized selections,
profile schema, catalog schema, evidence and identity contract versions, fixed
union policy, both algorithm versions, and every source-asset SHA-256. The
catalog and structured export retain the same values so a later run can prove
which inputs produced the evidence.

Search and expansion are local after installation. Schema 6 and the corrected
preview/ranking response have no Monarch suggestion action, request flag,
online-enrichment field, stored suggestion/error fields, service-catalog
entry, or Monarch network call.

### Versioned schema and fingerprint contract

The corrected contract uses independent versions because changing a JSON shape
is different from changing a biological algorithm:

| Contract component | Corrected value | Meaning |
|---|---|---|
| Profile `schemaVersion` | `6` | Shape and source-revalidation rules for the saved Genes query; the defective build's profile schema 5 is unsupported |
| Gene-evidence catalog `schemaVersion` | `2` | Shape and visibility rules for catalog fields |
| `evidenceContractVersion` | `gene-profile-evidence-v2` | Meaning and structure of generated gene and allele evidence |
| `identityContractVersion` | `hgnc-identity-v2` | Canonical HGNC identity plus result-specific identity |
| `geneSetAlgorithmVersion` | `hpo-association-query-v5` | HPO, MONDO, Reactome, and entered-gene membership rules |
| `phenotypeRankingAlgorithmVersion` | `resnik-query-disease-v1` | Resnik corpus, score, aggregation, tie, and rank rules |

The legacy composite `algorithmVersion` may be read from an imported evidence
catalog only to identify old audit data. New previews, catalogs, and exports
write the two specific algorithm fields. Profile schema 6 is the only supported
and written saved Genes query format. All earlier profile schemas are
unsupported and can never restore an active Genes filter.

Catalog schema 2 contains `schemaVersion`, `fingerprint`,
`evidenceContractVersion`, `identityContractVersion`,
`geneSetAlgorithmVersion`, `phenotypeRankingAlgorithmVersion`, `sourceAssets`,
and `fields`. Each `sourceAssets` entry records a stable asset name, release
label, and lowercase 64-character SHA-256. Entries are ordered by asset name for
deterministic export. The catalog may record generation time, but timestamps do
not enter the evidence fingerprint.

## Unified search

The Genes popover searches features, conditions, pathways, and genes through
`GET /api/phenotypes/terms`. Queries shorter than two normalized search units
return no matches. Results contain an identifier, canonical label, item type,
match kind, and the text that matched.

When a result is open, gene search also uses identities present in that result.
Exact identifiers rank first, followed by exact gene symbols, exact labels,
label prefixes, and other substring matches. Duplicate identifiers of the same
item type are removed.

`POST /api/phenotypes/terms` is the exact, gene-only resolver for pasted
entries. Saved gene lists use the same gene-only identity contract. A current
or uniquely resolvable historical gene symbol or identifier returns the
approved gene identity. The response separates recognized, ambiguous, and
unrecognized values; partial search hits are not silently accepted. An HPO,
MONDO, or Reactome identifier or label is unrecognized in this workflow even
when it is a valid ontology item. Those items must be selected through the
unified `GET` search so their type is retained.

### HPO feature search

HPO search includes active descendants of `HP:0000118` (Phenotypic
abnormality), excluding the root itself. It matches:

1. an exact canonical HPO identifier;
2. an exact label;
3. a label prefix;
4. an exact synonym;
5. a label substring; and
6. a synonym substring.

The selected identifier is authoritative. Before use, obsolete terms are
replaced only when the installed ontology supplies a single replacement. The
stored label is replaced with the installed canonical label.

Observed features keep the most specific selected term when an ancestor and
descendant are both present. Explicitly absent features keep the most general
term. An observed term cannot also fall under an explicitly absent ancestor.

### MONDO condition search

MONDO search is limited to active human conditions. It matches:

1. an exact MONDO identifier;
2. an exact supported external identifier, including OMIM, OMIM phenotypic
   series, Orphanet, and DECIPHER identifiers;
3. an exact label or synonym;
4. a label prefix or substring; and
5. a synonym or indexed-text substring.

Search results report the number of active descendants. Deprecated MONDO terms
are replaced only when a single replacement is available. Selection fails
instead of silently choosing when a term is unavailable, non-human, or has no
unique replacement.

### Reactome pathway search

Reactome search includes installed human pathways. It matches an exact
`R-HSA-*` identifier, exact label, label prefix, or label substring. Results
include the number of unique genes in the pathway.

Applying a pathway re-resolves its identifier against the installed release;
the submitted label and gene list are not trusted. Every unique gene symbol
listed for that pathway in the installed Reactome Pathways Gene Set is eligible
for the resolved gene list. This relation means membership in that release's
gene set; it does not by itself establish a direct molecular interaction or a
disease association.

### HGNC gene search and resolution

Gene resolution prefers the installed HGNC identity bundle and supplements it
with transcript-cache and current-result identities. Matching is
case-insensitive and supports:

- approved HGNC symbols;
- HGNC identifiers;
- numeric NCBI/Entrez Gene identifiers as stored by HGNC;
- Ensembl gene identifiers, with a numeric version suffix removed;
- unique HGNC alias and previous symbols; and
- unique withdrawn symbols that HGNC maps to an approved record.

Resolution returns the approved symbol and a best-available runtime or HGNC
Ensembl mapping when one is uniquely available. The current resolver falls
back to the approved symbol when no such identifier is available; it does not
return the stable HGNC identifier as its result ID. A historical symbol or
identifier that maps to more than one approved gene is **ambiguous** and is
never chosen silently. Unknown values remain unresolved. Exact pasted-list
resolution reports recognized, ambiguous, and unrecognized entries separately.

The current result contributes runtime identities so a gene present in the
result remains usable when the installed HGNC bundle lacks that exact key.
Runtime fallback also requires a unique mapping.

The stable canonical identity is the HGNC identifier when one is available.
The approved HGNC symbol is the user-facing label, while a result's Ensembl or
other coordinate-bound identifier remains attached as the result identifier.
Evidence and exports retain both identities so a later symbol change does not
change the identity of an existing match. A symbol-only fallback is permitted
only when no stable HGNC mapping is available and is marked as such in
provenance.

A corrected gene search, saved selection, preview, and evidence row use the
same explicit identity shape:

```json
{
  "symbol": "SCN1A",
  "canonicalGeneId": "HGNC:10585",
  "resultGeneId": "ENSG00000144285",
  "identityStatus": "hgnc"
}
```

`canonicalGeneId` is the comparison and deduplication key whenever it is
available. `resultGeneId` identifies the gene representation attached to the
current result and is `null` when no result-specific identity exists. For the
permitted last-resort symbol-only case, `canonicalGeneId` is `null` and
`identityStatus` is `symbol-only`; comparison then requires the exact normalized
approved symbol. Ambiguous and unresolved values have no resolved-gene object
and cannot be previewed or applied.

### Search result contract

| Item type | Resolved identifier | Search metadata | Selection revalidation |
|---|---|---|---|
| HPO feature | `HP:#######` | Label, matching synonym, match kind | Active phenotypic-abnormality term or unique obsolete replacement |
| MONDO condition | `MONDO:#######` | Label, synonym scope, external-ID match, descendant count | Active human condition or unique deprecated replacement |
| Reactome pathway | `R-HSA-*` | Label, match kind, pathway gene count | Exact pathway ID in the installed human release |
| Gene | `symbol`, `canonicalGeneId`, `resultGeneId`, and `identityStatus` | Matched symbol or identifier and match kind | Unique HGNC or runtime identity |

Search labels are presentation data. The identifier is re-resolved before gene
expansion so a client cannot submit a fabricated label or pathway membership.

## Saved Genes query (`PhenotypeProfile`) request and state contract

AnnoCAT creates this record automatically for the Genes feature and keeps it
with the result/run. The user does not choose a schema and does not load a
separate profile file. AnnoCAT reads the record when that result is reopened or
imported. Schema 6 is the only supported format.

Profile schema 6 stores HPO terms, MONDO conditions, and Reactome pathways as
typed `{id, label}` selections. Its `genes` entries use the resolved-gene shape
above rather than the generic ontology-term shape. The saved Genes query can
store observed HPO terms, MONDO conditions, Reactome pathways, and entered
genes. Schema 6 does not contain `excluded` or `excludedGenes`. Applying a saved
Genes query requires the fingerprint returned by the latest preview; the
server rejects an apply request
when releases, source hashes, identities, selections, schemas, contracts, or
either algorithm changed after preview.

The browser has no **Match any** or **Match every** control. Schema 6 has no
`combination` field. Selected feature, condition, pathway, and entered-gene sets
are always combined by the fixed union rule. A schema-6 preview or apply request
containing `combination`, `excluded`, or `excludedGenes` is rejected; an API
caller must not receive silently changed semantics.

The schema-6 top-level record contains exactly `schemaVersion`, `runId`,
`updatedAt`, `observed`, `conditions`, `pathways`, `genes`, `showMatchesOnly`,
and `activeGeneration`. For example, an applied HPO query is stored as:

```json
{
  "schemaVersion": 6,
  "runId": "example-run",
  "updatedAt": "2026-08-25T18:00:00Z",
  "observed": [{"id": "HP:0001250", "label": "Seizure"}],
  "conditions": [],
  "pathways": [],
  "genes": [],
  "showMatchesOnly": true,
  "activeGeneration": {
    "fingerprint": "sha256-value",
    "evidenceFile": "gene-profile-evidence.parquet",
    "catalogFile": "gene-profile-catalog.json",
    "matchedGeneCount": 24
  }
}
```

`activeGeneration` is `null` when no corrected query is applied.
`showMatchesOnly` must be `true` whenever `activeGeneration` is present. Schema
6 has no `ranking`, `limitToLinkedGenes`, negative-feature, gene-exclusion, or
Monarch fields; corrected rank data is stored in catalog-2 evidence instead.

`showMatchesOnly` controls the result filter after a saved Genes query is
applied. The corrected browser may enable **Apply** only when the latest preview
has a current fingerprint, has no unresolved or ambiguous pasted genes, and reports
`includedGenesInResult > 0`. It always submits `showMatchesOnly: true`. The
server must enforce the same positive-overlap precondition and must not accept a
zero-overlap apply by silently changing `showMatchesOnly` to `false`.

`limitToLinkedGenes` is a schema-5 legacy field and is not part of schema 6.
Schema 6 has one documented membership rule: the union defined below.

Schema 6 does not contain `requestMonarchSuggestions`, `monarchSuggestions`,
or `monarchError`. Corrected preview and ranking responses do not contain
`onlineEnrichment` or `onlineError`. The corrected browser and server do not
offer or call a Monarch suggestion endpoint, and the source catalog contains
no Monarch semantic-similarity service.

## HPO feature-to-gene expansion

Gene membership is derived from documented HPO disease annotations and the
ontology's true-path rule, not from a semantic-similarity threshold:

1. For each selected positive HPO feature, inspect positive disease annotations
   in `phenotype.hpoa`.
2. Match an annotation when its HPO term is the selected term (**HPO link via
   exact disease annotation**) or a descendant of the selected term (**HPO link
   via more-specific disease annotation**).
3. Join each matched disease to its eligible `MENDELIAN` entries in
   `genes_to_disease.txt`.
4. Retain the selected term, annotated term, disease, gene, association type,
   source, and the HPO annotation reference, evidence code, frequency, and
   biocuration metadata when present.
5. Union the per-feature gene sets with the selected MONDO, Reactome, and
   entered-gene sets.

This follows the same ancestor-propagation direction documented for HPO's
[`phenotype_to_genes.txt`](https://obophenotype.github.io/human-phenotype-ontology/annotations/phenotype_to_genes/),
which includes genes inherited from descendant annotations. AnnoCAT reconstructs
the relation from its installed disease annotations and disease-gene file so it
can retain disease and association provenance and restrict the join to eligible
`MENDELIAN` associations. The HPO summary file is therefore a directional
reference and sanity check, not a promise of row-for-row equality. A broader
disease annotation, a sibling term, or another merely similar term may
contribute to ranking but must not create gene membership.

Explicitly absent features may remain in unsupported old audit evidence, but
they are not migrated into schema 6. They do not create or exclude genes and do
not participate in ranking. Disease-reported negative phenotypes also remain
separate from positive associations.

### Phenotype ranking method

AnnoCAT uses Resnik similarity with an asymmetric selected-query-to-disease
best-match average for phenotype ranking. This is a pragmatic, versioned
baseline choice, not a claim that Resnik is universally the best semantic-
similarity method. Term-level Resnik similarity and asymmetric
query-to-disease best-match averaging are established HPO-analysis techniques,
including in the original
[`Phenomizer`](https://pmc.ncbi.nlm.nih.gov/articles/PMC2756558/), an early
[HPO gene-prioritization study](https://pmc.ncbi.nlm.nih.gov/articles/PMC4117966/),
and a more recent [phenotype-similarity evaluation preprint](https://pmc.ncbi.nlm.nih.gov/articles/PMC12667856/).
AnnoCAT's eligible corpus, best-disease-per-gene aggregation, and fixed global
rank are product-specific, versioned heuristic choices; they are not an HPO
standard or a clinically validated diagnostic model. The alternatives in the
[GOSemSim semantic-similarity reference](https://yulab-smu.top/biomedical-knowledge-mining-book/01-semantic-similarity.html)
are out of scope for `resnik-query-disease-v1`.

The Resnik implementation contract is:

1. Build exactly one disease profile for each distinct `database_id` in the
   installed `phenotype.hpoa`, after trimming surrounding whitespace. Merge all
   rows with that exact source identifier. Do not merge or deduplicate profiles
   merely because their identifiers map to the same MONDO term.
2. A profile is eligible only when it has at least one positive `P`
   (`Phenotypic abnormality`) annotation and at least one `MENDELIAN`
   association from `genes_to_disease.txt` that resolves to a unique gene under
   `hgnc-identity-v2`. Let `D` be the number of these eligible profiles, counted
   once each.
3. For HPO term `t`, let `Dt` be the number of eligible profiles annotated
   positively to `t` or any descendant of `t`, counted at most once per profile
   under the true-path rule. Calculate `IC(t) = -ln(Dt / D)` with the natural
   logarithm.
4. A term with `Dt = 0` has no usable IC and is never assigned infinity. It
   cannot be the most informative common ancestor. A selected term with no
   direct corpus support may still match through a supported ancestor; when no
   common ancestor with `Dt > 0` exists, its term-pair similarity is `0`.
5. The Resnik similarity of two terms is the greatest usable IC among their
   common ancestors. For each positive selected query term, take its greatest
   similarity to any positive term in the disease profile, then average those
   values over the selected query terms. This is the asymmetric
   query-to-disease best-match average.
6. For a gene associated with multiple eligible profiles, use its highest
   disease-profile score as the gene score and retain that exact source disease
   identifier and label as the explanation.
7. Canonicalize disease-gene associations before constructing the ranking
   universe. Count one gene once by `canonicalGeneId`. A unique symbol-only
   fallback is counted once by its normalized approved symbol and marked
   `symbol-only`; ambiguous or unresolved associations are excluded from the
   ranking universe and its denominator.
8. Rank the complete eligible HPO Mendelian gene universe. The denominator does
   not change when result rows are filtered or when the query also contains
   MONDO conditions, Reactome pathways, or entered genes.
9. Sort selected query terms, disease terms, disease identifiers, and canonical
   gene identities lexically by their canonical IDs before evaluation. Sum each
   query term's best score sequentially in query-ID order; do not use a
   nondeterministic parallel floating-point reduction.
10. Persist the raw `f64` score, but compare and tie scores using
    `scoreKey = floor(rawScore * 10^12 + 0.5)`. A gene's competition rank is one
    plus the number of genes with a greater score key. Equal score keys receive
    the same rank, and tie count is the total number of genes with that key.
    Order tied genes by approved symbol and then canonical identity only to make
    display and exports deterministic; that order does not imply a biological
    difference.

The query-to-disease direction is intentional for AnnoCAT's partial-search
workflow: it does not penalize a disease for features the user has not entered.
Known limitations are retained in provenance: extensively annotated diseases
have more opportunities to match, and genes associated with many diseases have
more opportunities to receive a high best-disease score.

Only positive HPO search selections participate in the Resnik calculation.
Explicitly absent features in unsupported old evidence do not receive the
current ad hoc conflict score and are not folded into the displayed rank. If
frequency- and negative-finding-aware diagnostic probabilities are later
required, a method such as
[`LIRICAL`](https://pmc.ncbi.nlm.nih.gov/articles/PMC7477017/) requires a
separate clinical design and validation effort.

The current 0-100 Lin value must not be carried into corrected evidence or
shown to users: it is not a percentage, a probability, or a calibrated strength
of evidence. Resnik similarity may not add or remove a gene.

No cross-method benchmark is required. Deterministic fixtures verify that the
implementation follows this formula; they do not establish scientific or
clinical usefulness. A separate, small, versioned face-validity set uses public
cases with cited HPO profiles and a predeclared known gene or disease. The
known target's entire tie group must end within the top 20: no more than 20
genes may have a score key greater than or equal to the target's. Cases and
expected targets are fixed before seeing the output. This prevents a universal
rank-1 tie from passing. It is a regression/sanity gate, not a diagnostic-
performance or clinical-validation claim. Unrelated public HPO queries must
also produce distinct, non-universal rankings.

#### Phenotype rank presentation

`phenotypeRank` is the structured field name and its user-facing column label
is **Phenotype rank**. The field remains nonselectable until the Resnik
implementation passes the required fixtures. After validation, it uses the
existing **Columns** menu alongside the other result fields. No separate rank
control, icon, or column chooser is added. Its availability and contextual
initial visibility depend only on the number of applied positive HPO features:

| Applied positive HPO features | Initial columns when no explicit saved column choice exists | Phenotype rank availability |
|---:|---|---|
| 0 | **Gene matches** | Not offered or displayed |
| 1 | **Gene matches** | Available in **Columns**, initially unchecked |
| 2 or more | **Phenotype rank**, then **Gene matches** | Initially checked and visible |

Two terms are a presentation threshold, not a claim that two terms make a
query clinically complete or scientifically valid. A one-feature rank is still
calculable, but a broad feature can produce many ties and limited
discrimination. **Gene matches** remains initially visible for every row set
because it works for HPO, MONDO, Reactome, and entered-gene selections and
explains the applied filter.

The current **Columns** menu remains authoritative. An explicit user choice to
show or hide either available column persists through the existing per-schema
column preference and wins when the result is reopened; applying a query must
not reset all column choices. When a newly available `phenotypeRank` has no
saved choice, the table above supplies its initial state. Choosing **Restore
recommended** restores the table's initial state for the currently applied
positive-HPO count. **Gene matches** is therefore initially visible, not
mandatory.

Showing **Phenotype rank** does not change gene membership, the active result
filter, or the current row order. The user may sort the column explicitly. The
compact cell displays the competition rank and fixed denominator, for example:

`3 of 4,804`

When tied, the cell also makes the tie visible:

`81 of 4,804 · 6 tied`

The cell must not display `81%`, `Top 4%`, a normalized 0-100 score, a colored
strength category, or labels such as **weak**, **moderate**, or **strong**.
Those forms imply calibration that a relative semantic-similarity rank does not
provide.

The column-header tooltip says:

> Ranks variant genes by similarity between the selected HPO features and HPO
> disease profiles. Lower ranks indicate greater relative similarity. Rank 1
> is highest.

An illustrative cell tooltip says:

> **SCN1A — Rank 81 of 4,804 · 6 tied**
>
> Based on 3 selected HPO features.
>
> Best-matching disease: Dravet syndrome (OMIM:607208).
>
> 6 genes share this rank.
>
> Method: Resnik query-to-disease best-match average.
>
> HPO release: 2026-06-23.
>
> This is a relative phenotype-similarity rank. It is not a diagnostic
> probability, pathogenicity classification, or the reason this gene was
> included.

The current compact cell remains the visible target; no tooltip icon or extra
column is added. Hover and keyboard focus on the cell expose the same tooltip,
the description is available to assistive technology, and Escape closes it.

The example demonstrates presentation only; its rank and tie count are not
asserted results for that gene, disease, query, or release. Runtime text uses
the actual ranked gene, selected-feature count, best-matching disease and
identifier, tie count, method, denominator, and installed HPO release. With one
selected feature, the tooltip also states: **Based on 1 selected HPO feature;
broad features may produce many ties.**

For an allele linked to multiple genes, the cell shows the best rank and the
tooltip names that gene; all gene-specific ranks remain available in structured
evidence and export.
The raw Resnik value, IC inputs, matched term pairs, algorithm version, and HPO
release remain audit/export data rather than primary UI values. A gene without
an eligible HPO disease profile displays **Not ranked**, not zero. Its tooltip
says: **No eligible HPO disease profile was available for this gene.**

Under `gene-profile-evidence-v2`, `phenotypeRank` is an integer competition
rank, where a smaller value is better. `phenotypeRankDetails` stores the fixed
denominator, tie count, exact best-disease identifier and label, raw Resnik
score, score key, algorithm version, query-term count, and HPO source hashes.
When the user explicitly sorts this column, the first sort direction is
ascending because a smaller rank is better. Merely showing the column through
its initial default does not apply that sort. A user-directed filter
such as `rank <= 100` has that literal meaning. Saved `v4` score filters are
invalidated rather than reinterpreted with the reversed scale.

MONDO condition links, Reactome membership, and entered-gene matches are not
part of `phenotypeRank`. They are already explained by **Gene matches**.

## MONDO condition-to-gene expansion

An HPO disease profile is linked to a selected MONDO condition when the
profile's disease identifier resolves to:

- the selected MONDO term itself (**Exact condition**); or
- an active descendant of the selected term (**Condition subtype**).

Genes with eligible `MENDELIAN` or `POLYGENIC` disease associations may be
expanded from condition matches. The evidence retains the selected condition,
matched MONDO condition, source disease, association type, and association
source. Other or missing association types are ineligible and are not silently
presented as Mendelian or polygenic.

An external disease identifier may resolve to MONDO only through an
unambiguous `skos:exactMatch` in the installed release. Close, broad, narrow,
and related mappings do not create gene membership. This local, HPO-mediated
condition expansion is deliberately limited to the installed sources. Its
coverage must therefore be described as **HPO disease annotations mapped
through MONDO**, not as all known genes for the condition.

## Reactome and entered-gene expansion

A selected Reactome pathway links each gene symbol listed in the installed GMT
gene set with the relation **Listed in pathway gene set**. An entered gene links
only its uniquely resolved approved gene with the relation **Entered gene**.

HGNC resolution is applied consistently to HPO genes, Reactome gene-set entries,
entered genes, and result genes before symbols are compared. A source symbol
that cannot be mapped uniquely is not allowed to alias a different gene.

## Combining and excluding genes

The current UI uses one combination behavior: a gene is included when it is in
at least one selected feature, condition, or pathway gene set. Entered genes are
also included. In set terms, the result is the union of all selected source
sets. There is no user-facing intersection or **Match every** mode.

Schema-5 explicit gene exclusions are unsupported in schema 6. Because the
current popover neither creates nor displays exclusions, the corrected browser
omits `excludedGenes` and the schema-6 API rejects that field. A future
exclusion workflow would require its own visible design and a new schema.

The preview reports both the complete resolved gene count and the number of
those genes present in the current result. Result presence changes the
displayed overlap; it does not change the biological expansion.

**Apply** is a result-filter action, not a separate save-record action. When
the preview reports zero included genes in the current result, **Apply** remains
disabled and the popover states **No resolved genes have variants in this
result.** That explanation uses the popover's existing inline scope/message
area; it does not open a modal or add another panel. The current rows and
previously active filter remain unchanged. A direct API request with the same
zero overlap is rejected. With positive overlap, applying the query creates a
normal result filter and shows only matching rows. It does not reorder
variants, infer inheritance, or assign causality.

The inspection action **View N without variants** remains visible whenever the
resolved gene set contains genes absent from the current result, including when
all resolved genes are absent and **Apply** is disabled. It shows those genes in
the existing inspection dialog. Opening, searching, or closing that dialog does
not apply the query, change the active saved query, or alter the result filter. With
partial overlap, `N` is only the number of resolved genes that have no variants
in this result.

## Evidence contract

Generated evidence has two levels:

- **Gene-level query evidence** records phenotype rank, documented feature
  links, condition links, and structured provenance for a canonical gene.
  Imported old evidence may retain legacy conflict data for audit only.
- **Allele-to-gene match evidence** records which selected items matched genes
  associated with a particular result allele.

Every gene-bearing row records `geneSymbol`, `canonicalGeneId`, `resultGeneId`,
and `identityStatus` as defined by `hgnc-identity-v2`. Allele match details keep
one row per `(allele, resolved gene, selected item)`. Compact display values are
derived from those rows but deduplicate selected items by
`(itemType, selectedId)`.

The field-level contract is:

| Field | Purpose | User-facing standalone field? |
|---|---|---|
| `geneMatches` | Compact selected-item match for a result allele | Yes; composite table/filter field |
| `phenotypeRank` | Integer competition rank for the combined positive-HPO query; displayed as **Phenotype rank** | Only after validation and with at least one positive HPO feature; initially visible with two or more when no explicit saved column choice exists, beside rather than instead of **Gene matches**; never a Genes mode or gene-inclusion rule |
| `phenotypeRankDetails` | Denominator, tie count, best disease, raw Resnik score, score key, algorithm, query count, and HPO provenance | No; structured tooltip/audit/export dependency |
| `phenotypeRelevance` | Legacy catalog-1 value produced by `hpo-lin-query-v4`, retained only when reading old evidence | No; never mapped to `phenotypeRank`, never selectable, and never used for an active schema-6 filter |
| `geneMatchDetails` | Structured allele, selected-item, gene, and relation rows | No; audit/export dependency |
| `phenotypeEvidenceDetails` | Structured feature matches and condition links | No; audit/export dependency |
| `geneMatch` | Boolean presence helper | No |
| `matchedSelectedItems` | Text fallback for composite match presentation | No |
| `matchedItemTypes` | Text fallback for composite match presentation | No |
| `profileLinked` | Legacy exact-feature-or-condition helper; not the final filter | No |
| `includedGene` | Internal final-inclusion marker | No |
| `observedFeatureLinked` | Internal exact-feature marker | No |
| `bestMatchingCondition` | Dependency of phenotype summary/details | No |
| `directFeatureMatches` | Legacy exact-overlap dependency; audit only | No |
| `absentFeatureConflict` | Legacy experimental score retained only in imported old evidence; not emitted by the corrected contract | No |
| `selectedConditionMatches` | Dependency of phenotype summary/details | No |
| `matchedSelectedConditions` | Dependency of phenotype summary/details | No |
| `selectedConditionRelation` | Dependency of phenotype summary/details | No |

Support fields may remain in the evidence file for filtering, compatibility,
export, and auditing. They must be nonselectable and excluded from generic
Variant Details rows.

## Variant Details presentation

Variant Details must not render a **Gene associations** or other gene-profile
section. The dedicated section duplicates the **Gene matches** result column
while exposing implementation detail that is not needed for routine variant
review.

The required UI change is removal of the current **Gene associations**
accordion and its repeated support-field rows. It is not replaced with a new
card, accordion, tab, or summary in Variant Details.

The **Gene matches** result column remains the user-facing explanation of which
selected query item matched a variant gene. Structured HPO, MONDO, Reactome,
and entered-gene provenance remains in the evidence package for filtering,
export, reproducibility, and future audit tooling. Internal booleans such as
`Gene match: true`, raw helper strings, and JSON support fields must never be
displayed as ordinary evidence rows.

The existing compact column layout is retained. The cell shows the first
matched selected-item label followed by the number of additional distinct
selected items, for example `Seizure +2`. The count is the number of distinct
user selections that match at least one gene linked to the allele. It is not a
count of diseases, annotation rows, ontology paths, repeated variants, or
gene-item duplicates.

For deterministic display, first deduplicate by `(itemType, selectedId)`,
retaining the earliest occurrence in each schema-6 selection array. Then order
items by the saved-query groups: `observed` HPO features, `conditions`,
`pathways`, and `genes`, preserving array order within each group. The compact
cell uses the first item in that order, and the tooltip lists all items in that
same order. This ordering is presentation only and does not assign greater
biological importance to the first item.

The tooltip lists the selected identifier and label, the matched gene when an
allele has more than one gene, and one precise relation:

- **HPO link via exact disease annotation**;
- **HPO link via more-specific disease annotation**;
- **Exact condition · MENDELIAN**, **Exact condition · POLYGENIC**,
  **Condition subtype · MENDELIAN**, or **Condition subtype · POLYGENIC** for
  MONDO;
- **Listed in Reactome pathway gene set**; or
- **Entered gene**.

Full source diseases, annotation references, evidence codes, association
types, mapping provenance, and release identifiers remain in structured
evidence and export. The compact tooltip does not call any of these matches a
diagnosis, causal relationship, or validated gene-disease relationship.

For MONDO, the ontology relation and association type are separate facts.
**Exact condition** versus **Condition subtype** says how the disease profile
relates to the user's selected condition; **MENDELIAN** versus **POLYGENIC** is
the association type supplied by the installed HPO gene-disease file. An
unknown association type is not eligible for MONDO expansion and is not shown
as if it were known.

All positive HPO terms in one applied query are asserted to describe one
patient. Because the current result format does not bind the query to one
sample, a multi-sample result applies one run-level gene set and rank to every
row; it must not imply per-sample phenotype matching. Result exports include
the selected HPO terms and their generated evidence, so the exported result
must be handled as patient phenotype data even though resolution itself is
local.

## Required correction plan

The implementation should change only the shared resolution and presentation
boundaries:

1. Build feature gene sets only from exact or descendant disease annotations
   joined to eligible Mendelian gene-disease associations. Do not use
   `matched_phenotypes` or any similarity threshold as the source of inclusion.
2. Compute phenotype rank separately with the fully specified
   `resnik-query-disease-v1` corpus, zero-count, aggregation, quantization, and
   tie rules. The rank may never add or remove genes.
3. Write profile schema 6, catalog schema 2, `gene-profile-evidence-v2`,
   `hgnc-identity-v2`, and the two specific algorithm versions. Include all
   source-asset SHA-256 values in the fingerprint, catalog, and export.
4. Persist the canonical HGNC identity, approved symbol, result-specific
   identity, and identity status everywhere a resolved gene crosses a search,
   saved-query, preview, evidence, or export boundary.
5. Revalidate every schema-6 HPO, MONDO, Reactome, and gene selection against the
   installed releases. Clear the active generation whenever a schema, contract,
   algorithm, source hash, or resolved identity differs, forcing a visible new
   preview before apply. Reject the preview as a whole when any saved item is
   unavailable or ambiguous; do not silently apply a partial saved query.
6. When a result contains profile schema 5 or earlier, open the result but do
   not restore or migrate its saved Genes query or active filter. Show an
   unsupported-query notice in the existing popover message area. Do not add a
   modal, compatibility screen, **any**, **every**, negative-feature, or
   exclusion control. Schema-6 requests containing those unsupported semantics
   are rejected rather than normalized. Align the writer, phenotype validator,
   and portable-result preflight validator on schema 6, while keeping archive
   integrity failures fatal.
7. Restrict pasted-entry resolution to exact gene identities. Keep HPO, MONDO,
   and Reactome selection in the typed autocomplete path, and never discard an
   item's type and reinterpret it as a manual gene. Preserve the current
   interaction in which editing the textarea or choosing **Use list** switches
   to gene-only input, while **Save list** stores resolved genes without
   ontology-query provenance.
8. Remove Monarch suggestion types and controls, `requestMonarchSuggestions`,
   `monarchSuggestions`, `monarchError`, `onlineEnrichment`, `onlineError`, the
   HTTP request/parser, its tests, and the source-catalog service entry. No
   corrected Genes action may initiate a Monarch network request.
9. Enable **Apply** only when the current preview reports at least one included
   gene in the current result. Enforce the same rule on the server, preserve
   `showMatchesOnly: true`, and show the specified zero-overlap message instead
   of applying an unfiltered no-op. Keep **View N without variants** available
   as a non-applying inspection action in the existing scope/message area.
10. Deduplicate the compact **Gene matches** value by selected item rather than
   by gene-item row, while retaining per-gene rows in `geneMatchDetails`.
11. Mark `geneMatch`, `matchedSelectedItems`, and `matchedItemTypes` as
   nonselectable catalog dependencies.
12. Remove the current **Gene associations** accordion and do not replace it
   with another phenotype-domain section in Variant Details. Keep the existing
   **Gene matches** result column and retain structured details only in the
   evidence package.
13. Add the new `phenotypeRank` and `phenotypeRankDetails` fields; never reuse
    `phenotypeRelevance`. Keep the rank unavailable until its deterministic and
    face-validity gates pass. If enabled, use the existing **Columns** menu,
    use the documented initial visibility, preserve explicit per-schema user
    column choices, place it immediately before **Gene matches**, and do not
    sort automatically. Make **Restore recommended** restore the initial state
    for the current positive-HPO count.
14. Keep MONDO condition information out of `phenotypeRank`. In **Gene
    matches**, show both the ontology relation and the source association type.
15. Treat all positive HPO selections in one application as one patient query,
    but apply it at result/run level until sample binding exists. Preserve that
    scope and the phenotype-data warning in export metadata.
16. Add accessible status and tooltip behavior through the existing message,
    task-status, and compact-cell surfaces; do not add another progress panel or
    tooltip icon. Ignore stale asynchronous previews. Treat performance timing
    as release telemetry until a reproducible runner baseline exists; do not add
    a flaky blocking threshold to this correction.
17. Gate release on the specified unit, browser, workspace, source-contract,
    deterministic Resnik, and public known-case checks. Record the exact commit,
    release asset digest, and reference-source hashes for the released build.

No MONDO, Reactome, or HGNC source files need to change. AnnoCAT can reconstruct
the HPO true-path association behavior from its existing HPO files, so a new
download is optional rather than required. Mondo project URLs remain necessary
for the local MONDO data source; they are not Monarch gene-suggestion support.
The HPO defect is both a zero-score software error and a scientific conflation
of similarity with association. The Variant Details defect is catalog
visibility and unnecessary presentation.

## Compatibility and reopening saved queries

Profile schema 6 is the only supported saved Genes query format. When an
otherwise valid result contains profile schema 5 or earlier, AnnoCAT still
opens the result. It does not restore, interpret, or migrate that nested Genes
query, its active generation, or its gene filter. The Genes popover shows an
unsupported-query notice in its existing inline message area and starts empty;
the existing Genes control shows no active-query count. No blocking modal or
new compatibility screen is added. Variants, annotations, candidates, notes,
and other result data remain available. An invalid or unsupported nested Genes
query must never make the entire result fail to open.

Saved manual gene lists in the Genes-popover dropdown are a separate data store
with their own schema (`SAVED_GENE_LISTS_SCHEMA_VERSION = 1`). Dropping profile
schema 5 does not delete or invalidate those lists. **Use list** loads only
their resolved genes, **Delete** removes only the selected list, and **Clear**
does not delete them.

A supported schema-6 record is revalidated through a fresh preview whenever its
Genes popover is opened, because installed reference releases may have changed:

- an HPO or MONDO identifier is preserved only when it is active, or replaced
  when the installed ontology supplies exactly one documented replacement;
- a Reactome pathway is preserved only when the same `R-HSA-*` identifier
  exists in the installed human release; a missing pathway is not recovered by
  label matching;
- an HGNC identifier resolves to its current approved symbol, while a
  symbol-only selection is resolved again and may become ambiguous; and
- an unavailable, multiply replaced, ambiguous, or unresolved item remains
  visible as its existing chip, the existing inline message reports the failed
  revalidation, and the server rejects the preview as a whole. **Apply** remains
  disabled until the user removes the item or selects a unique replacement.

Failing the whole preview is intentional. It prevents an old multi-item query
from silently changing meaning by dropping one item and applying the rest. A
unique documented replacement is canonicalized by the server and is stored
with its current label when the user applies the reviewed preview.

Any change to selections, resolved identities, source hashes, schema or contract
versions, or either algorithm version invalidates the fingerprint and active
generation. A new preview and apply creates a new catalog-2 evidence pair.

Old catalog-1 evidence produced by `hpo-lin-query-v4` can remain inside an
imported result for provenance and integrity verification, but it cannot drive
an active filter. Its legacy `phenotypeRelevance` value remains an unselectable
audit field and is never copied to `phenotypeRank`. A saved sort or numeric
filter on `phenotypeRelevance` is cleared. The user must explicitly choose a new
rank sort or filter after corrected evidence is available.

## Code ownership

| Area | Maintained implementation |
|---|---|
| Unified HTTP search and paste resolution | `crates/annocat-cli/src/main.rs` |
| HPO loading, ranking, gene expansion, evidence, profile persistence | `crates/annocat-cli/src/phenotype.rs` |
| MONDO search, canonicalization, exact/subtype relationships | `crates/annocat-cli/src/mondo.rs` |
| Reactome search, canonicalization, pathway gene sets | `crates/annocat-cli/src/reactome.rs` |
| HGNC and runtime gene resolution | `crates/annocat-cli/src/gene_identity.rs` |
| Removal of the Monarch online-service entry | `config/source-catalog.json`, `crates/annocat-core/src/source_catalog.rs` |
| Genes popover and preview/apply requests | `web/src/app/phenotypes.js` |
| Results column availability, initial visibility, persistence, and sorting | `web/src/app.js` |
| Portable phenotype export/import and unsupported-query isolation | `crates/annocat-cli/src/phenotype.rs`, `crates/annocat-cli/src/report_import.rs` |
| Variant Details exclusion of gene-profile evidence | `web/src/app/variant-presentation.js` |
| Browser-level phenotype behavior | `web/tests/phenotypes.test.mjs` |

## External product comparison

Commercial products provide useful presentation references, but they do not
define a universal scientific score or required workflow. VarSome, Emedgene,
and Fabric GEM are primarily case-oriented products; their use of patient
phenotypes does not change AnnoCAT's search-oriented product contract.

| Product | Documented presentation | AnnoCAT conclusion |
|---|---|---|
| [VarSome Clinical](https://docs.varsome.com/en/phenotype-matching) | HPO, MONDO, or OMIM terms are attached to and saved with a germline sample. The variant table is updated later with a phenotype-association count, matched names on hover, and an optional dynamic filter. | VarSome's **Save** is sample-metadata persistence, not the equivalent of AnnoCAT's immediate result-filter **Apply**. Its label-plus-count presentation still supports **Gene matches** and does not justify a separate Variant Details section. |
| [VarSeq](https://www.goldenhelix.com/Downloads/VarSeq/VSClinical-Manual.pdf) | Gene-list or phenotype-linked matching creates a boolean column indicating whether each variant gene matches at least one linked gene; a filter card can then use that field. PhoRank separately creates rank and score fields. | Deterministic association and semantic ranking must remain separate. A computed match field and its later filter are not a reason for AnnoCAT to accept an Apply action that silently leaves all rows visible. PhoRank's score scale is not transferable. |
| [Illumina Emedgene](https://help.emg.illumina.com/emedgene-analyze-manual/reviewing_a_case/phenotypic-match/phenotypic-match-models) | Proprietary model-specific scores are shown with the best-matching gene-related disease and model-specific interpretation ranges. | A displayed rank must identify its best disease and algorithm. Emedgene's thresholds cannot be applied to AnnoCAT. |
| [Fabric GEM](https://fabricgenomics.com/wp-content/uploads/2021/10/20211015-Final-GEM-paper-PDF.pdf) | Candidate genes and conditions are ranked using Bayes factors that combine phenotype, genotype, inheritance, and variant evidence under a defined hypothesis. | A phenotype-only semantic-similarity rank cannot be labeled or interpreted as a Bayes factor, probability, diagnostic confidence, or pathogenicity score. |

VarSome exposes **any** and **all** as an explicit gene-list analysis choice.
That is a product control, not a scientific requirement. Because AnnoCAT has no
such control, schema 6 has a fixed union policy and no `combination` field.

The products do not establish a universal button-enablement rule. VarSome saves
valid patient terms before its association column is updated, and table
filtering is a separate optional action. VarSeq materializes match or ranking
fields and lets the user add a filter afterward. AnnoCAT's **Apply** directly
means **filter this result now**; therefore a zero-overlap preview must disable
it rather than create an active saved query that silently shows every row.

Meaningful gene-disease information in a future Variant Details section would
require distinct data such as the disease, inheritance, gene-disease validity
classification, evidence source, and evaluation date. AnnoCAT does not
currently install that evidence model. Selection-match booleans and query
provenance therefore remain in **Gene matches** and audit/export data rather
than creating a placeholder section.

## Deliberate limits and open decisions

A positive semantic-similarity score can be broad and is not a documented gene
association.
The corrected gene set therefore uses exact and descendant HPO annotation
relationships only. Resnik ranking remains separate and must pass its specified
fixtures before display. A high relative rank does not establish strong
absolute evidence: every combined query has a highest-ranked gene even when the
query is uninformative. Phenotype frequency, negative findings, and genotype
likelihoods are future ranking concerns, not gene-membership rules.

MONDO matching intentionally includes exact conditions and descendants, not
ancestors or merely similar labels. This is an ontology classification used for
computational aggregation, not a clinical rule; Mondo likewise cautions against
using mappings directly as clinical rules in its
[mapping documentation](https://mondo.readthedocs.io/en/latest/editors-guide/mappings/).
Reactome expansion uses the installed
[Reactome Pathways Gene Set](https://reactome.org/download-data/). HGNC
ambiguity intentionally fails closed; HGNC IDs, rather than symbols or Ensembl
mappings, are the stable nomenclature identifiers according to the
[HGNC symbol-report documentation](https://hgnc.genenames.org/help/symbol-report/index.html).

## Nonfunctional UI requirements

These rules preserve the existing Genes-popover layout while making its state
understandable and safe:

- Preserve the current compact progress copy: the footer button says
  **Resolving…** during preview and **Applying…** during apply, and the existing
  application task status says **Updating gene matches · N s** during the
  server update. If resolution lasts longer than 250 ms, the existing
  status/live region also announces **Resolving genes…** to assistive
  technology. No separate progress panel is added.
- A disabled **Apply** button has adjacent visible text explaining whether the
  cause is unresolved input, an outdated or in-progress preview, or zero result
  overlap. This text uses the existing inline scope/message area and is not
  available only on hover.
- The existing compact **Gene matches** and **Phenotype rank** cells expose
  their tooltips on hover and keyboard focus, expose the same text to assistive
  technology, keep focus behavior predictable, and close with Escape. No new
  tooltip icon is added. **View N without variants** remains a real button and
  its existing dialog returns focus to the invoking button when closed.
- Clearing or changing a selection invalidates the pending fingerprint.
  Responses for older request fingerprints are ignored, so a slow earlier
  search cannot overwrite the latest query. Closing the popover or clearing the
  query remains possible while resolution runs.
- Release qualification may record preview/ranking timing as telemetry. It is
  not a blocking threshold until a versioned fixture and a reproducible baseline
  from the same runner class have been checked in. Functional release gates must
  not depend on an invented local-to-CI timing conversion.

## Required regression coverage

The maintained test contract includes:

- an exact HPO disease annotation produces an HPO link via exact disease
  annotation;
- a disease annotation below the selected HPO term produces an HPO link via
  more-specific disease annotation through the true-path rule;
- broader, sibling, and merely similar disease annotations do not add genes;
- three unrelated public HPO features produce distinct, non-universal gene
  counts through the preview endpoint;
- one ranking profile is built per distinct source `database_id`, MONDO-mapped
  identifiers are not collapsed, `D` and each `Dt` count profiles once, and a
  zero-count term never creates infinite IC;
- semantic-ranking details do not affect gene inclusion, and the Genes popover
  contains no ranking-mode selector;
- zero positive HPO features omit **Phenotype rank**, one makes it selectable
  but initially unchecked, and two or more initially check and show it
  immediately before **Gene matches** when no saved choice exists; explicit
  per-schema user visibility choices survive apply and reopen, while **Restore
  recommended** restores the initial state for the current positive-HPO count;
- initially showing **Phenotype rank** does not change row order, its first
  explicit sort is ascending, tied cells report the tie count, and unranked
  genes display **Not ranked** with the specified explanation;
- Resnik IC, most-informative-common-ancestor similarity, best-match averaging,
  best-disease gene aggregation, competition ranks, ties, and the fixed
  eligible-gene denominator reproduce versioned fixtures, including the
  12-decimal score key and deterministic secondary order;
- filtering result rows does not change a gene's phenotype rank;
- the current 0-100 Lin and absent-conflict values are not shown in corrected
  evidence, `phenotypeRelevance` is never reinterpreted as `phenotypeRank`, and
  genes without eligible profiles display **Not ranked**;
- MONDO exact and subtype conditions resolve to the documented genes, while
  close, broad, narrow, related, and ambiguous mappings do not add genes;
- Reactome results report and expand installed pathway gene-set entries;
- the pasted-list endpoint resolves exact current and historical gene
  identities only; valid HPO, MONDO, and Reactome IDs remain unrecognized there
  and must be selected through typed autocomplete;
- autocomplete selections retain their Feature, Condition, Pathway, or Gene
  type when their generated gene preview is written into the textarea;
- the Genes popover retains its current control inventory and contains no new
  panel or mode selector; editing the textarea or choosing **Use list** replaces
  typed ontology selections with gene-only input, **Save list** stores resolved
  genes without ontology-query provenance, **Delete** removes only the selected
  manual list, and **Clear** clears the active query/filter without deleting
  saved lists;
- current, historical, withdrawn, versioned Ensembl, ambiguous, and unknown
  HGNC inputs resolve as documented, and evidence retains the canonical HGNC
  ID separately from the result-specific identifier; ambiguous and unresolved
  associations do not enter the ranking denominator;
- changing any one source file without changing its release label changes the
  fingerprint because its SHA-256 changed;
- a preview with zero included genes in the current result displays the
  specified message and disables **Apply**, a direct zero-overlap apply is
  rejected without changing the active saved query or result rows, and a
  positive-overlap apply keeps `showMatchesOnly: true` and displays only matches;
- **View N without variants** remains available for zero and partial overlap,
  reports the correct absent count, and opening it never applies or changes a
  filter;
- browser selections use union behavior, schema-6 requests containing a
  combination, absent terms, or gene exclusions are rejected, and no
  **any**/**every** or
  ranking-mode control appears in the Genes popover;
- schema 6 is the only saved Genes query format read or written; profile schema
  5 or earlier cannot restore a query or active filter, but the containing
  result still opens, the unsupported notice uses the existing popover message
  area without a modal, and the separate saved manual gene lists remain
  available;
- a schema-6 phenotype group round-trips through portable export/import; an
  integrity-valid older phenotype group is skipped without blocking the base
  result, while a path, size, declared-file, or checksum failure still rejects
  the archive;
- schema 6 exposes no Monarch control, request field, stored result/error field,
  online-enrichment response field, service-catalog entry, endpoint call, or
  background network request;
- stale preview responses cannot replace the newest fingerprint; disabled-state
  explanations use the existing inline message area; **Resolving…**,
  **Applying…**, and **Updating gene matches · N s** remain the visible progress
  copy; and both compact-column tooltips pass keyboard and screen-reader checks
  without adding tooltip icons;
- one multi-gene allele whose genes both match one selected pathway displays
  only that pathway label, not `+1`; adding a second distinct selected item adds
  `+1`, while structured details still retain both gene rows; mixed item types
  use the documented saved-query group and array order for the compact label
  and tooltip; and
- **Gene matches** reports the defined HPO, Reactome, entered-gene, or combined
  MONDO relation-and-association text and remains available while Variant
  Details never renders gene-profile support fields or a Gene associations
  section.

Verification includes the focused Rust phenotype tests, HGNC, MONDO, and
Reactome unit suites, browser phenotype tests, full Rust workspace, full web
suite, and formatting checks in `.github/workflows/ci.yml` (**Test**).
Deterministic Resnik fixtures and the predeclared public known-case set belong
in that phenotype test path. The separate
`.github/workflows/source-contract-validation.yml` workflow validates upstream
file formats and remains a release-packaging gate; it does not replace the
phenotype behavior and scientific-contract tests. The Windows release workflow
must require both before publishing an asset.

No cross-method benchmark is required for corrected evidence. Release evidence
consists of the deterministic Resnik fixtures, a small versioned public-query
sanity set, and the predeclared top-20 known-case face-validity gate. A
different metric is considered only if later real-world evidence shows that the
specified Resnik ranking is inadequate; any such change requires a new ranking
algorithm version rather than silently changing existing rank meaning.
