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
updated on 2026-08-26. **Required** describes the target contract. The local
implementation status below does not mean the behavior has been published in a
release.

| Scope | Exact identity | Conformance to this contract |
|---|---|---|
| Public Windows release | GitHub release [`v0.1.0`](https://github.com/annocat-project/AnnoCAT/releases/tag/v0.1.0), release record published 2026-08-20; tag commit `795b5161cda0b814f27b886dc17f242546256bbe`; asset `AnnoCat-0.1.0-windows-x86_64.zip`, uploaded 2026-08-23, SHA-256 `fb27c464e07d6ec59152079dfc4b7fe80d0fbf116eaefd31d73821d6ca9c327a` | Uses the defective `hpo-lin-query-v4` behavior and does not conform to the corrected contract |
| Local tracked baseline | Commit `4b9ee40d1336d316899a1601cef6e9ce635c5057` | Implements the corrected scientific and existing-UI workflow using a generated phenotype evidence file |
| Local working tree | The tracked baseline plus the uncommitted live-query correction inspected on 2026-08-26 | Removes the generated phenotype evidence/catalog files and rebuilds the active query in memory. It predates the autocomplete-count, polygenic-scope, generated-list notation, upstream/downstream-scope, and wider-popover requirements added below; it is not committed, pushed, or published |
| Target corrected query | Profile schema `6`, active-query contract `gene-profile-live-v1`, identity contract `hgnc-identity-v2` | Partially implemented in the local working tree. HPO/MONDO autocomplete counts, the polygenic-association switch, the upstream/downstream variant checkbox, typed generated-list headings, and wider responsive popover remain to be implemented and verified before publication |

Local verification through 2026-08-26 used the pinned source manifests and produced
the following evidence for the pre-switch local implementation:

| Check | Local result |
|---|---|
| Full Rust workspace, including the standalone report worker and AppContainer tests | 387 passed, 5 intentionally ignored, 0 failed |
| Browser-module suite | 36 passed, 0 failed |
| Pinned HPO self-retrieval regression | SCN1A/OMIM:607208 rank 1 and CACNA1A/OMIM:108500 rank 1; each tie group ends at 1 in a 4,805-gene denominator. These queries reuse complete disease profiles from the installed HPO corpus, so they verify implementation behavior but are not independent patient-case evidence |
| Pinned HPO association sanity queries | Seizure 1,575; Short stature 987; Atrial septal defect 355; all distinct and non-universal |
| Official HPO membership-oracle audit | The 2026-06-23 `phenotype_to_genes.txt` asset matched its 66,907,216-byte size, publisher SHA-256, and five-column header. `HP:0001262` produced 27 Mendelian oracle gene IDs; the raw source reconstruction produced those 27 plus two unresolved placeholder-symbol rows, which explains the prior `-` output |
| Pinned HGNC reproducibility audit | The former mutable-current-object generation URLs returned `NoSuchKey`. The runtime manifest now uses the digest-equivalent immutable 2026-08 archive copies, which matched the configured complete-set and withdrawn-file byte sizes and SHA-256 values |
| Configured-source URL audit | All 116 unique production URLs passed the protocol-aware release check with zero blocking or advisory failures; all six pinned HPO, MONDO, and HGNC assets also passed full byte-count and SHA-256 verification |
| Isolated local UI smoke test | Existing Genes popover retained; entered HNF1A resolved to 1 of 1 result gene; **Gene matches** displayed `HNF1A`, no `true` value or **Gene associations** section appeared, and the compact tooltip worked by keyboard and Escape |
| Static and repository checks | Rust formatting, edited JavaScript syntax, the VEP concordance comparator self-test, and `git diff --check` passed; the final UI detector reported only two pre-existing width-transition warnings outside the changed CSS block |

These are local results, not GitHub Actions or release evidence. The workflows
are changed to run the pinned HPO gate, but that remote execution cannot occur
until the implementation is committed and pushed.

The public defect counts below are observations from that exact `v0.1.0`
release with the stated installed data, not timeless properties of AnnoCAT.
Release acceptance must use checked-in fixtures and recorded source hashes
rather than relying on prose counts alone.

The defective build has a composite phenotype algorithm value
`hpo-lin-query-v4`. Corrected evidence separates gene membership from semantic
ranking with `geneSetAlgorithmVersion: "hpo-association-query-v6"` and
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
cells. The target widens the existing popover on desktop, adds gene counts to
the existing autocomplete result line, adds one native switch for polygenic
associations, adds one default-off checkbox for VEP upstream/downstream variant
matches, and adds the **Phenotype rank** result column generated by the existing
dynamic-column system. It does not require a new panel, dedicated ranking
control, or Variant Details replacement section.

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
or **Match every** choice in the Genes popover. One polygenic-association switch
changes whether documented `POLYGENIC` disease-gene associations are eligible;
HPO `UNKNOWN` associations remain excluded. A separate default-off checkbox
changes whether allele filtering also accepts VEP upstream/downstream-only
matches. Neither control changes the union rule or the phenotype-ranking
method. Selecting multiple terms always previews the deterministic union of
their association-derived gene sets under the current control states.

### Genes popover interaction contract

The visible layout remains the current one: the heading **Add a feature,
condition, pathway, or gene** with one compact **Include polygenic associations
for HPO and MONDO** switch at the top right of the same header row; one **Search
names or identifiers** input; selected-item chips; the gene-only textarea; the
saved-list selector with **Use list**, **Save list**, and **Delete**; the existing
inline scope/message area; and one right-aligned footer action group ordered as
the **Include upstream/downstream variants (VEP 5 kb)** label, its checkbox,
**Clear**, and **Apply**. The visible controls have the following fixed meanings:

| User action | Meaning |
|---|---|
| Choose a result labeled **Feature** | Add that positive HPO feature to the patient phenotype query |
| Choose a result labeled **Condition** | Add that MONDO condition and use the documented condition-to-gene expansion |
| Choose a result labeled **Pathway** | Add that Reactome pathway and use its installed human gene set |
| Choose a result labeled **Gene** | Add the resolved gene directly |
| Paste genes in the textarea | Resolve gene identities only and switch to the manual-list workflow |
| Turn on **Include polygenic associations for HPO and MONDO** | In addition to the default Mendelian associations, admit source association type `POLYGENIC` for HPO features and MONDO conditions; continue to exclude `UNKNOWN`, and do not change Reactome, entered genes, union behavior, or phenotype ranking |
| Check **Include upstream/downstream variants (VEP 5 kb)** | In addition to direct selected-gene transcript consequences, allow VEP `upstream_gene_variant` and `downstream_gene_variant` proximity matches within the configured 5,000 bp distance for every resolved gene, regardless of whether it came from HPO, MONDO, Reactome, autocomplete, paste, or a saved manual list |
| Choose **Use list** | Load the selected saved list as manual genes; do not restore ontology selections |
| Choose **Save list** | Save the currently resolved genes as a manual gene list, not as a saved HPO/MONDO/Reactome query |
| Choose **Delete** | Delete only the selected saved manual gene list |
| Choose **Clear** | Clear the current Genes query and active result filter, reset both scope controls to their defaults, and do not delete saved manual gene lists |
| Choose **Apply** | When the current preview has at least one resolved gene present under the current variant-match scope, apply the fixed union gene set and show those rows |

On desktop, the popover target width is `51.25rem` (820 px at the default root
size), matching the existing wide-dialog design token instead of retaining the
current 560 px JavaScript cap. The actual width is
`min(51.25rem, calc(100vw - 24px))`, with 12 px minimum viewport margins and the
existing anchored centering logic. This gives the generated gene textarea and
long item-specific bracket headings materially more horizontal space without
making the popover taller. Its current maximum height, internally scrolling
content, sticky action area, and narrow-screen saved-list stacking remain
unchanged. No horizontal page scrolling or clipped footer action is permitted.

Autocomplete result labels and canonical identifiers make the chosen entity
type visible. Selected items appear as labeled chips. Other than the documented
polygenic-association switch and upstream/downstream variant checkbox, the
popover must not gain controls for an association mode, semantic-similarity
method, **any**, **every**, or a complete-patient-profile mode.

The polygenic switch is a single compact label-and-toggle control, not a
full-width row, card, or persistent helper paragraph. On desktop it stays
top-right in the header. When the available width cannot fit the heading and
control on one line, the header may wrap the control directly beneath the
heading and before the search field. It does not move into the footer. The
upstream/downstream checkbox remains in the footer because it changes the
variant rows included when **Apply** is chosen, rather than changing ontology
gene expansion.

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

#### Polygenic-association switch

The switch is off by default. Its exact source-type policy is:

| Switch state | HPO feature expansion | MONDO condition expansion | Phenotype-ranking corpus |
|---|---|---|---|
| Off | `MENDELIAN` | `MENDELIAN` | Mendelian-only, unchanged |
| On | `MENDELIAN` and `POLYGENIC` | `MENDELIAN` and `POLYGENIC` | Mendelian-only, unchanged |

The native tooltip says exactly:

> By default, HPO features and MONDO conditions include only Mendelian
> disease-gene associations. Turn this on to also include associations labeled
> POLYGENIC. Mendelian associations remain included.

HPO `UNKNOWN` associations are excluded in both states. They are not another
name for polygenic evidence and are not exposed through a second switch. The
[official HPO format](https://obophenotype.github.io/human-phenotype-ontology/annotations/genes_to_disease/)
enumerates `UNKNOWN` but does not preserve the more specific
Orphanet relationship meaning in this summary field. In the installed HPO
2026-06-23 file, all 8,288 `UNKNOWN` rows come from the Orphadata product and
cover 4,511 gene symbols and 4,102 disease identifiers. That audit is specific
to the pinned file with SHA-256
`a247027ae9944e34545e0a91060243ff6c118681c06379b9721af1ee4f39286a`, not a
timeless count. The underlying
[Orphadata product specification](https://sciences.orphadata.com/docs/OrphadataFreeAccessProductsDescription.pdf)
distinguishes
disease-causing, susceptibility, modifier, biomarker, candidate-gene, and other
relationships. Treating all of those flattened rows as one selectable class
would therefore mix materially different evidence and could create misleadingly
large gene sets. A concrete pinned-file example is TP53: HPO marks its links to
ORPHA:524, ORPHA:3318, and ORPHA:210159 alike as `UNKNOWN`, while
[Orphanet distinguishes them](https://www.orpha.net/en/disease/gene/TP53) as a
disease-causing relationship, a biomarker relationship, and a candidate-gene
relationship, respectively.

Supporting those Orphanet records later requires ingesting their explicit
relationship type and validation status, defining which types are eligible,
and versioning that source and algorithm. Any user option for non-causal or
otherwise unclassified relationships must then be separate from the polygenic
switch. The current contract does not infer a relationship type from HPO
`UNKNOWN` and does not use those rows for membership, counts, ranking, generated
gene sections, or **Gene matches**.

The polygenic switch affects association-derived membership and autocomplete
counts only. It never changes Resnik scores, the fixed union behavior, Reactome
membership, or manually entered genes. A gene included only through
`POLYGENIC` evidence may display **Not ranked** when it has no eligible
Mendelian disease profile in the fixed ranking corpus.

Changing the switch invalidates the current autocomplete response, gene
preview, fingerprint, and unapplied generated gene list. The browser reruns the
current autocomplete query and preview under the new state. **Clear**, editing
the generated textarea into a manual list, or choosing **Use list** resets the
switch to off. An applied schema-6 Genes query persists the state so reopening
reconstructs the same membership policy; a saved manual gene list does not
persist it.

#### Upstream/downstream variant checkbox

The checkbox **Include upstream/downstream variants (VEP 5 kb)** is immediately
before **Clear** in the popover footer. Its visible label is on the left of its
checkbox square, producing the fixed desktop order **label, checkbox, Clear,
Apply**. The label text and checkbox square are vertically centered on the same
horizontal axis as the text inside **Clear** and **Apply**. They must align with
the buttons' vertical midpoint, not with the buttons' top edge. The coded layout
uses one footer flex group with centered cross-axis alignment rather than a
manual top margin or pixel offset, so the alignment survives font scaling and
zoom. It is a checkbox rather than a switch because changing it does not
immediately alter the table;
**Apply** commits the selected variant-match scope. At narrow widths the entire
labeled checkbox moves as one unit to a full footer row above the action
buttons; the label and square do not split into different rows. It is off by
default, and **Clear** resets it to off.

With the checkbox off, a resolved gene matches an allele only when that gene
has at least one consequence whose Sequence Ontology term set is not limited to
`upstream_gene_variant` and `downstream_gene_variant`. Coding, splice, UTR,
intronic, and non-coding-transcript consequences therefore remain eligible even
when the selected gene is not the representative gene displayed in the main
table. A consequence that also carries an upstream or downstream term remains
eligible when it has another qualifying term.

With the checkbox on, a resolved gene may additionally match an allele when
its only relationship to that allele is `upstream_gene_variant` or
`downstream_gene_variant`. This scope applies uniformly to genes resolved from
HPO features, MONDO conditions, Reactome pathways, autocomplete, pasted input,
and saved manual lists. It changes allele membership and current-result overlap
counts only. It does not add genes to an HPO, MONDO, or Reactome expansion,
change autocomplete association counts, alter phenotype rank, or establish a
regulatory or causal relationship.

AnnoCAT does not currently pass `--distance`, and this correction must not
change that production command. The exact fastVEP source revision pinned in
`config/fastvep-pin.json` defines the `annotate --distance` default as 5,000 bp.
This also matches
[Ensembl VEP's documented default](https://www.ensembl.org/info/docs/tools/vep/script/vep_options.html)
for assigning upstream and downstream consequences relative to a transcript.
Under the current invocation and pinned fastVEP contract, AnnoCAT annotations
therefore use the 5 kb boundary through fastVEP's default.

Release validation must build the exact pinned fastVEP commit, confirm that its
`annotate` command still declares a 5,000 bp default, and exercise the implicit
default with boundary fixtures: 5,000 bp is included and 5,001 bp is excluded
on both transcript strands. The AnnoCAT invocation itself must contain no
`--distance` override in this correction. If a later fastVEP pin changes the
default, validation fails and the annotation contract must be reviewed before
the pin is accepted. Adding an explicit override later is a separate reviewed
annotation-command change, even if its value is also 5,000.

Upstream and downstream mean 5' and 3' relative to that transcript's strand;
they are not fixed left and right genomic directions.
The 5,000 bp boundary is an annotation setting, not a biological regulatory
boundary. [Ensembl classifies these consequences](https://www.ensembl.org/info/genome/variation/prediction/predicted_data.html)
as `MODIFIER`, for which effect prediction is difficult or unsupported, rather
than as evidence that the variant is benign, pathogenic, relevant, or
irrelevant.

The checkbox uses this exact native-tooltip text, including the documentation
reference as plain text because an operating-system tooltip cannot contain an
interactive link:

> Off by default. Includes VEP upstream/downstream matches within 5 kb of a
> transcript for a selected gene, which is VEP's default distance. These
> variants can be biologically relevant, but proximity alone does not show that
> they affect the selected gene. A row may display a different representative
> gene. Open Variant Details and use the transcript selector to view the
> selected gene's upstream/downstream annotation. See Transcript and evidence
> selection in the documentation.

The same complete text is exposed through an accessible description. The
referenced [Transcript and evidence selection](transcript-and-evidence-selection.md#variant-details-transcript-selector)
section explains why the table and matching transcript can name different
genes and how the selector changes the displayed transcript context.

Changing the checkbox invalidates the current preview and fingerprint and
recomputes `includedGenesInResult` under the new match scope. It does not rerun
ontology autocomplete or change the generated gene list. An applied schema-6
Genes query persists `includeUpstreamDownstream`; a saved manual gene list does
not. Editing generated text into a manual list or choosing **Use list** does not
silently change the visible checkbox state.

#### Generated gene-list association notation

Genes populated automatically into the textarea are
grouped under whole-line square-bracket headings that identify the exact
selected item and, for HPO or MONDO, the source association type. There is one
section per `(selected item, association type)` pair rather than one ambiguous
global **Mendelian** or **Polygenic** section. When the switch is off, only
Mendelian HPO/MONDO sections can appear; turning it on permits separate
polygenic sections. For example:

```text
[Feature: Example feature (HP:0000001) · MENDELIAN]
GENE1, GENE2

[Feature: Example feature (HP:0000001) · POLYGENIC]
GENE3

[Condition: Example condition (MONDO:0000001) · MENDELIAN]
GENE5

[Pathway: Example pathway (R-HSA-0000000)]
GENE6, GENE7
```

The example is format-only and does not assert real associations. A gene with
both Mendelian and polygenic associations appears in each applicable display
section, but the operative preview, Apply filter, and
saved manual list deduplicate it by canonical gene identity. Every selected
feature, condition, and pathway receives its own section even when multiple
items of the same type are selected. A heading always includes the item type,
canonical label, and canonical identifier so similarly named items cannot be
confused. Reactome and entered-gene sections have no disease-association type;
they use `[Pathway: label (identifier)]` and `[Entered genes]` respectively.

Every bracket heading occupies its own line and is display-only. The gene-only
paste parser ignores a line matching `[section label]`; it never submits that
line as a gene. Inline forms such as `GENE1 [POLYGENIC]`, invented symbol
suffixes, or comments after a gene are prohibited because the editable textarea
must continue to accept ordinary comma- or whitespace-separated gene entries.
Once the user edits the generated text, the workflow becomes a manual gene list
and the association-type grouping is no longer authoritative. **Save list**
stores only the deduplicated resolved genes, not the headings or provenance.

Phenotype rank is result presentation, not another Genes search option. Its
new structured field is `phenotypeRank`; the old `phenotypeRelevance` score is
not reinterpreted. In the corrected schema-6 implementation, the user first
applies positive HPO selections normally. With one positive HPO feature,
**Phenotype rank** is
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
`(itemType, selectedId)`. The live representation must still retain one
transient row per `(allele, gene, selected item)` so no gene-level provenance
is lost while the query is active. In the
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

### HPO and MONDO autocomplete omit association counts

The existing autocomplete renderer can display `geneCount`, but the current
server populates it only for Reactome pathways. HPO features and MONDO
conditions therefore show no gene count, including when the active polygenic
scope would resolve zero genes. Users cannot distinguish a recognized but
association-empty term from one that will create a useful gene filter until
after selecting it and waiting for a preview.

The corrected search response supplies the unique association-derived gene
count for every HPO feature and MONDO condition under the current
**Include polygenic associations for HPO and MONDO** switch state. HPO `UNKNOWN`
rows never enter
either count. This uses the
same source-type, ontology-direction, HGNC-resolution, placeholder-rejection,
and deduplication rules as a one-item preview. Reactome retains its existing
pathway gene count. Counts are source counts, not counts of genes or variants
present in the open result.

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

The installed source manifests record the release label and SHA-256 of every
source asset actually read: `hp.obo`, `phenotype.hpoa`,
`genes_to_disease.txt`, `mondo.json`, the installed human Reactome gene-set
file, `hgnc_complete_set.txt`, and `withdrawn.txt`. A release label alone is
not sufficient because a file can be republished without a new label.

The pinned URL must also remain reproducibly retrievable. During the 2026-08-26
review, the two `config/hpo-assets.json` URLs that addressed an old generation
of HGNC's mutable current-object paths returned `NoSuchKey`. The exact pinned
content remains available from [HGNC's monthly archive](https://hgnc.genenames.org/download/archive/):

- `hgnc_complete_set_2026-08-07.txt`, 16,931,491 bytes, SHA-256
  `faaeb6ae1e2a596be658b5f23ee44937c9c5379fa37d1f5ace54b94b16962b1c`,
  at `https://storage.googleapis.com/public-download-files/hgnc/archive/archive/monthly/tsv/hgnc_complete_set_2026-08-07.txt?generation=1786106235498772`;
- `withdrawn_2026-08-04.txt`, 258,931 bytes, SHA-256
  `77235063bba9492d09997e58387a50b6f750aed2de67a44c519c920b48f7ff87`,
  at `https://storage.googleapis.com/public-download-files/hgnc/archive/archive/monthly/tsv/withdrawn_2026-08-04.txt?generation=1785848506697635`.

The runtime manifest now uses these digest-equivalent archive URLs.
`scripts/verify-configured-urls.py` is part of source-contract validation and
therefore gates the Windows release. It checks every configured production URL
using the endpoint's actual contract, verifies declared object sizes, and fully
streams every asset with an explicit SHA-256. Citation and provider links are
reported but do not block a release; runtime dependencies do. This delivery
correction does not change the installed identity content or its release
fingerprint.

The installed source remains a rolling snapshot, not a permanently frozen data
channel. **Data sources** resolves the current official HPO and MONDO releases
and the current HGNC objects, freezes their versions, sizes, and checksums into
one install manifest, and installs them side by side with older snapshots. It
never reads changing remote data directly during a search. Source-contract
validation runs the exact production rolling resolver against the official
ClinVar, dbSNP, HPO, MONDO, and HGNC endpoints before a Windows release can run.

HGNC states that monthly archives older than 365 days can be deleted, while its
quarterly archives are currently retained. A quarterly snapshot must therefore
be a deliberate replacement for the release baseline, not a fallback for a
missing monthly URL: different snapshots can contain different approved
symbols, aliases, identifiers, or statuses. Loading both as identity assets or
silently switching between them is prohibited.

The latest matched quarterly pair available during this review is:

- `hgnc_complete_set_2026-07-07.txt`, 16,913,890 bytes, SHA-256
  `e73e9259177884b5994fc81ed733c1b3d4df34c84290bc9dddc86e960d5d6419`,
  at `https://storage.googleapis.com/public-download-files/hgnc/archive/archive/quarterly/tsv/hgnc_complete_set_2026-07-07.txt?generation=1783428431060479`;
- `withdrawn_2026-07-07.txt`, 258,931 bytes, SHA-256
  `77235063bba9492d09997e58387a50b6f750aed2de67a44c519c920b48f7ff87`,
  at `https://storage.googleapis.com/public-download-files/hgnc/archive/archive/quarterly/tsv/withdrawn_2026-07-07.txt?generation=1783428430353103`.

Moving the baseline to that pair requires one reviewed source update: set
`hgncRelease` to `2026-07-07`, replace both HGNC asset records and their sizes
and SHA-256 values, allow only the official quarterly HGNC path in manifest
validation, then rerun the complete HGNC resolution, HPO membership/ranking,
configured-URL, rolling-resolver, and full application suites. Existing result
provenance is not rewritten. If the August 2026 identity bytes must be retained
instead, the correct durable solution is a controlled immutable mirror of
those exact bytes, not the different July quarterly snapshot.

The preview and active-query fingerprint includes the normalized selections,
the `includePolygenic` and `includeUpstreamDownstream` states, profile schema,
active-query and identity contract versions, fixed union policy, both algorithm
versions, and every source-asset SHA-256. The saved record stores the
selections, both scope controls, and fingerprint, not a copied evidence dataset
or a second copy of the source manifest. Reopening recomputes the fingerprint
from the installed manifests; a mismatch deactivates the query and requires a
new preview and Apply.

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
| Active-query contract | `gene-profile-live-v1` | Meaning of the in-memory gene fields, allele matches, saved upstream/downstream scope, and live consequence join |
| `identityContractVersion` | `hgnc-identity-v2` | Canonical HGNC identity plus result-specific identity |
| `geneSetAlgorithmVersion` | `hpo-association-query-v6` | HPO, MONDO, Reactome, entered-gene, polygenic-scope, and generated-list grouping rules |
| `phenotypeRankingAlgorithmVersion` | `resnik-query-disease-v1` | Resnik corpus, score, aggregation, tie, and rank rules |

The legacy composite `algorithmVersion` identifies old unsupported data only.
New previews use the two specific algorithm versions. Profile schema 6 is the
only supported and written saved Genes query format. All earlier profile
schemas are unsupported and can never restore an active Genes filter. The
existing merged `query-field-catalog.json` may contain regenerated field
definitions and the active fingerprint so the current table can expose its
columns; it contains no phenotype evidence values and is not a second saved
query record.

## Unified search

The Genes popover searches features, conditions, pathways, and genes through
`GET /api/phenotypes/terms`. Queries shorter than two normalized search units
do not start a request and leave the autocomplete listbox closed. Results
contain an identifier, canonical label, item type, match kind, the text that
matched, and—for features, conditions, and pathways—the unique associated-gene
count under the current polygenic-switch state.

When a result is open, gene search also uses identities present in that result.
Exact identifiers rank first, followed by exact gene symbols, exact labels,
label prefixes, and other substring matches. Duplicate identifiers of the same
item type are removed.

### Autocomplete listbox states

After the existing input debounce starts a search request, the autocomplete
listbox opens with one nonselectable **Searching…** status row while that current
request remains pending. A fast response may replace the row immediately; the
browser must not impose a minimum display time or delay usable results merely
to make the loading state visible.

When the current request succeeds with no results, the listbox remains open and
shows one nonselectable **No matching feature, condition, pathway, or gene**
row. Matching results replace either transient row with the normal selectable
options and their documented identifiers, types, match details, and associated-
gene counts. The pending and no-match messages are each announced once through
a polite status region. They do not create a selection, change the current
query, or enable **Apply**.

Missing HPO/MONDO or Reactome data continues to use the existing warning and
**Open Data sources** action outside the listbox; the autocomplete does not add
a duplicate source-unavailability row. A request failure continues to use the
existing inline message area. These transient listbox states add no persistent
control or helper text to the popover.

`POST /api/phenotypes/terms` is the exact, gene-only resolver for pasted
entries. Saved gene lists use the same gene-only identity contract. A current
or uniquely resolvable historical gene symbol or identifier returns the
approved gene identity. The response separates recognized, ambiguous, and
unrecognized values; partial search hits are not silently accepted. An HPO,
MONDO, or Reactome identifier or label is unrecognized in this workflow even
when it is a valid ontology item. Those items must be selected through the
unified `GET` search so their type is retained.

### Autocomplete gene-count contract

`geneCount` has one meaning: the number of unique canonical genes that a
single autocomplete item would contribute before intersection with the open
result. For an HPO feature it applies the exact-or-descendant annotation rule;
for a MONDO condition it applies the exact-condition-or-subtype rule; for a
Reactome pathway it counts unique installed pathway genes. It is not a disease
count, annotation-row count, phenotype-rank denominator, sum across selected
items, or current-result overlap count.

HPO and MONDO supply both a Mendelian-only count and a Mendelian-plus-polygenic
count for the installed resource release. HPO `UNKNOWN` rows contribute to
neither. The search response chooses one integer according
to `includePolygenic`; switching the control aborts or ignores the
older response and reruns the current query. Counts are computed once from the
already installed HPO, MONDO, and HGNC knowledge and retained as compact
in-memory integer arrays. Autocomplete must not traverse the disease graph,
scan result Parquet, or write a disk cache on every keystroke. With the current
approximately 42,800 searchable HPO and MONDO terms, two `u32` counts per term
retain roughly 0.33 MiB before ordinary container overhead.

A zero-count term remains visible as a recognized ontology result and displays
**0 associated genes**; it is not silently removed. A zero-count HPO feature
remains selectable because, when combined with a selection that contributes
genes, its ontology position can still participate in the separate Resnik
rank. A zero-count MONDO condition has no membership or ranking effect under
the current scope and is shown with `aria-disabled="true"`; turning on the
polygenic switch can enable it if its Mendelian-plus-polygenic count is
positive. A zero-gene
Reactome pathway is handled the same way as a zero-count condition. Exact-ID
and label searches therefore still reveal that the term is known instead of
looking like a failed search.

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
stored label is replaced with the installed canonical label. Each result also
reports its unique feature-derived `geneCount` under the current association
scope.

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

Search results report the number of active descendants and the unique
condition-derived `geneCount` under the current polygenic-switch state. Deprecated
MONDO terms are replaced only when a single replacement is available.
Selection fails instead of silently choosing when a term is unavailable,
non-human, or has no unique replacement.

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
The active query retains both identities so a later symbol change does not
silently alias an existing match. A symbol-only fallback is permitted only
when no stable HGNC mapping is available and is marked as such in the live
provenance details.

A corrected gene search, saved selection, preview, and in-memory query row use the
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
| HPO feature | `HP:#######` | Label, matching synonym, match kind, polygenic-scope gene count | Active phenotypic-abnormality term or unique obsolete replacement |
| MONDO condition | `MONDO:#######` | Label, synonym scope, external-ID match, descendant count, polygenic-scope gene count | Active human condition or unique deprecated replacement |
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
genes. It stores `includePolygenic` as the polygenic-association switch state
and `includeUpstreamDownstream` as the VEP upstream/downstream variant-match
checkbox state. Schema 6 does not contain `excluded` or `excludedGenes`.
Applying a saved Genes query requires the fingerprint returned by the latest
preview; the server rejects an apply request
when releases, source hashes, identities, selections, schemas, contracts, or
either algorithm changed after preview.

The browser has no **Match any** or **Match every** control. Schema 6 has no
`combination` field. Selected feature, condition, pathway, and entered-gene sets
are always combined by the fixed union rule. A schema-6 preview or apply request
containing `combination`, `excluded`, or `excludedGenes` is rejected; an API
caller must not receive silently changed semantics.

The schema-6 top-level record written by the corrected implementation contains
exactly `schemaVersion`, `runId`, `updatedAt`, `observed`, `conditions`,
`pathways`, `genes`, `includePolygenic`, `includeUpstreamDownstream`,
`showMatchesOnly`, and `activeGeneration`. For example, an applied HPO query is
stored as:

```json
{
  "schemaVersion": 6,
  "runId": "example-run",
  "updatedAt": "2026-08-25T18:00:00Z",
  "observed": [{"id": "HP:0001250", "label": "Seizure"}],
  "conditions": [],
  "pathways": [],
  "genes": [],
  "includePolygenic": false,
  "includeUpstreamDownstream": false,
  "showMatchesOnly": true,
  "activeGeneration": {
    "fingerprint": "sha256-value",
    "matchedGeneCount": 24
  }
}
```

Because schema 6 has not been distributed in a corrected release, writers
always emit both control booleans without advancing the schema number. A reader
may treat an otherwise valid earlier schema-6 record that omits either field as
`false`; the change from `hpo-association-query-v5` to
`hpo-association-query-v6` still invalidates its old fingerprint and requires a
reviewed preview before Apply. Neither field accepts a value other than a JSON
boolean. The abandoned combined `includePolygenicAndUnknown` name and any
`includeUnknown` field are rejected rather than silently reinterpreted.

`activeGeneration` is `null` when no corrected query is applied.
`showMatchesOnly` must be `true` whenever `activeGeneration` is present. Schema
6 has no `ranking`, `limitToLinkedGenes`, negative-feature, gene-exclusion, or
Monarch fields. Corrected rank and match data are reconstructed in memory from
the selections, installed sources, HGNC resolution, and result consequences.

`showMatchesOnly` controls the result filter after a saved Genes query is
applied. The corrected browser may enable **Apply** only when the latest preview
has a current fingerprint, has no unresolved or ambiguous pasted genes, and
reports `includedGenesInResult > 0` under the current
`includeUpstreamDownstream` scope. It always submits `showMatchesOnly: true`.
The server must enforce the same positive-overlap precondition and must not
accept a zero-overlap apply by silently changing `showMatchesOnly` to `false`.

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
3. Join each matched disease to `MENDELIAN` entries in
   `genes_to_disease.txt`. When `includePolygenic` is true, also join
   `POLYGENIC` entries. `UNKNOWN`, other, and missing types are ineligible.
4. Retain the selected term, annotated term, disease, gene, association type,
   source, and the HPO annotation reference, evidence code, frequency, and
   biocuration metadata when present.
5. Union the per-feature gene sets with the selected MONDO, Reactome, and
   entered-gene sets.

This follows the same ancestor-propagation direction documented for HPO's
[`phenotype_to_genes.txt`](https://obophenotype.github.io/human-phenotype-ontology/annotations/phenotype_to_genes/),
which includes genes inherited from descendant annotations. AnnoCAT reconstructs
the relation from its installed disease annotations and disease-gene file so it
can retain disease and association provenance and apply the explicit switch-
controlled association-type policy. The HPO summary file is also the release-
validation oracle specified below. Equality is defined on normalized biological
identities after association-type filtering, not on raw row order, duplicate
rows, or source gene-symbol text.
A broader disease annotation, a sibling term, or another merely similar term
may contribute to ranking but must not create gene membership.

Explicitly absent features may remain in unsupported old audit evidence, but
they are not migrated into schema 6. They do not create or exclude genes and do
not participate in ranking. Disease-reported negative phenotypes also remain
separate from positive associations.

### HPO gene-membership release oracle

The official HPO `phenotype_to_genes.txt` file is a suitable oracle for whether
AnnoCAT reconstructed the published HPO feature-to-gene relation in the correct
ontology direction. It is not a second runtime data source and is not an
independent biological truth set: HPO generates it from the same underlying
ontology, phenotype annotations, and disease-gene curation that AnnoCAT reads.
Passing this oracle demonstrates source conformance and catches implementation
errors; it does not establish that every published association is causal or
clinically useful.

For the currently pinned HPO release, the validation-only asset is:

| Field | Required value |
|---|---|
| Release | `2026-06-23` |
| File | `phenotype_to_genes.txt` |
| Bytes | `66,907,216` |
| SHA-256 | `1386a4dd3ea046f5a5971f4011a3711a9d7928d60961e2e7b757b6860c63c778` |
| URL | `https://github.com/obophenotype/human-phenotype-ontology/releases/download/v2026-06-23/phenotype_to_genes.txt` |

The source-contract job downloads and streams this file in temporary CI or
release-validation storage. AnnoCAT must not install it, load it at runtime,
include it in the application or release ZIP, expose it in **Data sources**,
write it beside a result, include it in a profile fingerprint, or retain it as
a disk cache. A future HPO update must pin this oracle from the same release as
`hp.obo`, `phenotype.hpoa`, and `genes_to_disease.txt` and record its byte size
and publisher SHA-256 before expected outputs are updated.

The complete membership-oracle test is:

1. Verify the byte size, SHA-256, and the exact five-column header `hpo_id`,
   `hpo_name`, `ncbi_gene_id`, `gene_symbol`, `disease_id`. Reject malformed
   HPO identifiers, nonnumeric NCBI Gene identifiers, empty disease identifiers,
   or a release mismatch rather than trying to repair the oracle.
2. Treat numeric NCBI Gene ID as the source identity. The source symbol is a
   diagnostic label only and must never be used as the comparison key or emitted
   when it is `-`, blank, ambiguous, or inconsistent with the pinned HGNC
   identity bundle.
3. Normalize the `NCBIGene:` prefix in `genes_to_disease.txt` and join each
   oracle row to that file on the exact pair `(disease_id, numeric NCBI Gene
   ID)`. An unmatched oracle row is a source-contract failure; the test must not
   guess an association type from the disease or gene.
4. Because `phenotype_to_genes.txt` does not contain `association_type`, apply
   the product policy from the joined `genes_to_disease.txt` row. With
   `includePolygenic: false`, retain only `MENDELIAN`; with it true, retain
   `MENDELIAN` and `POLYGENIC`. Exclude `UNKNOWN`, other, and missing types in
   both states. If duplicate joined rows have different types, a normalized
   provenance tuple retains each eligible literal type while the operative gene
   set remains deduplicated.
5. In separate validation code, independently reconstruct the raw association
   relation from `hp.obo`, positive `phenotype.hpoa` annotations, and
   `genes_to_disease.txt`. Resolve every eligible numeric source identity through
   the pinned `hgnc-identity-v2` bundle. Compare genes by stable
   `canonicalGeneId`, or by the documented unique `symbol-only` fallback only
   when no HGNC identity is available. Record every unresolved or ambiguous raw
   association in a deterministic exclusion ledger containing HPO ID, disease
   ID, numeric NCBI Gene ID, source label, association type, and reason. A
   placeholder such as `-` is never a gene.
6. Reconcile the independently reconstructed view with the published oracle.
   Every eligible, canonically resolved oracle tuple must exist in the raw view,
   and every canonically resolved raw tuple must exist in the oracle. A raw row
   absent from the oracle is permitted only when it is also excluded from the
   operative set and appears in the reviewed exclusion ledger. For release
   2026-06-23, the raw disease-gene file has 10 such Mendelian rows, covering
   seven numeric NCBI Gene IDs, all with `-` as the source symbol; the official
   oracle publishes none of those seven IDs. A resolved raw-only gene, an
   unexplained oracle-only gene, or any other unreviewed differential is a hard
   failure rather than an automatically accepted source update.
7. For every active HPO descendant of `HP:0000118`, including active terms
   absent from the oracle, construct expected Mendelian-only and Mendelian-plus-
   polygenic sets. Compare them with AnnoCAT's one-item generated membership by
   exact set equality after the documented identity exclusions. Assert no
   missing genes, no extra genes, no duplicate canonical identities, and exact
   equality of the independently derived exclusion ledger. The autocomplete
   `geneCount`, one-item preview count and gene set, generated textarea's
   operative union, and active resolved-gene map must all reproduce that same
   expected set before it is intersected with variants in the open result.
8. Test unions separately with fixed pairs and triples of overlapping and
   disjoint HPO selections. The operative union is deduplicated by canonical
   identity, while each selected feature retains its own disease and association-
   type provenance.
9. Verify exact-versus-more-specific provenance against `phenotype.hpoa` and
   `hp.obo`, not against the flattened oracle. For every emitted feature,
   disease, and gene tuple, classify it as exact only when that disease has a
   positive annotation to the selected term; otherwise classify it as more-
   specific only when the positive annotation is a descendant. Exact wins when
   both paths exist. A broader, sibling, merely similar, negative, or absent
   annotation is a hard failure if it contributes membership.
10. Retain `HP:0001262` **Excessive daytime somnolence** as a pinned sentinel.
    Under the 2026-06-23 Mendelian policy the official oracle contains 27
    distinct numeric NCBI Gene identities. The independent raw reconstruction
    contains those 27 plus `10108` and `3653`, whose
    `genes_to_disease.txt` symbols are both `-`. Neither numeric ID resolves to
    a unique approved identity through the pinned HGNC bundle: HGNC marks
    MKRN3-AS1 as an entry-withdrawn record, while
    [NCBI reports](https://www.ncbi.nlm.nih.gov/gene/3653) that retired Gene ID
    `3653` was replaced by `104472715` and the pinned source row does not carry
    that replacement. The expected operative list is therefore exactly 27
    canonical genes, with the two raw source rows in the exclusion ledger and
    zero placeholder symbols. For human review, the approved symbols are
    `ASH1L, DMPK, DNMT1, DPYS, FKRP, FKTN, FOXE1, HCRT, HERC2, HMGCL, LARGE1,
    MAGEL2, MKRN3, MOG, NPAP1, POMT1, POMT2, PRR12, PTRHD1, PTS, PWAR1, PWRN1,
    SLC22A5, SNORD115-1, SNORD116-1, TDP2, TWNK`; the automated assertion still
    compares canonical IDs rather than symbol text. AnnoCAT must not invent an
    online or name-based replacement during a local query.

This oracle intentionally does not test MONDO condition expansion, Reactome
pathway membership, Resnik scores, or clinical diagnostic performance. Those
have separate tests below. Commercial software output is also not an oracle,
because its licensed sources, versions, filters, and algorithms are generally
not identical or fully inspectable.

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

The current 0-100 Lin value must not be carried into the corrected rank or
shown to users: it is not a percentage, a probability, or a calibrated strength
of evidence. Resnik similarity may not add or remove a gene.

Ranking validation has three noninterchangeable layers. Hand-calculated
ontology fixtures verify `D`, `Dt`, IC, most-informative common ancestors,
asymmetric best-match averaging, best-disease aggregation, score quantization,
competition ranks, and ties. A small validation-only reference implementation
then compares complete per-gene score keys and ranks for fixed multi-feature
queries; it must not share production scoring code or become a second runtime
pipeline. Finally, a versioned face-validity set uses positive HPO observations
from cited public patient cases and predeclared causative HGNC identities.

Each patient-case manifest must pin its source release and SHA-256 and record the
case ID, publication, exact positive HPO IDs, and target HGNC ID before the
output is examined. It must not replace the patient's observations with the
target disease's complete `phenotype.hpoa` profile, and a Phenopacket finding
marked excluded must not be converted into a positive query term. The target
must first occur in the fixed eligible ranking universe; otherwise the case is
reported as a coverage failure rather than being assigned a synthetic rank.

Published work supports this structure but also limits what it can prove.
[`PhEval`](https://link.springer.com/article/10.1186/s12859-025-06105-4) is the
closest direct standard: it defines reproducible corpora, adapters, normalized
rank output, analysis, and phenotype-noise experiments for monogenic
phenotype-driven gene and variant prioritizers. The
[`Phenopacket Store`](https://pubmed.ncbi.nlm.nih.gov/39394689/) supplies
standardized individual case observations and predeclared genomic
interpretations specifically suitable for testing prioritization software.
Earlier evaluations such as
[`Phen2Gene`](https://pmc.ncbi.nlm.nih.gov/articles/PMC7252576/) likewise use
expert-curated, solved single-gene cases and report where the known causal gene
appears in the result. A broader
[`evaluation of phenotype-driven gene-prioritization methods`](https://pmc.ncbi.nlm.nih.gov/articles/PMC8921623/)
independently recommends real patient cases with one established causal gene
and expert-curated HPO terms. These papers apply directly to AnnoCAT's optional HPO
**Phenotype rank**, but not to exact HPO/MONDO/Reactome membership, identifier
resolution, or manual gene filtering. Those search functions require the
source-specific equality tests in this document rather than a patient-case
ranking benchmark.

The case-level validation has two distinct sets:

1. A small frozen sentinel set is a blocking regression gate. Its inclusion
   criteria and expected worst-tie rank are fixed before implementation output
   is examined. Every sentinel's entire tie group must end within its declared
   threshold; the initial threshold is top 20 unless the manifest documents a
   stricter precomputed expectation. Unrelated sentinels must produce distinct,
   non-universal rankings. The threshold protects known behavior and is not a
   claim that every real Mendelian case should rank within the top 20.
2. A broader frozen benchmark cohort measures plausibility and performance.
   It reports target coverage, found and missed counts, top 1, 3, 5, 10, and 20
   proportions, mean reciprocal rank, median target rank, complete worst-tie
   rank distribution, runtime, and peak memory. Initially this report is
   release telemetry, not a new arbitrary pass percentage. After one reviewed
   baseline release, subsequent releases must not regress beyond a predeclared
   tolerance; the cohort and tolerance cannot be changed after seeing a
   candidate build's output.

#### Patient-case benchmark governance

The broader cohort has a predeclared eligibility contract. A primary benchmark
case must represent one individual, contain at least one observed positive HPO
phenotypic-abnormality term, and have a curated molecular interpretation that
identifies exactly one causal human gene which resolves uniquely to an approved
HGNC identity. Multiple causal variants, including a compound-heterozygous
pair, remain eligible when they implicate that same gene. AnnoCAT relies on the
frozen corpus interpretation for this benchmark; it does not independently
reclassify the case's variants.

The primary monogenic cohort excludes unsolved or candidate-only cases,
polygenic or complex-trait cases, multiple molecular diagnoses, multilocus
cases with more than one causal gene, and chromosomal or structural diagnoses
for which no single causal gene is established. A case is not excluded because
AnnoCAT omits or poorly ranks its target. An eligible target outside the fixed
ranking universe remains a coverage failure, contributes zero to mean
reciprocal rank, and stays in the relevant denominator.

Corpus preparation normalizes every positive HPO identifier against the pinned
HPO release. An active identifier is retained; an obsolete identifier is
accepted only when the release provides one unambiguous replacement, and that
replacement is recorded. An unknown, ambiguous, or replacement-free positive
term fails manifest validation instead of being silently dropped. Findings
marked excluded remain recorded as excluded and do not enter the positive query.
Known duplicate representations of the same individual are collapsed before
the corpus is frozen. The manifest records every included case and every
exclusion with its reason, and neither rules nor exclusions may change after
candidate output has been examined.

The cohort report must make repeated cases visible rather than allowing a gene
or publication with many cases to dominate unnoticed. It reports:

- the existing case-weighted coverage, top-k, mean reciprocal rank, median, and
  worst-tie rank distribution, because a case is the user-workflow unit;
- gene-balanced top-k and mean reciprocal rank, computed by averaging the
  case-level value within each causal HGNC gene and then giving each gene equal
  weight;
- disease-balanced summaries when every included case has one unambiguous
  causal disease identifier; otherwise the report says why that summary is not
  applicable rather than inventing a disease assignment;
- eligible and excluded counts with reasons, unique causal genes, diseases and
  publications, cases per gene and publication, and positive HPO-term counts;
  and
- descriptive strata for predeclared query-term-count, phenotype-specificity,
  target-gene disease-profile-count, and source-publication groups. For this
  report, a query term's specificity is the greatest usable IC of that term or
  one of its ancestors under the same pinned ranking corpus; terms with no
  usable IC are counted separately. Bin boundaries are fixed in the manifest
  before candidate results are run.

Top-k proportions, mean reciprocal rank, and median rank have deterministic
95% percentile-bootstrap intervals from 2,000 fixed-seed resamples. The primary
interval resamples causal HGNC genes and retains all cases for each sampled gene;
a separate publication-cluster sensitivity interval resamples publications and
retains all of their cases. These intervals describe stability within the
frozen corpus. They do not make overlapping public cases independent or turn a
retrospective public benchmark into an estimate of clinical diagnostic
accuracy. Resampling whole dependent groups follows the established
[`cluster-bootstrap`](https://pmc.ncbi.nlm.nih.gov/articles/PMC7148287/)
principle rather than incorrectly treating every case row as independent. The
report records the randomization algorithm and version, seed, ordered input
cluster identifiers, SHA-256 of the generated assignment stream, and the
nearest-rank interval endpoints (ordered resamples 50 and 1,950) so another
runner can reproduce the summary exactly without storing every resample.

The broader cohort also has a target-label negative control. After the real
per-case rank outputs are frozen, 1,000 distinct fixed-seed derangements map each unique
causal HGNC gene to a different causal gene and apply that mapping to all cases
for the original gene. Patient HPO profiles, case groups, output rankings, the
set of target genes, tie handling, and missing-target rules remain unchanged;
only the phenotype-to-target correspondence is broken. The operation is a
lookup against recorded rank output and does not rerun the ranker. The report
records the algorithm, seed, ordered target identifiers, and assignment-stream
SHA-256 rather than embedding every mapping. If the frozen cohort cannot supply
1,000 distinct derangements, the null gate is not runnable and **Phenotype
rank** is not release-qualified from that cohort.

The predeclared primary null statistic is gene-balanced mean reciprocal rank
using the end-of-tie evaluation rank. Before **Phenotype rank** is release-
qualified, its observed value must strictly exceed the nearest-rank 95th
percentile, the 950th value after the 1,000 null values are sorted ascending.
Observed and null top 1, 3, 5, 10, and 20 proportions are also reported
but are not additional first-baseline gates. Absolute real-cohort top-k values
remain telemetry until a reviewed baseline supplies a non-regression tolerance.
This negative control detects a universal, popularity-only, or otherwise
nondiscriminating result at cohort level; it does not prove that every returned
gene is causal. Complete HPO/MONDO/Reactome source-oracle comparisons remain the
test of membership correctness.

PhEval directly supports standardized monogenic corpora, normalized output, and
phenotype perturbation. The gene-balanced summaries, clustered intervals, and
target-label control above are AnnoCAT-specific statistical safeguards for a
corpus with repeated genes and publications; this document does not present
them as requirements imposed by PhEval or as a universal field standard.

For every top-k proportion, median rank, and mean reciprocal rank, the target's
evaluation rank is the end of its complete tie group: displayed competition
rank plus tie count minus one. A missing or uncovered target contributes zero
to mean reciprocal rank and is included in the denominator. The report also
retains the displayed competition rank and tie count. This conservative rule
prevents an uninformative universal tie at displayed rank 1 from appearing to
be perfect performance.

The runner should accept a pinned Phenopacket corpus and emit PhEval-compatible
gene ranks, target identifiers, found/missed state, and tie-aware ranks. It may
be a validation-only adapter or export step; AnnoCAT does not need PhEval in the
application or release ZIP. PhEval is designed for monogenic prioritization, so
the benchmark excludes MONDO-only queries, Reactome pathways, manual gene
lists, and `POLYGENIC` membership-only additions. Variant-level or simulated-
VCF PhEval experiments are also out of scope because this version ranks genes
and does not combine phenotype similarity with variant pathogenicity into one
diagnostic score.

Robustness testing follows the perturbation patterns used by PhEval and prior
phenotype-prioritization studies, including the
[`clinical phenotype-based gene-prioritization study`](https://pmc.ncbi.nlm.nih.gov/articles/PMC4117966/)
that separately evaluated noise, less-specific terms, and their combination.
Each case is rerun after deterministic term dropout, replacement by a
less-specific HPO ancestor, and addition of unrelated positive HPO noise. Seeds,
perturbation levels, replacement rules, and resulting profiles are saved with
the report. Cohort-level degradation is measured rather than requiring every
individual case to change monotonically, because removing or broadening a
feature can legitimately improve one case's relative rank.
Query-term order, duplicate input terms, and excluded Phenopacket findings are
hard invariance tests for `resnik-query-disease-v1`: order and duplicates must
not change output, and excluded findings must remain ignored. A robustness
failure cannot be repaired by changing the corpus or expected target after the
candidate results are known.

Phenopacket Store and HPO can share terminology, publications, or downstream
curation, so the public cohort is not assumed to be free of information leakage
and must not be presented as an independent estimate of real-world diagnostic
accuracy. Cases must be grouped by publication, disease, and causal gene when
constructing subsets; a random case-level split can place near-duplicate cases
on both sides. The strongest future test is a temporal holdout: freeze
AnnoCAT's HPO knowledge at date T, then test cases or gene-disease associations
first published or curated after T. A separately governed clinical cohort is
stronger still, if one becomes available. Until then the proper claim is
**source conformance, implementation correctness, and case-level plausibility**,
not clinical validation. The existing SCN1A and CACNA1A tests that query
complete profiles from the same installed HPO corpus remain useful
self-retrieval regressions but do not satisfy this external case-level layer.

#### Verification implementation scope and difficulty

The minimum implementation reuses the existing production resolver and ranker,
the ignored official-HPO case test, `verify-phenotype-known-cases.ps1`, and the
current release workflows. It does not add PhEval, a second ontology library, a
new runtime service, or a patient-case corpus to the application or release ZIP.

| Work item | Existing base | Relative difficulty |
|---|---|---|
| Pinned fastVEP 5 kb default guard | Source-contract CI already clones and builds the exact pin | Low: add an implicit-default boundary fixture and record the verified default; do not change AnnoCAT's command |
| Resnik hand fixtures and invariants | Production ranking and a fixed-IC/tie test already exist | Low to moderate: expand the synthetic DAG cases and add order, duplicate, excluded-term, and worst-tie assertions |
| Independent Resnik comparator | The production formula and output contract are fixed | Moderate: write one small validation-only implementation that shares no scoring helpers and compare exact score keys and ranks |
| Complete HPO/MONDO/Reactome/HGNC source oracles | Production parsers, pinned manifests, and source-contract workflow already exist | Moderate to high: independent parsing, complete-set reconciliation, exclusion ledgers, and actionable failure reports are still required |
| Public patient-case runner | The official-HPO test already loads the ranking corpus and calls the production ranker | Moderate: validate the pinned case manifest and exclusion ledger, then emit deterministic case-, gene-, and optional disease-balanced rank records |
| Cohort uncertainty and negative control | Uses the recorded public-case output; no second ranker or runtime dependency is required | Low after the runner exists: add fixed-seed clustered bootstrap summaries and target-label derangements to the validation report |
| Noise and imprecision report | Uses the same public-case runner | Low to moderate after the runner exists: generate deterministic dropout, ancestor-replacement, and unrelated-noise profiles from recorded seeds |
| Temporal or private clinical holdout | No independent post-snapshot or governed clinical cohort is bundled | High scientifically and operationally; this is future stronger evidence, not a blocker for the current correction |

The shortest correct implementation path is:

1. Keep the small synthetic source, Resnik, identity, and UI tests in normal CI.
2. Extend the existing ignored official-HPO runner to read a validation-only
   case manifest, enforce its eligibility and normalization rules, and write
   machine-readable target rank, tie, coverage, cohort-composition, runtime,
   and memory records. It must call the production ranker rather than duplicate
   it.
3. Keep the independent Resnik comparator small and restricted to synthetic
   fixtures; it must not become another way to generate user results.
4. Run the frozen sentinels as the blocking patient-case gate. During a Windows
   release, download the pinned public case corpus into temporary runner storage,
   run the broader cohort, clustered summaries, target-label control, and
   robustness variants, upload the JSON report, and discard the corpus. Do not
   install or package it.
5. Reuse the existing source-contract workflow for complete HPO, MONDO,
   Reactome, and HGNC equality. Keep those pass/fail results separate from the
   patient-case ranking report.

This is a moderate verification project overall, not a difficult production
architecture change. The largest work is making independent full-source
comparisons and useful failure reports. The ranking runner itself is relatively
small because AnnoCAT already has the production scorer and an official-HPO
test entry point.

A cross-method contest is not required to verify the fixed
`resnik-query-disease-v1` contract. Comparing Resnik with Lin, Jiang-Conrath,
or a commercial tool would answer a later algorithm-selection question, not
whether this implementation computes its declared method correctly.

#### Phenotype rank presentation

`phenotypeRank` is the structured field name and its user-facing column label
is **Phenotype rank**. It may be enabled in a release only after the deterministic,
reference-comparator, and frozen-sentinel gates pass and the versioned
public-cohort report is produced and reviewed, including the target-label null
gate above. The current
complete-disease-profile self-retrieval tests do not by themselves satisfy that
requirement. Once qualified, the field uses the existing **Columns** menu
alongside the other result fields. No separate rank control, icon, or column
chooser is added. Its availability and contextual
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

`3 of 4,805`

When tied, the cell also makes the tie visible:

`81 of 4,805 · 6 tied`

The cell must not display `81%`, `Top 4%`, a normalized 0-100 score, a colored
strength category, or labels such as **weak**, **moderate**, or **strong**.
Those forms imply calibration that a relative semantic-similarity rank does not
provide.

The column-header tooltip says:

> Ranks variant genes by similarity between the selected HPO features and HPO
> disease profiles. Lower ranks indicate greater relative similarity. Rank 1
> is highest.

An illustrative cell tooltip says:

> **SCN1A — Rank 81 of 4,805 · 6 tied**
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
tooltip names that gene; all gene-specific ranks remain available while the
active query is reconstructed in RAM.
The raw Resnik value, IC inputs, matched term pairs, algorithm version, and HPO
release remain audit/export data rather than primary UI values. A gene without
an eligible HPO disease profile displays **Not ranked**, not zero. Its tooltip
says: **No eligible HPO disease profile was available for this gene.**

Under `gene-profile-live-v1`, `phenotypeRank` is an integer competition
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

With the polygenic switch off, only `MENDELIAN` disease associations are
eligible for condition expansion. With it on, `POLYGENIC` is also eligible.
The live provenance retains the selected condition, matched MONDO condition,
source disease, literal association type, and association source. `UNKNOWN`,
other, and missing association types are ineligible.

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
those genes present in the current result under the active
`includeUpstreamDownstream` scope. A gene with only upstream/downstream result
consequences counts as present only when that checkbox is on. Result presence
changes the displayed overlap; it does not change the biological expansion.

**Apply** is a result-filter action, not a separate save-record action. When the
selected items resolve no genes under the active polygenic-switch state, **Apply**
remains disabled and the popover states **No associated genes were found for
this selection in the installed HPO/MONDO data.** It does not offer **View 0
without variants**. When genes resolve but none has a variant in the current
result, **Apply** remains disabled and the popover instead states **None of the
N associated genes have variants in this result.** That second state retains
**View N without variants**. These explanations use the existing inline
scope/message area; they do not open a modal or add another panel. The current
rows and previously active filter remain unchanged. A direct API request must
distinguish and reject both cases. With positive overlap, applying the query
creates a normal result filter and shows only rows with an eligible consequence
match under the checkbox state. It does not reorder variants, infer inheritance,
or assign causality.

The inspection action **View N without variants** remains visible whenever the
resolved gene set contains genes absent from the current result, including when
all resolved genes are absent and **Apply** is disabled. It shows those genes in
the existing inspection dialog. Opening, searching, or closing that dialog does
not apply the query, change the active saved query, or alter the result filter.
With partial overlap, `N` is only the number of resolved genes that have no
eligible variant match in this result under the current upstream/downstream
scope.

## Runtime and storage model

Applying a Genes query writes only the small schema-6 `phenotypes.json` saved
query record in the result-library overlay. AnnoCAT does not write
`phenotype-gene-evidence.*.parquet`,
`phenotype-field-catalog.*.json`, a phenotype query projection, or another
phenotype cache. Opening a result, clearing the query, and successful Apply also
remove those obsolete files if an earlier local build left them behind.

The existing generic `query-field-catalog.json` remains a derived table-column
catalog. It can include the regenerated **Phenotype rank** and **Gene matches**
field definitions and active fingerprint, but it contains no gene, allele,
match, rank, or source-provenance rows. FAVOR refreshes preserve these active
field definitions. Removing the active query removes the definitions and any
obsolete phenotype projection by field index without changing canonical or
FAVOR annotation evidence.

On Apply and after reopening a result, AnnoCAT resolves the saved selections
against the currently installed HPO, MONDO, Reactome, and HGNC assets. It builds
one active gene map in RAM for resolved genes present in that result and
registers the map as temporary DuckDB tables for each result query. The allele
join reads every relevant gene-bearing row from `consequences.parquet`; it is
not limited to the representative gene displayed in the normal **Gene** column.
The join then excludes upstream/downstream-only gene matches unless
`includeUpstreamDownstream` is true. Reopening after application exit rebuilds
the map. Applying another query or clearing the current query replaces or
releases it. Ordinary result searches and filters are not written to disk by
this feature.

The active RAM map contains canonical/result identities, gene-level rank and
provenance values, selected-item matches, and the exact result-gene aliases
needed for the consequence join. Result pages still query the full result.
The separate bounded result-page cache may remember up to 10,000 matched row
identities for each of eight recent queries as a paging optimization. Its key
includes the active-query fingerprint, so applying a different Genes query
cannot reuse an earlier matched-row set. The cache does not cap full-result
filtering, counting, searching, or sorting.

The 2026-08-26 WGS acceptance measurement used 4,804 active genes and a result
with 4.85 million variants. The retired file join and the live consequence
join both matched exactly 645,533 alleles. Warm live count, page, and rank-sort
queries completed in approximately 0.36, 0.63, and 0.64 seconds respectively;
the measured process peaked at 552.1 MiB working set and 563.4 MiB private
bytes. These values are local benchmark evidence, not a universal hardware
guarantee.

Portable result ZIPs include `phenotypes.json` only. They do not include a
phenotype evidence Parquet file or phenotype field catalog. Import restores a
valid schema-6 selection record; the active map is regenerated from installed
sources when the result is opened. Unsupported older profile schemas leave
only a minimal inactive, clearable schema marker so the existing popover
can explain why the query was not restored. Their selections are not retained,
they never restore a filter, and they never enter a later portable package.
Archives that explicitly declare the retired phenotype evidence roles are
rejected as unsupported legacy packages.

## Live query contract

The active query has two transient levels:

- Gene-level rows hold phenotype rank, documented HPO and MONDO links, pathway
  membership, entered-gene matches, and identity provenance for a canonical
  gene.
- Allele-to-gene rows are produced by joining result consequences to the active
  resolved-gene aliases and applying the saved upstream/downstream scope. They
  identify which selected items matched which genes and which consequence
  context made each result allele eligible.

Every gene-bearing row keeps `geneSymbol`, `canonicalGeneId`,
`resultGeneId`, and `identityStatus` as defined by `hgnc-identity-v2`.
Allele match details retain one transient row per
`(allele, resolved gene, selected item)`. Compact display values deduplicate
selected items by `(itemType, selectedId)`.

The field-level contract is:

| Field | Purpose | User-facing standalone field? |
|---|---|---|
| `geneMatches` | Compact selected-item match for a result allele | Yes; composite table/filter field |
| `phenotypeRank` | Integer competition rank for the combined positive-HPO query; displayed as **Phenotype rank** | Only with at least one positive HPO feature; initially visible with two or more when no explicit saved column choice exists, beside rather than instead of **Gene matches** |
| `phenotypeRankDetails` | Denominator, tie count, best disease, raw Resnik score, score key, algorithm, query count, and HPO provenance | No; tooltip dependency regenerated in RAM |
| `geneMatchDetails` | Allele, selected-item, matched gene, relationship, matched consequence term, and representative-gene context | No; compact-tooltip dependency regenerated in RAM |
| `phenotypeEvidenceDetails` | Documented feature and condition links for a gene | No |
| `geneMatch` | Boolean presence helper used by the applied filter | No |
| `matchedSelectedItems` | Text fallback for composite match presentation | No |
| `matchedItemTypes` | Text fallback for composite match presentation | No |
| `profileLinked` | Exact-feature-or-condition helper | No |
| `includedGene` | Internal final-inclusion marker | No |
| `observedFeatureLinked` | Internal exact-feature marker | No |
| `bestMatchingCondition` | Best HPO disease profile for ranking explanation | No |
| `directFeatureMatches` | Exact selected-feature count | No |
| `selectedConditionMatches` | Selected MONDO condition count | No |
| `matchedSelectedConditions` | Selected MONDO condition labels | No |
| `selectedConditionRelation` | Exact/subtype relation and source association type | No |

`phenotypeRelevance` and `absentFeatureConflict` belong only to unsupported
older contracts. They are not regenerated, imported as active data,
reinterpreted as `phenotypeRank`, or exposed through the schema-6 UI.

Support fields exist only in the active RAM map and temporary query tables.
They are nonselectable and excluded from generic Variant Details rows. A CSV
export can include the visible **Gene matches** or **Phenotype rank** column
when the user includes that column; it does not silently create a hidden
phenotype evidence package.

## Variant Details presentation

Variant Details must not render a **Gene associations** or other gene-profile
section. The dedicated section duplicates the **Gene matches** result column
while exposing implementation detail that is not needed for routine variant
review.

The required UI change is removal of the current **Gene associations**
accordion and its repeated support-field rows. It is not replaced with a new
card, accordion, tab, or summary in Variant Details.

The **Gene matches** result column remains the user-facing explanation of which
selected query item matched a variant gene. HPO, MONDO, Reactome, and
entered-gene provenance is regenerated in RAM for filtering and compact
tooltips. Internal booleans such as
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

- **HPO link via exact disease annotation · MENDELIAN** or **· POLYGENIC**;
- **HPO link via more-specific disease annotation · MENDELIAN** or
  **· POLYGENIC**;
- **Exact condition · MENDELIAN** or **· POLYGENIC**, and the corresponding
  **Condition subtype** forms for MONDO;
- **Listed in Reactome pathway gene set**; or
- **Entered gene**.

When `includeUpstreamDownstream` admits a proximity-only match, the same native
tooltip also names the matched gene, its upstream/downstream consequence, and
the different representative gene when applicable. It tells the user to open
Variant Details and select a transcript for the matched gene. For example:

> Matched gene: CACNA1A. Consequence: downstream gene variant. The
> table displays ITPR1 from the representative transcript. In Variant Details,
> select a CACNA1A transcript to inspect this match.

The example is format-only. The displayed symbols and consequence come from the
current allele. The existing Variant Details transcript selector, not a new
gene-associations section, exposes the matching transcript context. Changing
the selector changes only the detail context and its transcript-scoped evidence;
it does not change allele membership or the representative values in the main
table. The user workflow and selection distinction are documented in
[Transcript and evidence selection](transcript-and-evidence-selection.md#variant-details-transcript-selector).

Full source diseases, annotation references, evidence codes, association
types, mapping provenance, and release identifiers remain in the regenerated
RAM provenance. They are not written as a separate result asset. The compact
tooltip does not call any of these matches a diagnosis, causal relationship,
or validated gene-disease relationship.

For HPO and MONDO, the ontology relation and source association type are
separate facts. For MONDO, **Exact condition** versus **Condition subtype** says
how the disease profile relates to the user's selected condition. For HPO,
**exact disease annotation** versus **more-specific disease annotation** says
how the disease phenotype relates to the selected feature. `MENDELIAN` and
`POLYGENIC` are literal types supplied by the installed HPO
gene-disease file. HPO `UNKNOWN` rows are excluded rather than displayed or
reinterpreted as Mendelian, polygenic, causal, or low confidence.

All positive HPO terms in one applied query are asserted to describe one
patient. Because the current result format does not bind the query to one
sample, a multi-sample result applies one run-level gene set and rank to every
row; it must not imply per-sample phenotype matching. Portable result ZIPs
include the selected HPO terms in `phenotypes.json`, so they must be handled as
patient phenotype data even though resolution itself is local. A plain filtered
CSV contains only the columns the user chose.

## Required correction plan

The implementation should change only the shared resolution and presentation
boundaries:

1. Build feature gene sets only from exact or descendant disease annotations
   joined to association types eligible under `includePolygenic`.
   Build MONDO gene sets under the same type policy. Do not use
   `matched_phenotypes` or any similarity threshold as the source of inclusion,
   and exclude HPO `UNKNOWN` rows in both polygenic-switch states.
2. Compute phenotype rank separately with the fully specified
   `resnik-query-disease-v1` corpus, zero-count, aggregation, quantization, and
   tie rules. The rank may never add or remove genes.
3. Write profile schema 6 and use `gene-profile-live-v1`,
   `hgnc-identity-v2`, and the two specific algorithm versions. Include every
   source-asset SHA-256, `includePolygenic`, and
   `includeUpstreamDownstream` in the fingerprint. Do not write a phenotype
   evidence Parquet file, phenotype field catalog, or phenotype query
   projection.
4. Preserve the canonical HGNC identity, approved symbol, result-specific
   identity, and identity status everywhere a resolved gene crosses a search,
   saved-query, preview, live-query, or visible export-column boundary.
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
   ontology-query provenance. Treat whole-line generated `[section]` headings
   as display-only and never accept inline association annotations as genes.
8. Remove Monarch suggestion types and controls, `requestMonarchSuggestions`,
   `monarchSuggestions`, `monarchError`, `onlineEnrichment`, `onlineError`, the
   HTTP request/parser, its tests, and the source-catalog service entry. No
   corrected Genes action may initiate a Monarch network request.
9. Enable **Apply** only when the current preview reports at least one included
   gene in the current result under `includeUpstreamDownstream`. Enforce the same
   rule on the server, preserve `showMatchesOnly: true`, and distinguish no
   source-associated genes from genes with zero eligible current-result overlap
   instead of applying an unfiltered no-op. Keep **View N without variants**
   only when `N` resolved genes actually exist as a non-applying inspection
   action in the existing scope/message area.
10. Deduplicate the compact **Gene matches** value by selected item rather than
   by gene-item row, while retaining per-gene and matched-consequence context in
   transient `geneMatchDetails`.
11. Mark `geneMatch`, `matchedSelectedItems`, and `matchedItemTypes` as
   nonselectable catalog dependencies.
12. Remove the current **Gene associations** accordion and do not replace it
   with another phenotype-domain section in Variant Details. Keep the existing
   **Gene matches** result column and regenerate its details only for the
   compact tooltip. Preserve all consequence contexts in the existing
   transcript selector and document how to select the matched gene's transcript.
13. Add the new `phenotypeRank` and `phenotypeRankDetails` fields; never reuse
    `phenotypeRelevance`. Keep the rank unavailable until its deterministic and
    validation-only reference-comparator and frozen patient-case gates pass. If
    enabled, use the existing **Columns** menu,
    use the documented initial visibility, preserve explicit per-schema user
    column choices, place it immediately before **Gene matches**, and do not
    sort automatically. Make **Restore recommended** restore the initial state
    for the current positive-HPO count.
14. Keep MONDO condition information and the polygenic-association switch out of
    `phenotypeRank`. In **Gene matches**, show both the HPO or MONDO ontology
    relation and the literal source association type.
15. Treat all positive HPO selections in one application as one patient query,
    but apply it at result/run level until sample binding exists. Preserve that
    scope in the schema-6 record packaged with a portable result.
16. Add accessible status and tooltip behavior through the existing message,
    task-status, and compact-cell surfaces; do not add another progress panel or
    tooltip icon. Ignore stale asynchronous previews. Treat performance timing
    as release telemetry until a reproducible runner baseline exists; do not add
    a flaky blocking threshold to this correction.
17. Gate release on the specified unit, browser, workspace, full-release HPO
    membership oracle, MONDO/Reactome/HGNC source contracts, deterministic and
    reference-comparator Resnik checks, frozen patient-case sentinels, and the
    required public-cohort report. The public report must include its eligibility
    and exclusion ledger, case- and gene-balanced metrics, clustered uncertainty
    summaries, and a passing target-label null control.
    Record the exact commit, release asset digest, and every runtime and
    validation-reference source hash for the released build.
18. Keep the active resolved-gene map in RAM only, rebuild it from schema-6
    selections and installed assets on reopen, join it to all consequence genes,
    apply the saved upstream/downstream eligibility rule, and prove live filter,
    display, sort, and export parity before release.
19. Precompute and retain Mendelian-only and Mendelian-plus-polygenic unique-gene
    counts for each searchable HPO feature and MONDO condition. Exclude HPO
    `UNKNOWN` rows from both. Return the selected count
    through the existing `geneCount` field without a per-keystroke disease-graph
    traversal, result scan, or disk cache. Add the exact nonselectable
    **Searching…** and **No matching feature, condition, pathway, or gene**
    listbox states without duplicating the existing source-unavailability
    warning or inline request-error message.
20. Add exactly one native **Include polygenic associations for HPO and MONDO**
    switch at the top right of the popover header, persist `includePolygenic` in
    schema 6, and
    use it consistently for autocomplete counts, preview, Apply, reopen, and
    provenance. Do not expose or accept an HPO `UNKNOWN` switch. Widen the
    desktop popover to the existing `51.25rem` wide-dialog target while retaining
    12 px viewport margins and responsive stacking.
21. Return transient generated-list groups per exact selected item and source
    association type. Format whole-line headings with item type, canonical
    label, canonical identifier, and—only for HPO/MONDO—the literal association
    type. Deduplicate the operative gene set even when a gene appears in more
    than one display section.
22. Add exactly one default-off **Include upstream/downstream variants (VEP 5
    kb)** checkbox immediately before **Clear** in the popover footer. Render
    its label to the left of the checkbox square, use the desktop order **label,
    checkbox, Clear, Apply**, and center the label and square on the buttons'
    vertical midpoint rather than their top edge. Persist
    `includeUpstreamDownstream` in schema 6, retain the existing fastVEP
    annotation command with no `--distance` override, verify the pinned 5,000 bp
    default as specified above, and exclude
    upstream/downstream-only allele-to-gene matches when the checkbox is off.
    Use the exact documented native tooltip and accessible description, including
    its plain-text reference to **Transcript and evidence selection**.
23. Replace the expired mutable-current-object HGNC generation URLs in
    `config/hpo-assets.json` with the documented digest-equivalent monthly
    archive URLs. Extend source-contract validation to download and verify those
    exact runtime identity assets and the validation-only HPO oracle before the
    Windows release job can run; do not change identity fingerprints merely
    because the delivery URL changes while bytes and SHA-256 remain identical.
24. Run the production rolling-source resolver integration test in
    source-contract validation so failures in current ClinVar, dbSNP, HPO,
    MONDO, or HGNC discovery block the Windows release. Keep update discovery
    rolling but every installed dataset immutable and checksum-verified.
25. For a long-lived HGNC release baseline, either adopt one matched official
    quarterly complete/withdrawn pair as a reviewed identity-source update or
    mirror the exact current baseline bytes at a controlled immutable location.
    Never use a different quarterly snapshot as an automatic URL fallback.

No installed MONDO, Reactome, HGNC, or HPO source set needs another runtime
file. AnnoCAT reconstructs the HPO true-path association behavior from its
existing installed HPO files. `phenotype_to_genes.txt` is downloaded only by
source-contract/release validation and is neither installed nor retained.
Mondo project URLs remain necessary for the local MONDO data source; they are
not Monarch gene-suggestion support.
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
does not delete them. Manual lists do not store either query-scope control or
generated bracket headings. The current saved Genes query separately persists
`includeUpstreamDownstream`, including when its genes came from a manual list.

A supported schema-6 record is revalidated through a fresh preview whenever its
Genes popover is opened, because installed reference releases may have changed:

- an HPO or MONDO identifier is preserved only when it is active, or replaced
  when the installed ontology supplies exactly one documented replacement;
- a Reactome pathway is preserved only when the same `R-HSA-*` identifier
  exists in the installed human release; a missing pathway is not recovered by
  label matching;
- an HGNC identifier resolves to its current approved symbol, while a
  symbol-only selection is resolved again and may become ambiguous; and
- `includePolygenic` is restored before autocomplete counts or the
  gene preview are recomputed; an omitted value in a valid pre-switch schema-6
  record is treated as `false`;
- `includeUpstreamDownstream` is restored before current-result overlap is
  recomputed; an omitted value in a valid earlier schema-6 record is treated as
  `false`; and
- an unavailable, multiply replaced, ambiguous, or unresolved item remains
  visible as its existing chip, the existing inline message reports the failed
  revalidation, and the server rejects the preview as a whole. **Apply** remains
  disabled until the user removes the item or selects a unique replacement.

Failing the whole preview is intentional. It prevents an old multi-item query
from silently changing meaning by dropping one item and applying the rest. A
unique documented replacement is canonicalized by the server and is stored
with its current label when the user applies the reviewed preview.

Any change to selections, either scope-control state, resolved identities,
source hashes, schema or contract versions, or either algorithm version
invalidates the fingerprint and active generation. A new preview and Apply
replaces the saved fingerprint and the single active RAM map.

Old phenotype evidence and catalog files are neither imported nor used to drive
an active filter. Local library overlays remove recognized obsolete files when
the result is opened or the query is cleared, applied, or migrated. A portable archive that declares
the retired phenotype-evidence roles is rejected as an unsupported legacy
package. `phenotypeRelevance` is never copied to `phenotypeRank`; a saved sort
or filter on a field absent from the current selectable catalog is cleared.

## Code ownership

| Area | Maintained implementation |
|---|---|
| Unified HTTP search and paste resolution | `crates/annocat-cli/src/main.rs` |
| HPO loading, ranking, gene expansion, live query registration, profile persistence | `crates/annocat-cli/src/phenotype.rs` |
| MONDO search, canonicalization, exact/subtype relationships | `crates/annocat-cli/src/mondo.rs` |
| Reactome search, canonicalization, pathway gene sets | `crates/annocat-cli/src/reactome.rs` |
| HGNC and runtime gene resolution | `crates/annocat-cli/src/gene_identity.rs` |
| Explicit VEP upstream/downstream distance | `crates/annocat-cli/src/annotation.rs`, pinned `tools/fastvep-fork/crates/fastvep-cli/src/main.rs` |
| Removal of the Monarch online-service entry | `config/source-catalog.json`, `crates/annocat-core/src/source_catalog.rs` |
| Genes popover and preview/apply requests | `web/src/app/phenotypes.js` |
| Results column availability, initial visibility, persistence, and sorting | `web/src/app.js` |
| Profile-only portable phenotype export/import and unsupported-query isolation | `crates/annocat-cli/src/phenotype.rs`, `crates/annocat-cli/src/report_import.rs`, `crates/annocat-cli/src/report_library.rs` |
| Variant Details exclusion of gene-profile evidence and matching-transcript inspection | `web/src/app/variant-presentation.js`, `docs/transcript-and-evidence-selection.md` |
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
provenance therefore support **Gene matches** in RAM rather than creating a
placeholder section or persistent audit dataset.

## Deliberate limits and open decisions

A positive semantic-similarity score can be broad and is not a documented gene
association.
The corrected gene set therefore uses exact and descendant HPO annotation
relationships only. Resnik ranking remains separate and must pass its specified
fixtures before display. A high relative rank does not establish strong
absolute evidence: every combined query has a highest-ranked gene even when the
query is uninformative. Phenotype frequency, negative findings, and genotype
likelihoods are future ranking concerns, not gene-membership rules.

Turning on polygenic associations broadens retrieval; it does not upgrade
evidence or establish causality. `POLYGENIC` retains its literal installed-source
type throughout provenance. HPO `UNKNOWN` is deliberately excluded because the
summary file has flattened materially different Orphanet relationship types;
it is not treated as an unknown gene, a confidence tier, or an optional synonym
for polygenic evidence. Terms with no eligible association under either
polygenic-switch state remain zero; AnnoCAT does not invent membership from
Resnik similarity merely to make every autocomplete result nonzero.

Turning on upstream/downstream variants broadens allele retrieval, not the
resolved gene set. The 5 kb window reproduces a VEP annotation setting and is
not a claim that every included variant regulates the nearby gene. Such variants
can be biologically relevant, but their proximity term alone neither proves nor
rules out an effect. AnnoCAT therefore excludes proximity-only matches by
default while preserving the opt-in route and the underlying consequences for
inspection in Variant Details.

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

These rules preserve the existing Genes-popover structure while widening its
desktop presentation and making its state understandable and safe:

- Preserve the current compact progress copy: the footer button says
  **Resolving…** during preview and **Applying…** during apply, and the existing
  application task status says **Updating gene matches · N s** during the
  server update. If resolution lasts longer than 250 ms, the existing
  status/live region also announces **Resolving genes…** to assistive
  technology. No separate progress panel is added.
- A disabled **Apply** button has adjacent visible text explaining whether the
  cause is unresolved input, an outdated or in-progress preview, no associated
  genes, or zero current-result overlap. This text uses the existing inline
  scope/message area and is not available only on hover.
- The polygenic control uses the existing native `.fui-switch` pattern with
  `role="switch"` and the always-visible label **Include polygenic associations
  for HPO and MONDO**. It is off by default and uses the exact native-tooltip
  text defined above. The same text is available through an accessible
  description for keyboard and screen-reader users; no custom tooltip, tooltip
  icon, or persistent helper paragraph is added. Its checked state is not
  conveyed by color alone. A zero-count condition or pathway exposes its reason
  through visible **0 associated genes** text as well as
  `aria-disabled="true"`.
- The upstream/downstream control is a native checkbox with the always-visible
  label **Include upstream/downstream variants (VEP 5 kb)**. The label precedes
  the square, and the labeled control sits immediately before **Clear** in the
  footer action group. Its text and square are vertically centered against the
  text inside **Clear** and **Apply**, not aligned to the buttons' top edge. It
  remains separate from the HPO/MONDO polygenic switch. It is off by default and
  uses the exact native-tooltip text defined above, including **These variants
  can be biologically relevant, but proximity alone does not show that they
  affect the selected gene** and the plain-text documentation reference. The
  same complete text is exposed through an accessible description. No custom
  tooltip or tooltip icon is added.
- The desktop popover is `min(51.25rem, calc(100vw - 24px))` wide and retains a
  12 px viewport margin. Positioning, resizing, zoom to 200%, long canonical
  labels, item-specific bracket headings, and the four saved-list controls must
  not produce horizontal page scrolling, clipped content, or unreachable
  **Clear** and **Apply** actions.
- The existing compact **Gene matches** and **Phenotype rank** cells expose
  their tooltips on hover and keyboard focus, expose the same text to assistive
  technology, keep focus behavior predictable, and close with Escape. No new
  tooltip icon is added. **View N without variants** remains a real button and
  its existing dialog returns focus to the invoking button when closed.
- Clearing, changing a selection, or changing either scope control invalidates
  the pending fingerprint. Responses for older request fingerprints or control
  states are ignored, so a slow earlier request cannot overwrite the latest
  query. Closing the popover or clearing the query remains possible while
  resolution runs.
- Release qualification may record preview/ranking timing as telemetry. It is
  not a blocking threshold until a versioned fixture and a reproducible baseline
  from the same runner class have been checked in. Functional release gates must
  not depend on an invented local-to-CI timing conversion.

## Required regression coverage

Release qualification uses source-specific oracles rather than treating one
file or one patient cohort as an oracle for every domain. There is no published
single benchmark that can validate this whole mixed search workflow. The
literature supports a layered plan:

| Layer | Question answered | Required evidence |
|---|---|---|
| HPO, MONDO, and Reactome expansion | Did AnnoCAT reproduce the associations and mapping policy in the pinned sources? | Complete normalized set equality, counts, provenance, exclusions, and source hashes |
| HGNC resolution | Did each accepted identifier resolve to the correct stable identity without guessing through ambiguity? | Full accepted-namespace matrix against the pinned HGNC release, including previous, withdrawn, ambiguous, placeholder, and unknown cases |
| Resnik implementation | Did AnnoCAT calculate the declared formula exactly? | Hand-computed DAG fixtures, a separately implemented comparator, metamorphic invariants, exact score keys, ties, and ranks |
| Patient-case ranking | Does the fixed method place known causal genes plausibly high on solved monogenic cases? | PhEval-compatible frozen sentinels, cohort metrics, and phenotype-perturbation reports |
| Product integration | Did the correct resolver remain correct through the user workflow? | Browser/API equality from autocomplete through Apply, reopen, row filtering, sorting, and export |

[`SSSOM`](https://pmc.ncbi.nlm.nih.gov/articles/PMC9216545/) specifically
supports retaining ontology-mapping predicate, direction, provenance, and
version rather than treating exact, close, broad, narrow, and related mappings
as interchangeable. Research on invalid and outdated symbols, including
[`HGNChelper`](https://pmc.ncbi.nlm.nih.gov/articles/PMC7856679/), supports
testing historical and ambiguous inputs, but the pinned official HGNC release
remains AnnoCAT's identity oracle. Likewise, PhEval and patient-case papers are
ranking benchmarks, not substitutes for direct Reactome pathway-set equality or
HGNC identity equality.

The source-specific release tests are:

- HPO membership runs the full-release `phenotype_to_genes.txt` procedure above
  for every active phenotypic-abnormality term and both polygenic-switch states.
  Normal CI may use a checked-in miniature fixture, but the Windows release gate
  must run the pinned 66,907,216-byte oracle and compare complete normalized sets
  and exclusion ledgers.
- MONDO membership independently walks exact and descendant conditions in the
  pinned `mondo.json`, accepts only unambiguous `skos:exactMatch` disease
  mappings, joins those source disease IDs to the pinned HPO disease-gene file,
  applies both association-type states, and compares complete canonical gene
  sets, counts, and provenance. Fixtures must include exact, descendant, close,
  broad, narrow, related, deprecated-with-one-replacement, ambiguous, and
  unmapped cases.
- Reactome membership parses the pinned human pathway gene-set file directly
  in validation code and compares every pathway's distinct canonical identity
  set and autocomplete count with the production resolver. It does not infer
  genes from pathway names, hierarchy, or network proximity.
- HGNC identity validation covers every key namespace actually accepted by the
  product: approved HGNC symbol and ID, unique alias and previous symbol,
  current numeric NCBI Gene ID, versioned Ensembl gene ID, uniquely mapped
  withdrawn symbol, runtime-result identity, ambiguous key, unknown key, and a
  retired source identifier present in the HPO sentinel. It verifies canonical
  HGNC identity, approved display symbol, result-specific identity, status, and
  deterministic exclusion; a placeholder is never accepted as a gene.
- Resnik uses the hand-calculated and validation-only reference-comparator
  layers above. The frozen sentinel manifest and broader public patient-case
  cohort are separate plausibility layers. Neither membership oracle is allowed
  to provide expected rank values, and rank is never allowed to alter an oracle
  gene set.
- Patient-case manifest validation accepts only the predeclared solved,
  single-causal-gene cohort, applies only unique HPO and HGNC normalization,
  retains uncovered or poorly ranked eligible targets, rejects result-dependent
  exclusions, and produces a complete inclusion/exclusion ledger before ranking.
  The frozen report reproduces case-, gene-, and applicable disease-balanced
  metrics, 2,000 fixed-seed gene- and publication-cluster bootstrap summaries,
  and 1,000 fixed-seed target-label derangements. Every derangement changes each
  unique gene label, preserves the unique target-gene set, and uses the same
  worst-tie and missing-target rules; the observed gene-balanced mean reciprocal
  rank must pass the predeclared 95th-percentile null gate.
- Browser/API integration verifies that each source-level expected set remains
  the same through autocomplete count, preview, generated text, Apply, reopen,
  active consequence matching, visible filtering, sorting, and CSV export,
  subject only to the separately tested current-result intersection and VEP
  upstream/downstream checkbox. This catches a correct resolver connected to an
  incorrect UI or result-query path.

The maintained test contract includes:

- an exact HPO disease annotation produces an HPO link via exact disease
  annotation;
- a disease annotation below the selected HPO term produces an HPO link via
  more-specific disease annotation through the true-path rule;
- broader, sibling, and merely similar disease annotations do not add genes;
- with `includePolygenic: false`, HPO and MONDO membership uses only
  `MENDELIAN`; with it true, membership additionally uses `POLYGENIC`;
  `UNKNOWN`, other, and missing source types remain excluded in both states,
  and changing the switch never changes Resnik scores or the ranking corpus;
- three unrelated public HPO features produce distinct, non-universal gene
  counts through the preview endpoint under each polygenic-switch state;
- HPO and MONDO autocomplete `geneCount` equals the corresponding one-item
  source preview under both polygenic-switch states, Reactome counts are
  unchanged, and autocomplete performs no result-row scan or disk-cache write;
- a query shorter than two normalized search units leaves the listbox closed;
  a deliberately delayed current request exposes one nonselectable
  **Searching…** row; a successful empty response exposes one nonselectable
  **No matching feature, condition, pathway, or gene** row; each state is
  announced once through a polite status region, and matching options replace
  it without an artificial minimum delay;
- missing HPO/MONDO or Reactome data remains in the existing warning outside the
  listbox, request failures remain in the existing inline message area, and
  neither condition creates a duplicate autocomplete status row;
- a zero-count HPO feature remains selectable for a combined rank query, while
  zero-count MONDO conditions and Reactome pathways remain visible with
  **0 associated genes** and are disabled; a switch change enables an item only
  when its Mendelian-plus-polygenic count is positive;
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
- the current 0-100 Lin and absent-conflict values are not shown in the corrected
  live query, `phenotypeRelevance` is never reinterpreted as `phenotypeRank`, and
  genes without eligible profiles display **Not ranked**;
- MONDO exact and subtype conditions resolve to the documented Mendelian genes
  and, only when enabled, documented polygenic genes, while close, broad,
  narrow, related, ambiguous, and HPO `UNKNOWN` mappings do not add genes;
- Reactome results report and expand installed pathway gene-set entries;
- with `includeUpstreamDownstream: false`, a selected gene represented only by
  `upstream_gene_variant` or `downstream_gene_variant` does not retain the
  allele; coding, splice, UTR, intronic, non-coding-transcript, and mixed
  consequences still do; with the field true, the proximity-only allele is
  retained without changing the resolved gene set or phenotype rank;
- the AnnoCAT annotation command contains no `--distance` override, the exact
  pinned fastVEP build declares a 5,000 bp default, and an implicit-default
  boundary fixture distinguishes 5,000 bp from 5,001 bp on both transcript
  strands;
- the pasted-list endpoint resolves exact current and historical gene
  identities only; valid HPO, MONDO, and Reactome IDs remain unrecognized there
  and must be selected through typed autocomplete;
- autocomplete selections retain their Feature, Condition, Pathway, or Gene
  type when their generated gene preview is written into the textarea;
- generated previews create one whole-line bracket heading per exact selected
  item and eligible association type, include the canonical label and ID, keep
  Mendelian and polygenic sections separate, and deduplicate a gene that appears
  in more than one display section from the operative union; headings are
  ignored by the paste parser, inline annotations are not accepted as genes,
  and editing generated text drops typed provenance and enters manual-list mode;
- the Genes popover adds only the documented **Include polygenic associations
  for HPO and MONDO** switch at the top right of its header and **Include
  upstream/downstream variants (VEP 5 kb)** checkbox immediately before
  **Clear** in its footer, with no panel, persistent helper paragraph,
  unclassified-association control, association mode, or ranking selector; both
  are off by default,
  their native tooltips have the exact documented copy, and their accessible
  descriptions expose the same text; editing the textarea or choosing **Use
  list** replaces typed ontology selections with gene-only input without hiding
  or silently changing the upstream/downstream checkbox, **Save list** stores
  resolved genes without ontology-query provenance or checkbox state,
  **Delete** removes only the selected manual list, and **Clear** clears the
  active query/filter without deleting saved lists and resets both controls;
- the popover uses `min(51.25rem, calc(100vw - 24px))` on desktop, retains 12 px
  viewport margins and its existing maximum height, and keeps search, generated
  headings, saved-list controls, **Clear**, and **Apply** reachable without
  horizontal page scrolling at narrow widths and 200% zoom; the polygenic
  control remains top-right on a wide header, wraps between the heading and
  search field when necessary, and never moves beside the footer actions; on
  wide layouts the footer order is **upstream/downstream label, checkbox, Clear,
  Apply**, with the label and square centered on the buttons' vertical midpoint;
  at narrow widths the intact labeled checkbox wraps above **Clear** and
  **Apply** without separating its text from its square;
- current, historical, withdrawn, versioned Ensembl, ambiguous, and unknown
  HGNC inputs resolve as documented, and the active map retains the canonical HGNC
  ID separately from the result-specific identifier; ambiguous and unresolved
  associations do not enter the ranking denominator;
- changing any one source file without changing its release label changes the
  fingerprint because its SHA-256 changed;
- changing `includePolygenic` changes the fingerprint and invalidates the pending
  preview; a valid pre-switch schema-6 record with the field omitted defaults to
  `false`, while non-boolean values and legacy `includePolygenicAndUnknown` or
  `includeUnknown` request fields are rejected;
- changing `includeUpstreamDownstream` changes the fingerprint and recomputes
  current-result overlap without changing autocomplete association counts or
  the generated gene list; an earlier valid schema-6 record with the field
  omitted defaults to `false`, and a non-boolean value is rejected;
- a preview with no source-associated genes displays the specified installed-data
  message, disables **Apply**, and does not offer **View 0 without variants**;
  a preview with `N > 0` resolved genes but zero eligible current-result overlap
  under the upstream/downstream scope displays the distinct result-overlap
  message, disables **Apply**, and offers **View N without variants**; a direct
  zero-overlap apply is rejected without changing the active saved query or
  result rows, and a positive-overlap apply keeps `showMatchesOnly: true` and
  displays only eligible matches;
- **View N without variants** remains available for all-zero-result and partial
  overlap when `N > 0`, reports the correct absent count under the current
  upstream/downstream scope, updates when the checkbox changes, and opening it
  never applies or changes a filter;
- browser selections use union behavior, schema-6 requests containing a
  combination, absent terms, or gene exclusions are rejected, and no
  **any**/**every** or
  ranking-mode control appears in the Genes popover;
- schema 6 is the only saved Genes query format read or written; profile schema
  5 or earlier cannot restore a query or active filter, but the containing
  result still opens, the unsupported notice uses the existing popover message
  area without a modal, and the separate saved manual gene lists remain
  available;
- a schema-6 `phenotypes.json` record, including the `includePolygenic` and
  `includeUpstreamDownstream` booleans, round-trips through portable
  export/import without an evidence or catalog companion; an older profile is
  skipped without blocking the base result, retired phenotype-evidence roles
  are rejected, and a path, size, declared-file, or checksum failure still
  rejects the archive;
- schema 6 exposes no Monarch control, request field, stored result/error field,
  online-enrichment response field, service-catalog entry, endpoint call, or
  background network request;
- stale autocomplete and preview responses from a previous selection or scope-
  control state cannot replace the newest count, generated list, overlap, or
  fingerprint; disabled-state
  explanations use the existing inline message area; **Resolving…**,
  **Applying…**, and **Updating gene matches · N s** remain the visible progress
  copy; and both compact-column tooltips pass keyboard and screen-reader checks
  without adding tooltip icons;
- one multi-gene allele whose genes both match one selected pathway displays
  only that pathway label, not `+1`; adding a second distinct selected item adds
  `+1`, while transient details still retain both gene rows; mixed item types
  use the documented saved-query group and array order for the compact label
  and tooltip; and
- **Gene matches** reports the defined HPO or MONDO relation with `MENDELIAN` or
  `POLYGENIC`, or the defined Reactome or entered-gene relation, never displays
  an excluded HPO `UNKNOWN` row, and remains available while Variant Details
  never renders gene-profile support fields or a Gene associations section; an
  admitted proximity-only row names the matched gene, consequence, and different
  representative gene in its native tooltip, and the documented transcript-
  selector workflow exposes the matching consequence context;
- Apply, clear, reopen, FAVOR refresh, result filtering, rank sorting, visible
  CSV export, and portable packaging never create a phenotype evidence
  Parquet, phenotype field catalog, or phenotype query projection; and
- a selected gene present only on a qualifying nonrepresentative consequence
  still keeps the allele, displays and exports the correct **Gene matches**
  value, and participates in rank sorting; the same gene present only through
  an upstream/downstream consequence is excluded by default and retained only
  when `includeUpstreamDownstream` is true.

Verification includes the focused Rust phenotype tests, HGNC, MONDO, and
Reactome unit suites, browser phenotype tests, full Rust workspace, full web
suite, and formatting checks in `.github/workflows/ci.yml` (**Test**).
Small deterministic source fixtures, hand-calculated Resnik fixtures, the
validation-only Resnik comparator, frozen patient-case sentinels, and the
versioned public-cohort runner belong in that maintained test path. The separate
`.github/workflows/source-contract-validation.yml` workflow must stream and
validate the full pinned HPO membership oracle and the pinned MONDO, Reactome,
and HGNC contracts. It remains a release-packaging gate and does not replace
behavior or browser tests. The Windows release workflow must require both jobs
and record their exact source digests before publishing an asset.

These checks support four deliberately limited conclusions:

1. Full normalized set equality against the HPO, MONDO, Reactome, and HGNC
   source contracts shows that AnnoCAT returns the genes defined by its pinned
   sources and documented eligibility policy.
2. Hand fixtures and a nonproduction comparator show that AnnoCAT computes the
   declared Resnik formula, aggregation, denominator, quantization, and ranks.
3. Frozen case-level patient observations show that the declared method gives
   plausibly useful ranks on those predeclared examples, remains interpretable
   when genes and publications repeat, and outperforms the specified shuffled-
   target null rather than passing through a universal or popularity-only rank.
4. None of the above proves that every curated source association is causal,
   that a high-ranked gene explains a patient, or that AnnoCAT is clinically
   validated. The public-cohort confidence intervals are conditional on that
   frozen retrospective corpus. Clinical claims require a substantially larger,
   representative and independently curated evaluation, ideally a temporal or
   separately governed clinical holdout, with prespecified performance metrics.

No Lin/Jiang-Conrath/commercial-tool contest is required for this correction.
A different metric is considered only if later evidence shows that the fixed
Resnik method is inadequate; any change requires a new ranking-algorithm version
rather than silently changing existing rank meaning. A source update likewise
requires a reviewed oracle diff and new digests, not automatic acceptance of
changed gene counts.
