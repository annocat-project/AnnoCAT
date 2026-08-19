# Annotation

AnnoCAT accepts GRCh38 VCF, VCF.GZ, and BGZ files. It creates one result for
each input file and does not merge samples or VCF files.

## Before annotation

- Confirm that the input uses GRCh38 coordinates.
- Keep the VCF header and sample columns intact.
- Install core annotation data.
- Install every local source required by the selected profile.

If a header names a custom reference file but does not identify the assembly,
AnnoCAT can ask you to confirm GRCh38. Confirmation does not convert coordinates
or repair alleles.

## Profiles

| Profile | Included annotation |
|---|---|
| **Standard** | Core, ClinVar, dbSNP, gnomAD exomes, PhyloP, and REVEL |
| **Comprehensive** | Core, dbNSFP, ClinVar, dbSNP, gnomAD genomes, CADD, PhyloP, and SpliceAI |
| **Core annotation** | Core annotation with FAVOR available after the result opens |
| **Custom** | The installed sources selected for this run |

Core annotation uses the pinned GRCh38 reference, Ensembl transcript data, and
fastVEP revision declared by the release. Source releases and field contracts
are recorded with the result.

## Output

Each alternate allele becomes one result row. AnnoCAT keeps all supported
consequences and selects representative gene and transcript values for the main
table. Source evidence remains associated with its biological scope so a
transcript-scoped value is not reused as an allele-wide value.

The result includes canonical Parquet data, a field catalog, provenance,
integrity metadata, and disposable query caches. Select **Keep annotated VCF**
only when that additional output is needed.

## Interrupted work

AnnoCAT records recoverable progress. Use **Tasks** or `annocat tasks list` to
inspect unfinished work. Resume continues from the last aligned complete VCF
and structured record. An incomplete trailing record is discarded and read
again; complete records are not skipped.

Cancel removes the partial task data. It does not remove installed annotation
sources or completed results.
