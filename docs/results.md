# Results

The Results page opens local AnnoCAT results and validated result ZIPs. A result
does not require the annotation sources that created it.

## Table

The table has one row per alternate allele. It supports:

- whole-result text search across displayed annotation fields;
- exact and numeric filters;
- multi-column sorting;
- configurable columns;
- gene, phenotype, condition, and pathway gene lists;
- row selection and candidate bookmarks; and
- export of the current filtered or selected set.

The viewer loads rows in pages. Search, filters, and sorting operate on the full
result, not only the rows currently rendered.

## Variant details

Select a row to review its representative gene and consequence, sample call,
transcripts, HGVS descriptions, source evidence, and provenance. Changing the
transcript changes evidence only when the source provides transcript-specific
values. Allele-level evidence remains unchanged.

Prediction colors and labels follow the
[computational evidence display policy](computational-evidence-display-policy.md).
They organize evidence and do not classify a variant.

## Genes

The Genes popover accepts symbols and supported identifiers. It can also expand
installed phenotype, condition, and Reactome pathway terms into gene lists.
Applying the list adds a normal table filter. It does not rank variants or
estimate causality.

Save a resolved list when it will be reused. Unresolved identifiers remain
visible so they can be corrected instead of being silently discarded.

## Online annotations

FAVOR annotations are requested only after you select **Get annotations**.
AnnoCAT sends the requested GRCh38 allele identifiers to FAVOR and stores the
returned fields with the result. Local evidence remains preferred when the same
logical field is already available from an installed source.

## Candidates, notes, and export

Candidates are manual bookmarks. Notes are local application data and are not
automatic interpretations. Confirm export contents before transferring a
result.

AnnoCAT result ZIPs include the result data, provenance, online annotations,
and candidate bookmarks declared by the package manifest. Notes are not part of
the result ZIP. Imported packages are validated before publication to Results.
