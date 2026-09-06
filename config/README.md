# Configuration contracts

This directory contains versioned contracts used to build and interpret an
AnnoCAT release. Runtime settings and saved user profiles are ignored by Git.

The tracked files define:

- the exact fastVEP source and binary identity;
- annotation source releases and profiles;
- dbNSFP members and retained fields;
- supplementary-source fields and adapters;
- indexed and whole-genome source layouts;
- evidence calibration and presentation rules; and
- phenotype, condition, gene-identity, and pathway assets.

Treat these files as code. Update the corresponding parser, tests, provenance,
and migration behavior when a contract changes. Do not place credentials,
machine-specific paths, or private data in this directory.

`scripts/verify-configured-urls.py` checks every configured public URL and its
declared byte count in source-contract validation. It also streams every asset
with an explicit SHA-256, currently the pinned HPO, MONDO, and HGNC assets, and
verifies its full contents. Runtime dependencies block a release when
unreachable; citation and provider links are reported as advisory. Service
endpoints and object-store prefixes use protocol-aware checks rather than
requiring every URL to behave like a downloadable file.

The same workflow runs the application's production rolling-source resolver
against its official endpoints. Rolling discovery selects an update; every
installed source remains a versioned, checksum-verified snapshot.

`source-overrides.example.json` documents optional local source overrides. Copy
it into runtime configuration rather than editing the tracked example.
