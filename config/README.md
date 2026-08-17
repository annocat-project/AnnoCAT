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

`source-overrides.example.json` documents optional local source overrides. Copy
it into runtime configuration rather than editing the tracked example.
