# Data and privacy

AnnoCAT is local-first. VCF inputs, prepared annotation caches, results,
candidates, notes, settings, and disposable query caches are stored in the
selected portable AnnoCAT home unless you export or move them.

## Network use

AnnoCAT uses the network when you request one of these actions:

- install, verify against a remote release, or update an annotation source;
- install phenotype, condition, gene-identity, or pathway knowledge data;
- get FAVOR annotations for selected or matching GRCh38 alleles;
- check a rolling source for updates; or
- open an external source or documentation link.

FAVOR requests contain the allele identifiers required by its API. Do not use
online annotation for data that cannot be sent under the applicable policy.
Local annotation profiles do not send VCF records to FAVOR.

## Results and transfers

A result ZIP is a transfer package. It can contain variant data, annotations,
provenance, online annotations, and candidate bookmarks. Treat it as sensitive
data. AnnoCAT validates imported packages for structure and integrity, but a
valid package is not proof that its author or biological assertions are
trustworthy.

Notes are local application data and are not included in result ZIPs. Review an
export before transfer and use the receiving organization's approved storage
and transfer controls.

## Third-party terms

Downloaded references, annotation sources, ontologies, and pathways remain
subject to their publishers' licenses, permitted uses, and attribution rules.
The Data sources page identifies the configured release and links to available
terms. CADD and other restricted sources can impose additional use limits.

AnnoCAT is for research and educational use only. See the intended-use notice
in the [project README](../README.md#intended-use).
