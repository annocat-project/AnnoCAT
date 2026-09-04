# Track upstream fastVEP changes

This document records how AnnoCAT evaluates fastVEP changes after the upstream
revision on which the maintained fork is based. It is a decision ledger, not a
build manifest. [`config/fastvep-pin.json`](../config/fastvep-pin.json) remains
the authoritative source for the fastVEP commit included in an AnnoCAT build.

## Current boundary

| Item | Revision |
| --- | --- |
| Upstream repository | [Huang-lab/fastVEP](https://github.com/Huang-lab/fastVEP) |
| Upstream baseline | [`0e13c5b`](https://github.com/Huang-lab/fastVEP/commit/0e13c5bdb92f22a5f780cf314699f8402fb8383e), fastVEP 0.3.0 |
| AnnoCAT fork | [annocat-project/fastVEP](https://github.com/annocat-project/fastVEP) |
| Current AnnoCAT pin | `78c870be81762f8cec0a020a76a0515cfdd1449c` |
| Latest upstream revision reviewed | [`8c6b179`](https://github.com/Huang-lab/fastVEP/commit/8c6b179f196a6dbfd60eb3773c1d7d69950db355) |
| Review date | 2026-09-03 |

Every upstream commit through the baseline is inherited. The tables below cover
every later upstream commit through the reviewed revision.

Status meanings:

- **Implemented**: the relevant behavior is present in the fork, although the
  upstream commit might have been ported rather than cherry-picked.
- **Superseded**: AnnoCAT implements the requirement differently because its
  cache or result contract differs from standalone fastVEP.
- **Partial**: some behavior is present and the remaining relevant work is
  identified below.
- **Planned**: relevant runtime behavior still needs to be ported and tested.
- **Deferred**: correct upstream behavior that the current AnnoCAT invocation
  cannot use.
- **Skipped**: documentation, packaging, benchmarks, or interfaces outside the
  AnnoCAT product boundary.

## Implemented update

The fork ports these changes without altering the AnnoCAT result schema or
existing OSA2 formats:

1. The consequence and HGVS corrections from `9266d04`, `f778cff`,
   `01fad02`, and `123995f` as one tested series. This includes CDS/UTR boundary
   terms, whole-span splice evaluation, sequence-derived delins consequences,
   exonic and intronic HGVS 3-prime normalization, and every applicable coding
   term.
2. Supplementary-source error accounting and failed-chunk memoization from
   `9266d04`. A declared source read error now produces an explicit warning and
   a failed chunk is not repeatedly decoded as an ordinary annotation miss.
3. The schema-neutral hot-path changes from `f162f9f`: test for a matching
   supplementary payload before allocating projection state, resolve CSQ field
   writers once, use the predictor's transcript order before falling back to a
   scan, and fast-path CSQ strings that need no escaping.
4. The upstream regression tests for each adopted behavior, plus AnnoCAT
   integration coverage for the CLI path it invokes.
   This includes the frameshift terminator-distance test from `99b1275` and
   adapted OSA/OSA2 round-trip, long-variant, and streaming-writer tests from
   `e47216c`. Update the fastVEP fork's CI to run the complete workspace test
   suite rather than `cargo test --workspace --lib`, which skips those tests.

The changes were ported rather than cherry-picked because upstream split the CLI
pipeline after the fork diverged. The fork retains its batching, structured
output, profiling, source loading, and both annotation paths.

No other post-baseline upstream runtime change should be ported for the current
AnnoCAT configuration. The remaining commits affect standalone fastVEP
interfaces, unsupported inputs, packaging, benchmarks, or cache formats that
AnnoCAT does not currently consume.

## Compatibility effects

| Change | Existing OSA caches | Transcript cache | Result schema | Existing result files |
| --- | --- | --- | --- | --- |
| Consequence and HGVS corrections | No rebuild | No rebuild | Unchanged | Unchanged until reannotation |
| Source error reporting and failed-chunk memoization | No rebuild | No rebuild | Unchanged | Unchanged |
| Output and projection hot-path changes | No rebuild | No rebuild | Unchanged | Unchanged |
| Generic GFF term support from `f162f9f` | Unchanged | Would require a reviewed rebuild policy | Unchanged | Unchanged until reannotation |
| APPRIS support from `8c6b179` | Unchanged | Upstream requires `FSTVEP05` and a rebuild | Unchanged | Unchanged until reannotation |

The planned correctness work can change consequence sets, impact, amino-acid or
codon fields, and HGVS for affected variants. Variant and allele counts should
not change. Existing results remain readable and are not rewritten in place.

## Earlier upstream decisions

| Date | Upstream commit | Status | AnnoCAT decision |
| --- | --- | --- | --- |
| 2026-07-29 | [`22fdd45`](https://github.com/Huang-lab/fastVEP/commit/22fdd45b91b9e09de4f3e2049e5a9575af6f92c4) | Skipped | Manuscript figures, benchmark text, and upstream release automation are not AnnoCAT runtime inputs. |
| 2026-07-29 | [`d0c27e4`](https://github.com/Huang-lab/fastVEP/commit/d0c27e4aef218434c5e45761d1bc4b9bc461a069) | Skipped | Changes only the standalone fastVEP web interface. AnnoCAT supplies its own UI and version display. |
| 2026-08-03 | [`dde192c`](https://github.com/Huang-lab/fastVEP/commit/dde192c3921fabec019b4bba4f10e590d53969f0) | Implemented | Ported by fork commit `7b764e5`; the fork retains shared, thread-scaled, byte-bounded supplementary caches. |
| 2026-08-03 | [`e47216c`](https://github.com/Huang-lab/fastVEP/commit/e47216cebe3abcd8dff722b7fb0ab1b19d4fcc80) | Implemented | Fork commit `c412083` carries the applicable OSA/OSA2 integration suites and makes CI run the complete workspace tests. Bioconda and upstream documentation changes remain outside the AnnoCAT runtime. |
| 2026-08-10 | [`f41de3b`](https://github.com/Huang-lab/fastVEP/commit/f41de3b2693295ba3c8f76236faa3e01c0cb356e) | Implemented | Ported by fork commit `c443f50`; OSA2 startup parses the central directory sequentially, defers local-header reads, and opens providers in deterministic parallel order. |
| 2026-08-10 | [`915d082`](https://github.com/Huang-lab/fastVEP/commit/915d082c8726be68b33a9a423c21b95f82a0eeec) | Superseded | The canonical OSA2 migration was incorporated. AnnoCAT intentionally preserves all records for declared multi-record sources instead of retaining only the first duplicate key, and adds strict conversion parity and lossless ambiguous-allele storage. |
| 2026-08-13 | [`b1d4dba`](https://github.com/Huang-lab/fastVEP/commit/b1d4dba11ac6aa0c166e5cc2ff734ee879564cfc) | Partial | The relevant missing-allele, multi-base consequence, and splice corrections are present. The experimental fastVEP ACMG classifier, its benchmark stack, and configurable `--pick-order` are not AnnoCAT interfaces. |
| 2026-08-13 | [`3857047`](https://github.com/Huang-lab/fastVEP/commit/38570470099114571bcc6443c55da796e9a6989e) | Skipped | Upstream's repository toolchain pin is not copied. AnnoCAT verifies the fork commit, lockfile, build command, and packaged binary instead. |
| 2026-08-13 | [`6b95e91`](https://github.com/Huang-lab/fastVEP/commit/6b95e917bdfedff4b92d7b04a2e15174205a7911) | Skipped | Formatting-only change with no runtime behavior. |
| 2026-08-13 | [`9d4e695`](https://github.com/Huang-lab/fastVEP/commit/9d4e695efd89c65195cfbb2ba708d7fc868110f3) | Implemented | In-frame insertions are routed through sequence-aware delins handling in the fork's consequence and HGVS corrections. |
| 2026-08-18 | [`ab0822a`](https://github.com/Huang-lab/fastVEP/commit/ab0822a30b4c513c4715b272d7c902d842c0f00f) | Implemented | The fork applies strand-aware HGVS 3-prime normalization and contains additional complex-allele tests. |
| 2026-08-18 | [`5136931`](https://github.com/Huang-lab/fastVEP/commit/5136931a79a6f11aabf1882238281c621b6da762) | Implemented | Ported by `1134afe`: transcript caches publish atomically, explicit cache failures stop annotation, and region-restricted loads are not reused as complete caches. |
| 2026-08-18 | [`3a741b8`](https://github.com/Huang-lab/fastVEP/commit/3a741b85b662b549e1718beb448f5ac585e53088) | Implemented | Ported by `1134afe`: in-frame HGVSp anchoring and selenocysteine translation are retained. |
| 2026-08-20 | [`fd982f4`](https://github.com/Huang-lab/fastVEP/commit/fd982f44f3cc5ec1295c96c2d3d5bf830a4bb892) | Superseded | AnnoCAT accepts only its installed `FSTVEP02` cache with a matching manifest, checksum, provenance, structure report, and complete-transcript checks. It does not adopt upstream's standalone `FSTVEP03` rejection path. |
| 2026-08-20 | [`fa3a00f`](https://github.com/Huang-lab/fastVEP/commit/fa3a00f8584aaca72b0c13dffaa663ae225b5e7c) | Implemented | Ported by the current fork tip `a3fa8d8`; reverse-strand periodic peptide anchors are ordered by the affected span. |

## Current upstream decisions

| Date | Upstream commit | Status | AnnoCAT decision |
| --- | --- | --- | --- |
| 2026-08-26 | [`f162f9f`](https://github.com/Huang-lab/fastVEP/commit/f162f9fbe6e0ce3fc5d1d92557ca17cf639d37e1) | Implemented | Fork commit `c412083` carries the four schema-neutral hot-path optimizations. Generic provisional transcript terms, the unrelated pipeline split, and the `FSTVEP04` bump remain deferred. |
| 2026-08-27 | [`9266d04`](https://github.com/Huang-lab/fastVEP/commit/9266d045015210750f362f1261b6903f494f56d3) | Implemented | Fork commit `c412083` carries the applicable CDS-boundary behavior, supplementary lookup error accounting, and failed-chunk memoization. Existing fork-specific source builders and cache contracts remain unchanged. |
| 2026-08-28 | [`f778cff`](https://github.com/Huang-lab/fastVEP/commit/f778cff348cc99eb8e7ba1f421a7accd40090611) | Implemented | Fork commit `c412083` carries the applicable consequence, splice, HGVS, and sequence-derived delins behavior without importing ACMG benchmark artifacts or toolchain churn. |
| 2026-08-28 | [`01fad02`](https://github.com/Huang-lab/fastVEP/commit/01fad02589675430464c5ea062e7e73765ec64bb) | Implemented | Fork commit `c412083` carries intronic insertion and duplication normalization with bounded reference windows in both annotation paths. |
| 2026-08-28 | [`99b1275`](https://github.com/Huang-lab/fastVEP/commit/99b1275fb114ecdff4b1b824a4284aa5058f8cef) | Implemented | Fork commit `c412083` carries the frameshift terminator-distance regression test and its concise VEP-divergence rationale. |
| 2026-08-28 | [`123995f`](https://github.com/Huang-lab/fastVEP/commit/123995f104cd971363a757729228e8796d2a6a61) | Implemented | Fork commit `c412083` carries complete coding-term collection and its defensive checks. |
| 2026-08-28 | [`0a3c4b6`](https://github.com/Huang-lab/fastVEP/commit/0a3c4b6e39535a96944ae3603a9b41124aaa66cb) | Skipped | ACMG benchmark output only; AnnoCAT does not expose the fastVEP classifier. |
| 2026-08-31 | [`7af0aa2`](https://github.com/Huang-lab/fastVEP/commit/7af0aa2bd52f68c9cda96b10b1144c338edf631d) | Skipped | ACMG documentation and downloader URL maintenance. AnnoCAT owns source locations and contracts in `config/source-catalog.json`. |
| 2026-08-31 | [`917ed15`](https://github.com/Huang-lab/fastVEP/commit/917ed154e93f6befd779143134fa0388a6d12e55) | Skipped | Documents the standalone fastVEP REST API, which AnnoCAT does not run. |
| 2026-08-31 | [`e3d4c35`](https://github.com/Huang-lab/fastVEP/commit/e3d4c35f2da6b5e9875d6e61a4980e2bdb758dc1) | Deferred | Corrects `--pick` behavior in the standalone library/web path. AnnoCAT does not pass `--pick`; it retains transcript-level consequences and selects display evidence separately. |
| 2026-09-02 | [`8c6b179`](https://github.com/Huang-lab/fastVEP/commit/8c6b179f196a6dbfd60eb3773c1d7d69950db355) | Deferred | Parses GENCODE APPRIS tags for `--pick`. AnnoCAT currently uses Ensembl 115 GFF3, which does not provide those tags, and does not invoke `--pick`. Adopting this commit would require the upstream `FSTVEP05` cache format and a transcript-cache rebuild for no current result change. |

## Validation before changing the pin

The candidate at `78c870b` passed the complete fastVEP workspace suite, exact
comparison with archived Ensembl VEP 115 over 197 public variants and 3,417
consequence identities, direct-GFF versus transcript-cache byte parity, all
eight managed supplementary-source parity contracts, and AnnoCAT result
projection. An interleaved 103,800-record regression benchmark was 27% faster
than `a3fa8d8`, with identical output size. A packaged WGS smoke run remains a
release gate rather than a code-port gate.

The integration is complete only after all of these checks pass:

1. Run the fastVEP workspace tests and the imported upstream regressions.
2. Run `scripts/test-fastvep-pin.ps1` against the candidate fork revision.
3. Compare targeted real variants with Ensembl VEP 115 for CDS boundaries,
   splice-spanning changes, delins, intronic repeats, and multi-term coding
   consequences.
4. Run OSA1/OSA2 source parity for every managed supplementary source and prove
   that a forced reader error cannot become an ordinary missing value.
5. Run an AnnoCAT packaged end-to-end annotation and verify unchanged variant
   and allele counts, readable transcript arrays, and result-viewer behavior.
6. Benchmark annotation throughput against `a3fa8d8`; treat any material
   regression as unresolved before updating the release pin.
7. Update `config/fastvep-pin.json`, the binary identity, and this ledger only
   after the candidate passes the complete gate.

When upstream advances, append every new commit to the current decisions table,
even when the decision is to skip it. This prevents a documentation or
out-of-scope commit from being repeatedly reviewed as unknown work.
