# AnnoCAT

Portable local variant annotation, curation, and review for Windows.

[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-4057d6.svg)](#system-requirements)
[![Release](https://img.shields.io/github/v/release/annocat-project/AnnoCAT?include_prereleases)](https://github.com/annocat-project/AnnoCAT/releases)

**AnnoCAT is a portable Windows application for annotating and exploring variants
from whole-genome, exome, or panel VCF files.** It combines gene consequences with
clinical, population, prediction, splicing, and conservation evidence in a
searchable results viewer.

AnnoCAT is local-first: your VCF files, annotations, and reports are processed and
stored on your computer rather than uploaded to an online service. Internet access
is used only when downloading reference and annotation data sources or following an
external link.

You can immediately open a shared AnnoCAT report ZIP without installing annotation
data. To annotate your own VCF, AnnoCAT must first download and prepare the selected
GRCh38 reference and annotation data sources. Download and extract the release ZIP,
then double-click `launch-annocat.cmd`—there is no installer or separate runtime.

AnnoCAT starts from an existing VCF and does not process raw FASTQ, BAM, or CRAM
sequencing files.

## Features

- Annotate panel, exome, and whole-genome VCFs against GRCh38.
- Use Minimal, Comprehensive, or Custom annotation profiles.
- Manage data-source installation, retained fields, progress, updates, and removal.
- Search, sort, and filter large results without loading the entire dataset at once.
- Compare transcript consequences and transcript-specific evidence.
- Review clinical, population, prediction, splicing, conservation, and sample data.
- Star variants as candidates and export selected or filtered rows and gene lists.
- Share report ZIPs that can be opened without locally installed annotation sources.

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

- **Open existing results** to open an AnnoCAT report ZIP immediately. Annotation
  data sources are not required.
- **Set up local annotation** to install the required GRCh38 reference data and an
  annotation profile before selecting your own VCF.

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
Review, candidates, export, and sharing
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
| **Minimal** | General review with a smaller setup | Core GRCh38 annotation, ClinVar, dbSNP, gnomAD exomes, PhyloP, and standalone REVEL |
| **Comprehensive** | Broader WGS investigation | Core GRCh38 annotation, dbNSFP, ClinVar, dbSNP, gnomAD exomes, CADD, PhyloP, and SpliceAI |
| **Custom** | User-selected sources | Any compatible combination of installed sources |

AnnoCAT shows the expected network transfer and available cache information before
installation. Sources and their retained fields can be installed, configured,
updated, or removed from **Data sources**. Comprehensive obtains REVEL evidence
through dbNSFP rather than installing the standalone REVEL source a second time.

## Annotating a VCF

1. Open **New annotation**.
2. Select one or more `.vcf`, `.vcf.gz`, or `.vcf.bgz` files.
3. Choose Minimal, Comprehensive, or Custom.
4. Review the selected data sources and output folder.
5. Start the annotation and follow progress from the status area or **Tasks**.
6. Open the completed result under **Browse results**.

Each input VCF becomes a separate annotation run. AnnoCAT does not combine multiple
VCFs or samples automatically.

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

## Sharing results

Completed annotations can be exported as an AnnoCAT report ZIP. The ZIP preserves
the searchable result and its source information, allowing another AnnoCAT user to
open it without downloading annotation databases.

Imported reports are copied into the local results library and appear under
**Browse results**. Private local case notes are not included in shared reports.

## Storage and privacy

Variant inputs and reports remain on your computer unless you explicitly export or
share them. Network access is used to download reference and annotation sources.
Resource, download, and results directories are shown in **Settings** and may be
placed on another drive.

Installed data and saved results use dedicated directories whose locations appear
in **Settings**. Preserve those directories when moving or manually updating a
portable installation. Large WGS annotation sources can require substantial network
transfer and storage; AnnoCAT shows available size information before installation.

## System requirements

- 64-bit Windows 10 or Windows 11.
- GRCh38 VCF input for the first release.
- Sufficient storage for the selected data profile and annotation results.
- Internet access for initial source installation and source updates.
- No internet connection or annotation sources are required to view an existing
  local AnnoCAT report.

## Intended use

AnnoCAT supports variant annotation and professional review. It has not been
independently validated or authorized to establish a clinical diagnosis. Results
should be reviewed by qualified professionals and confirmed through an appropriately
validated workflow before patient-care decisions are made.

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
annotation sources remain subject to their publishers' separate licenses, permitted
uses, and citation requirements.
