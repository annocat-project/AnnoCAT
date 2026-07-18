# fastVEP smoke fixture

This fixture comes from the pinned Apache-2.0 fastVEP source revision recorded in
`manifest.json`. `expected.vcf` was generated on Windows with AnnoCat's pinned
binary and contains eight annotated records, 48 loaded transcripts, and a
49-field dynamic CSQ schema.

The generated `.fastvep.cache` sidecar is intentionally not committed. Smoke tests
must build it in a temporary directory so a stale cache cannot mask a cache-building
regression.

`giab-hg002-cmrg-grch38.vcf` contains the first 40 records from the NIST
Genome in a Bottle HG002 GRCh38 CMRG v1.00 small-variant benchmark. It is a
small real-world genotype fixture covering phased genotypes, allele depths,
indels, and a multiallelic record. The exact source URL is recorded in the VCF
header.
