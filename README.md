# AnnoCat

AnnoCat is a local-first WGS variant annotation application. Its primary engine is
fastVEP, packaged as a native portable Windows binary. AnnoCat adds managed
sources, reproducible profiles and provenance, resumable downloads, exports, and
an interactive local results browser.

This repository contains the executable foundation: shared domain contracts, a
native CLI, environment diagnostics, a managed source catalog, streaming
preparation jobs, real fastVEP annotation runs, and a local results browser.

The Comprehensive profile prepares dbNSFP 4.9a, ClinVar, dbSNP, gnomAD exomes v4.1.1,
CADD v1.7, UCSC hg38 phyloP100way, and SpliceAI without retaining permanent copies of their
source archives. By default, source bytes stream directly into fastVEP and are not
retained as temporary source files. An optional hybrid-resumable setting can retain
the current compressed chromosome or indexed range for users who prefer network
resume over minimum disk use. gnomAD and phyloP use publisher chromosome objects; CADD and
the public Ensembl SpliceAI MANE Select v1.4 masked GRCh38 SNV scores use publisher
tabix indexes to request only one chromosome range at a time. Completed OSA shards
survive cancellation and restart. Ensembl does not publish the SpliceAI indel scores;
the separate account-gated Illumina indel dataset is not required by the unattended
Comprehensive profile.

Full gnomAD genomes v4.1.1 is available as a separate optional 526.80 GiB
network stream for users who need population frequencies outside exome-covered
regions. It is not included in either recommended profile, and exomes and genomes
are mutually exclusive for an annotation run. In the default pure-streaming mode,
an interrupted current chromosome restarts from its beginning while previously
verified chromosome caches remain intact.

The gnomAD builder does not copy the full VCF INFO payload into AnnoCat. It keeps
only allele frequency, allele count, allele number, homozygote count, and the
available ancestry-specific allele frequencies needed for population filtering
and ACMG frequency evidence. Coordinates and alleles are retained for lookup;
unrelated gnomAD INFO fields are discarded while streaming. This bounds the
prepared schema, although the final OSA size is measured from each completed
chromosome rather than promised from the compressed download size.

dbNSFP 4.9a exposes its curated retained-field groups before installation from
both the individual source card and recommended-profile review. Variant and
transcript linkage fields remain mandatory; optional prediction, loss-of-function,
conservation, and domain fields can be omitted to reduce the prepared cache. The
chosen field-set fingerprint is recorded in every chromosome checkpoint, so an
install cannot resume using shards built with a different selection. New installs
default to fastVEP's SIFT and PolyPhen fields plus REVEL, AlphaMissense, PrimateAI,
CADD PHRED, phyloP100way, and GERP++. SpliceAI remains a separate WGS source so its
scores are not duplicated in the dbNSFP cache. Existing full-field caches retain
their original identity and remain resumable.

Source preparation concurrency is shared by profile installs and sources added
individually. The default runs one streaming fastVEP builder at a time; Settings
can enable two concurrent builders on more powerful computers. Each source keeps
independent progress and cancellation state, and a paused source reserves its
scheduler slot until it is resumed or deleted.

Every managed supplementary fastVEP source also exposes its retained fields before
installation, both on its source card and in profile review. The current parser field
set remains the default. Additional supported fields may be enabled from the same
editor, but arbitrary names are rejected: each source has a pinned, versioned allowlist
that is parsed and tested by the bundled fastVEP build. A custom selection becomes part
of the cache identity, preventing shards built with different schemas from being mixed.
Installed caches must be removed before changing their retained-field selection.

REVEL v1.3 is also available as an optional managed source for either profile.
AnnoCat processes its 24 official chromosome ZIPs from Zenodo, inflates their CSV
members into fastVEP, and verifies the publisher MD5 and ZIP CRC. Temporary ZIP
parts are deleted after verification. It is intentionally not part of either one-click
profile because dbNSFP already includes REVEL fields; install the standalone source
when its complete v1.3 transcript-level release is preferred.

## Quick start

```text
cargo run -p annocat-cli -- doctor
cargo run -p annocat-cli -- fastvep status
cargo run -p annocat-cli -- sources
cargo run -p annocat-cli -- serve --port 8787
cargo run -p annocat-cli -- interactive
```

Then open `http://127.0.0.1:8787`. The server binds only to localhost.

Large databases, downloads, run outputs, and genomic inputs are intentionally
excluded from Git.

## Completed results

Every successful annotation is built in a unique staging directory and published
only after its temporary fastVEP VCF and canonical result validate together. A
completed run contains typed `variants.parquet`, `consequences.parquet`,
`evidence.parquet`, `field-catalog.json`, fastVEP logs, and an immutable
`manifest.json` with checksums and source identities. Schema version 1 stores one
core row per alternate allele and one row per transcript consequence. Supplementary
database fields are namespaced and retain their JSON scalar types; arrays and nested
values remain lossless JSON. The complete original structured consequence is also
retained, so unknown future fastVEP or database fields are not silently dropped.

fastVEP writes a temporary annotated VCF and newline-delimited structured sidecar
in one annotation pass. Consequence prediction and fastSA database lookups are not
repeated. AnnoCat ingests the sidecar incrementally into typed Parquet, validates the
result, and deletes both temporary files before publishing the completed run. Use the
CLI `--annotated-vcf` option when a retained annotated VCF is specifically needed.

The browser asks the localhost API for bounded pages from `variants.parquet`.
DuckDB is statically bundled in `annocat.exe`; there is no separate installation,
server, connection setup, or permanent DuckDB database file.

Windows release bundles include both `annocat.exe` and the pinned
`tools/fastvep/fastvep.exe`; users do not install fastVEP separately. Maintainers
produce the bundle with `scripts/package-windows.ps1`, which verifies the upstream
commit and Cargo lock, runs both projects' tests, records checksums and licenses,
and creates one portable ZIP.

Fork upgrade policy, exact commit verification, and measured structured-output
overhead are documented in [docs/FASTVEP_FORK.md](docs/FASTVEP_FORK.md).

See [TODO.md](TODO.md) for the fastVEP integration and validation roadmap.
