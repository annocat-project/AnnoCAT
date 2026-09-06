# Transcript and evidence selection

AnnoCAT preserves source evidence at the narrowest scope supported by that
source. A table value is selected for convenience; it does not change the raw
evidence retained in the result.

## Evidence scopes

- **Allele-level** evidence applies to the exact normalized alternate allele.
  Examples include ClinVar, dbSNP, gnomAD, CADD, and PhyloP.
- **Gene-level** evidence applies to a gene identity. Pathway and condition
  links use this scope.
- **Transcript-level** evidence applies to one transcript or aligned protein.
  REVEL and transcript-vector dbNSFP fields use this scope when the source
  provides the required identity.
- **Gene-scoped feature evidence** can depend on a gene but not a transcript.
  The installed SpliceAI source uses this scope.

Online FAVOR fields retain the scope supported by the response. A coding score
without a contributing transcript is not relabeled as transcript-specific.

## Representative feature selection

AnnoCAT uses the versioned `allele-gene-severity-v1` contract. It first groups
transcript consequences by stable gene ID. When a stable gene ID is absent, an
unambiguous gene symbol can identify the group. Regulatory, motif, and other
features are grouped separately by stable feature ID.

Within each gene or feature group, AnnoCAT selects one representative in this
order:

1. MANE Select.
2. MANE Plus Clinical.
3. Ensembl canonical.
4. APPRIS rank.
5. Transcript support level, with the lowest reported level preferred.
6. Protein-coding biotype.
7. CCDS membership.
8. Sequence Ontology consequence severity.
9. Longest translated, transcript, or feature length available.
10. Stable transcript or feature ID.
11. Source ordinal as the final deterministic fallback.

AnnoCAT then selects the allele's displayed representative from those
gene-level representatives in this order:

1. Sequence Ontology consequence severity.
2. MANE Select, then MANE Plus Clinical.
3. Ensembl canonical.
4. APPRIS rank.
5. Transcript support level.
6. Protein-coding biotype.
7. CCDS membership.
8. Longest translated, transcript, or feature length available.
9. Stable gene ID.
10. Stable transcript or feature ID.
11. Source ordinal as the final deterministic fallback.

This differs from selecting one transcript for the entire allele before genes
are considered. Gene-first grouping prevents a severe consequence in one gene
from disappearing because an unrelated transcript in another gene has higher
transcript preference.

## Variant Details transcript selector

The main result table contains one row per alternate allele and displays the
representative gene and transcript chosen by the rules above. That display
choice does not discard the allele's other retained transcript consequences.
Variant Details initially opens on the representative context, and its existing
transcript selector lets the user inspect the other consequence contexts for
the same allele.

A Genes query matches against eligible consequences for every resolved gene,
not only the representative gene displayed in the table. A result row can
therefore display one gene while **Gene matches** identifies another selected or
association-derived gene. To inspect that match, open Variant Details and use
the transcript selector to choose a transcript belonging to the matched gene.
The detail view then uses that transcript's consequence and matching
transcript-scoped evidence. Changing the selector does not change the allele's
filter membership, the representative values in the main table, or the saved
Genes query.

By default, an allele is not retained for a selected gene when that gene has
only an `upstream_gene_variant` or `downstream_gene_variant` consequence. If the
user applies **Include upstream/downstream variants (VEP 5 kb)**, those
proximity-only matches become eligible. The main table may still display a
different representative gene. The user must choose a transcript for the
matched gene in Variant Details to inspect the upstream/downstream annotation.

The 5 kb distance is the default of AnnoCAT's exact pinned fastVEP revision;
AnnoCAT does not currently override it on the annotation command. Release
validation checks that implicit boundary. These variants can be biologically
relevant, but proximity alone does not show that they affect the selected gene.
Transcript selection shows the retained annotation context; it does not
establish causality, pathogenicity, or regulatory effect. The checkbox,
tooltip, saved-query field, and filtering rules are defined in
[Phenotype, condition, pathway, and gene resolution](phenotype-and-gene-resolution.md#upstreamdownstream-variant-checkbox).

## Query consistency

Display, search, filtering, sorting, Variant Details, and export use the same
selected-value resolver for a logical field. The resolver uses a selected row
written with new results when available. Older results use the compatibility
resolver and can build a disposable per-field projection on first use.

If the requested transcript has no matching source value, AnnoCAT can use only
a documented source-specific fallback. It does not copy a value from another
transcript merely to avoid a missing result. Missing remains distinct from
zero.
