# Filtering

Filters operate on the complete result. Each filter uses the stored value and
the field's declared type; the display label is not used as a substitute for a
missing source value.

## Categorical fields

Bounded categorical fields use a value picker. The list combines:

- values declared by the pinned field contract; and
- bounded values observed while the result was written.

Where available, the number beside a value is the count of result alleles that
would match that exact category. Counts are written with new local or online
annotation data. Older imported results can expose exact values without counts.

**Not reported** is a selectable missing-value state. It is not a source
category and is never treated as biological zero.

Exact matching is field-specific. Set-valued fields match complete members,
not substrings. For example, an exact ClinVar significance filter for
`Pathogenic` does not match a different category merely because its text also
contains that word.

## Text and numeric fields

Text operators support literal text values. Numeric operators parse finite
numbers and reject invalid input before a whole-result query starts. Multiple
values are accepted only for operators whose contract defines a list.

## Saved filters and gene lists

Saved filters retain field identifiers, operators, and values. If a field is
not present in another result, AnnoCAT reports the missing field instead of
silently applying the rule elsewhere.

An applied gene list appears as a normal filter in the Filters popover. Clearing
the list removes that filter and restores the remaining search and filter state.
