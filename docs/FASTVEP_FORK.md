# fastVEP fork maintenance

AnnoCAT supports the exact fastVEP revision recorded in
[`config/fastvep-pin.json`](../config/fastvep-pin.json). Source update checks do
not update the annotation engine. A new engine is adopted only with a reviewed
pin change.

## Repositories and pin

- [Huang-lab/fastVEP](https://github.com/Huang-lab/fastVEP) is upstream.
- [annocat-project/fastVEP](https://github.com/annocat-project/fastVEP) contains
  AnnoCAT's maintained changes.
- `config/fastvep-pin.json` records the exact fork commit, upstream base,
  ordered changes, dependency-lock hash, and expected Windows artifact.

The branch name in the pin is descriptive. The commit SHA is the build and
compatibility boundary. Release packaging must not build a floating branch.

## Verify a checkout

Check out the pinned commit in a clean fastVEP clone, then run:

```powershell
./scripts/test-fastvep-pin.ps1 -FastVepSource <FASTVEP_SOURCE>
```

The check rejects a wrong commit, modified tracked files, a changed lockfile,
missing ordered changes, or an unrelated upstream base. The Windows packaging
script performs the same check before it builds `fastvep.exe`.

## Maintained differences

The complete ordered list belongs in the pin, not in prose that can drift away
from the code. At a high level, the maintained fork adds:

- streaming builders for AnnoCAT's managed supplementary sources;
- strict OSA1 and OSA2 validation and compatibility;
- lossless record lists for transcript- and gene-scoped evidence;
- bounded parsing, compression, cache, and memory behavior;
- deterministic parallel supplementary-source loading and lookup;
- structured output without duplicate source lookup;
- transcript-cache integrity and read-only runtime safeguards;
- consequence and HGVS corrections validated against Ensembl 115; and
- aggregate performance diagnostics that do not record variant values.

These changes must preserve the declared source field, allele, gene,
transcript, and missing-value contracts. A cache that can be opened is not
necessarily semantically compatible.

## Update procedure

1. Select and record the new upstream base.
2. Reapply or replace each maintained change without rewriting released
   history.
3. Run the locked fastVEP workspace tests.
4. Run AnnoCAT unit, integration, source-parity, and consequence-concordance
   tests.
5. Build the Windows artifact and record its SHA-256 and size.
6. Update every identity and ordered change in `config/fastvep-pin.json`.
7. Run the packaged end-to-end annotation gate before changing the release
   default.

Do not force-push a commit referenced by a released AnnoCAT pin.
