# AnnoCAT

Portable local variant annotation, curation, and review for Windows.

[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-4057d6.svg)](#system-requirements)
[![Release](https://img.shields.io/github/v/release/annocat-project/AnnoCAT?include_prereleases)](https://github.com/annocat-project/AnnoCAT/releases)

**AnnoCAT is a portable Windows application for annotating and exploring variants
from whole-genome, exome, or panel VCF files.** It combines gene consequences with
clinical, population, prediction, splicing, and conservation evidence in a
searchable results viewer.

AnnoCAT stores your VCF files and AnnoCAT results on your computer. AnnoCAT uses
the internet to download data sources, get online annotations, and open external
links.

Open an exported AnnoCAT result without installed annotation data. Before an
annotation, install the GRCh38 reference and the selected data sources. Extract the
release ZIP, and then double-click `launch-annocat.cmd`.

AnnoCAT starts from an existing VCF and does not process raw FASTQ, BAM, or CRAM
sequencing files.

## Features

- Annotate panel, exome, and whole-genome VCFs against GRCh38.
- Use Standard, Comprehensive, Core + online annotations, or Custom profiles.
- Manage data-source installation, retained fields, progress, updates, and removal.
- Search, sort, and filter large results without loading the entire dataset at once.
- Compare transcript consequences and transcript-specific evidence.
- Review clinical, population, prediction, splicing, conservation, and sample data.
- Star variants as candidates and export selected or filtered rows and gene lists.
- Export result ZIP files that open without locally installed data sources.

## Quick start

1. Download the latest Windows ZIP from
   [GitHub Releases](https://github.com/annocat-project/AnnoCAT/releases).
2. Extract the complete ZIP to a folder.
3. Double-click `launch-annocat.cmd`.
4. Keep the terminal window open while using AnnoCAT.

AnnoCAT opens in your default browser and runs only on your computer. The release
includes AnnoCAT, fastVEP, and all required software libraries. You do not need
Rust, Python, Node.js, Docker, or a separate database server.

On first launch, choose one of two paths:

- **Open results** to open an AnnoCAT result. Annotation data is not required.
- **Set up annotation** to install the GRCh38 reference and an annotation profile.

## How it works

```text
VCF file
   ↓
Gene and transcript consequences
   ↓
Clinical, population, prediction, splicing, and conservation evidence
   ↓
Searchable local result
   ↓
Review, candidates, and export
```

AnnoCAT uses the bundled fastVEP engine to identify affected genes and transcripts,
predict variant consequences, and generate HGVS descriptions. Installed annotation
sources then add matching clinical records, population frequencies, identifiers,
prediction scores, splicing scores, and conservation measurements.

Completed annotations are stored in a compact indexed result. The viewer requests
only the rows needed for the current view, search, or filter, allowing large WGS
results to remain responsive without loading the complete dataset into browser
memory.

## Annotation profiles

| Profile | Intended use | Included evidence |
|---|---|---|
| **Standard** | General review with a smaller setup | Core GRCh38 annotation, ClinVar, dbSNP, gnomAD exomes, PhyloP, and standalone REVEL |
| **Comprehensive** | Broader WGS investigation | Core GRCh38 annotation, dbNSFP, ClinVar, dbSNP, gnomAD exomes, CADD, PhyloP, and SpliceAI |
| **Core + online annotations** | Annotation with remote FAVOR data | Core GRCh38 annotation and FAVOR annotations for selected or matching variants |
| **Custom** | User-selected sources | Any compatible combination of installed sources |

AnnoCAT shows the download size and installed size before installation. Use **Data
sources** to install, configure, update, or remove data sources. Comprehensive gets
REVEL evidence from dbNSFP.

## Annotating a VCF

1. Open **New annotation**.
2. Select one or more `.vcf`, `.vcf.gz`, or `.vcf.bgz` files.
3. Select a profile.
4. Review the selected data sources and output folder.
5. Start the annotation and follow progress from the status area or **Tasks**.
6. Open the AnnoCAT result under **Results**.

Each input VCF produces a separate annotation and AnnoCAT result. AnnoCAT does not
combine VCF files or samples.

## Exploring results

The variants table shows one row per alternate allele and begins with a compact set
of useful columns. Click a column heading to sort it, or use **Columns** to show,
hide, resize, and reorder core or source-specific fields.

Use the search box for quick gene, variant, consequence, identifier, or annotation
lookups. For precise queries, add structured filters using `=`, `≠`, `>`, `≥`, `<`,
`≤`, and text operators. Filters can also accept comma-separated gene lists and can
be saved for use with other results.

For example:

```text
gnomAD allele frequency < 0.001
Impact = HIGH
Gene is in BRCA1, BRCA2, PALB2
```

Click a variant to open its details. The transcript selector updates the consequence,
HGVS descriptions, protein change, transcript support, and transcript-specific
predictions together. Variant-level evidence such as ClinVar, dbSNP, and population
frequency remains associated with the genomic allele. Source descriptions and
tooltips explain what fields mean and how scores are generally interpreted.

Colors and prediction scores help organize evidence; they are not diagnoses or final
pathogenicity classifications.

### Candidates and exports

- Click a star in the table or variant details to add or remove a candidate.
- Candidates are manual bookmarks, not automatically classified variants.
- Select individual variants, a range, or every result matching the current filters.
- Export selected or filtered rows using the visible columns.
- Export selected or filtered genes as a comma-separated gene list.

## Exporting results

Export an AnnoCAT result as a ZIP file. This file contains the searchable
variants, annotations, provenance, online annotations, and candidate bookmarks.

Imported results appear under **Results**. Candidates are included. Notes stay on
the computer that created them.

## Storage and privacy

Variant inputs and AnnoCAT results stay on your computer unless you export them.
AnnoCAT uses the internet to download data sources and get online annotations.
**Settings** shows the annotation data, download, and results folders.

Installed data and saved results use dedicated directories whose locations appear
in **Settings**. Preserve those directories when moving or manually updating a
portable installation. Large WGS data sources can require substantial network
transfer and storage; AnnoCAT shows available size information before installation.

## Command line

Run `annocat --help` for the complete command reference. The CLI uses the same
configuration, annotation data, results, validation, and recovery workflows as the
desktop application.

```text
annocat status --profile standard
annocat sources install --profile standard
annocat annotate -i sample.vcf.gz --profile standard
annocat results list
annocat results export RESULT_ID -o result.zip
annocat tasks list
```

Use repeated `-i` options to annotate a batch in sequence. Use repeated `--source`
options instead of `--profile` for an exact source selection. Add `--json` to
supported read commands for machine-readable output.

## System requirements

- 64-bit Windows 10 or Windows 11.
- GRCh38 VCF input for the first release.
- Sufficient storage for the selected data profile and annotation results.
- Internet access for initial source installation and source updates.
- No internet connection or annotation data is required to view a local AnnoCAT
  result.

## Intended use

AnnoCAT is intended solely for research and educational annotation, exploration,
filtering, and visualization of genomic variants. It has not received regulatory
clearance or approval for diagnostic or clinical use.

**For research use only. Not for use in diagnostic procedures.** Do not use AnnoCAT
or its outputs for diagnosis, screening, prognosis, treatment selection, or other
patient-care decisions. Results may be incomplete or incorrect, depend on third-party
data sources and computational predictions, and must not be treated as validated
clinical findings.

## Development

End users should use the Windows release and do not need a development environment.
Building from source is intended for contributors and requires a Rust toolchain.

```text
cargo test --workspace
cargo run -p annocat-cli -- launch
```

## License and acknowledgments

AnnoCAT is licensed under the [Apache License 2.0](LICENSE).

AnnoCAT bundles a modified Apache-2.0 fastVEP build. Downloaded reference and
data sources remain subject to their publishers' separate licenses, permitted
uses, and citation requirements.
