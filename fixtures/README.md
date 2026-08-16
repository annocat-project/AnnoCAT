# Synthetic fixtures only

Test fixtures in this directory must be fabricated and must never contain a
user's genomic data. The executable browser prototype currently generates its
small `DEMO1`/`DEMO2`/`DEMO3` dataset from `annocat-core`.

`source-cache-parity` contains synthetic source rows and their independently
authored expected annotations. It verifies exact OSA1/OSA2 parity for every
supplementary source adapter used by an AnnoCAT release.
