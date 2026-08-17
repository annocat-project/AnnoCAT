# Test fixtures

Fixtures in this directory must be fabricated or approved public test data.
They must never contain a user's genomic data or machine-specific paths. The
small browser demo uses fabricated `DEMO1`, `DEMO2`, and `DEMO3` records from
`annocat-core`.

`source-cache-parity` contains synthetic source rows and independently authored
expected annotations. It verifies exact OSA1/OSA2 parity for every
supplementary-source adapter used by a release.

Public fixtures must record their origin, license or permitted use, assembly,
and any transformation applied before commit.
