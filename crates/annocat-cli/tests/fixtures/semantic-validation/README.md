# Semantic validation fixture

This synthetic fixture checks AnnoCAT's own allele, transcript, evidence, query,
detail, and export contracts. It contains no patient data or copied source
records.

`expected.json` is authored independently from the conversion code. Change it
only when the declared semantic contract changes. Do not regenerate it from an
AnnoCAT result.

The fixture deliberately includes:

- zero and missing population frequencies;
- canonical and MANE transcripts with different REVEL and SpliceAI scores;
- exact ClinVar categories;
- called, homozygous-alternate, and missing genotypes; and
- PASS and non-PASS records.

Production-source and OSA1/OSA2 parity remain release validation checks because
those caches cannot be represented by synthetic source values alone.
