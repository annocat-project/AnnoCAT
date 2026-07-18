# fastVEP fork maintenance

AnnoCAT supports exactly the fastVEP fork commit recorded in
`config/fastvep-pin.json`. Data-source update discovery never updates the annotation
engine. A new engine is adopted only after its fork history, lockfile, tests, binary
identity, and AnnoCAT compatibility gates pass.

## Repository layout

- `Huang-lab/fastVEP` remains the upstream project.
- `annocat-project/fastVEP` is the public fork.
- The fork's `master` branch tracks upstream without AnnoCAT changes.
- Versioned `annocat/*` branches contain the reviewed AnnoCAT commits.
- AnnoCAT pins an exact fork commit SHA; it never builds a floating branch.

The ordered commit list in `config/fastvep-pin.json` is the compatibility and
provenance boundary. Packaging verifies that the fork commit descends from the pinned
upstream base and contains exactly those commits in that order.

## Local verification

Check out the pinned fork commit in a clean clone, then run:

```powershell
./scripts/test-fastvep-pin.ps1 -FastVepSource <path-to-fastVEP>
```

The script rejects the wrong commit, staged or modified tracked files, a changed
`Cargo.lock`, an unrelated upstream base, missing commits, reordered commits, or extra
commits. The packaging script runs the same verification automatically.

## Runtime impact

Most AnnoCAT fork commits affect source preparation, verification, sharding, or
cache-build locking and add no work to the normal annotation loop. Structured output
formats the already annotated in-memory result as newline-delimited JSON through a
buffered writer; it does not repeat transcript prediction, HGVS calculation, or fastSA
lookups.

On 2026-07-16, the Windows release build was measured on fastVEP's 1,003-record
`validation/human/chr22_1kgp.vcf` fixture using the same warmed transcript cache and
three warm runs per mode:

| Mode | Mean seconds |
| --- | ---: |
| Annotated VCF only | 10.106 |
| Annotated VCF plus structured sidecar | 10.522 |
| Observed overhead | 4.1% |

The resulting VCF was 3.15 MB and the sidecar was 5.06 MB. This is a small-fixture
result, not a WGS performance guarantee. The sidecar is staging-only and is removed
after AnnoCAT validates its Parquet conversion. The scale gates in `TODO.md` remain the
release measurements for runtime, disk throughput, output size, and peak memory.

## Updating fastVEP

1. Select and record a new upstream commit.
2. Create a new `annocat/*` branch from that commit; do not rewrite a released branch.
3. Cherry-pick, reimplement, or drop each AnnoCAT change as appropriate.
4. Submit generally useful changes upstream when practical.
5. Run fastVEP's locked workspace tests and AnnoCAT's tests and compatibility fixtures.
6. Build the Windows release executable and record its size and SHA-256.
7. Update the fork commit, upstream base, ordered changes, lockfile hash, and binary
   identity in `config/fastvep-pin.json`.
8. Repeat the real VCF-plus-structured-output and scale gates before changing the
   release default.

Pinned commits and released branches must never be force-pushed.
