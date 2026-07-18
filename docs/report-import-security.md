# Report import security boundary

AnnoCat report ZIPs are untrusted input. A checksum proves that an extracted file matches the
manifest; it does not prove that the report author is trustworthy. Import therefore uses format
validation and process isolation independently of checksums.

## Package validation

Before a report is visible to the library, the import worker must:

1. inspect the ZIP central directory without extracting it;
2. accept only safe top-level filenames and reject absolute paths, traversal, directories,
   symlinks/reparse points, duplicates (including case-only duplicates), excessive entry counts,
   and unsafe compression ratios;
3. require a versioned `annocat-manifest.json` that declares every entry and the four canonical
   roles (`variants`, `consequences`, `evidence`, and `field-catalog`);
4. stream every entry through a declared-size bound and SHA-256 verification;
5. extract with `create_new` into a new staging directory under the configured runs directory;
6. validate the actual fixed Parquet schemas and catalog structure; and
7. atomically rename staging into the library only after every check succeeds.

Catalog fields are descriptive data. Source IDs and field paths never become SQL identifiers.
Unknown catalog metadata is allowed, and unsupported evidence value types remain display-only.

## Windows worker containment

The Windows launcher uses a dedicated `annocat-report-worker.exe`; the UI, downloader, and DuckDB
query engine are not linked into this validation process. The release ZIP places the worker beside
`annocat.exe` and records its SHA-256 in the bundle manifest.

For each validation, the parent:

- opens the selected ZIP read-only and passes only that inherited handle, never its path;
- creates or opens a per-user AppContainer profile with zero capabilities (including no network);
- serializes launches, stages a fresh trusted worker in the Windows-managed profile, and removes it
  after the process exits;
- starts the worker suspended with an exact inherited-handle list and child-process creation
  disabled;
- creates it in a Job Object with one active process, a 1 GiB job-memory limit, and
  `KILL_ON_JOB_CLOSE`;
- verifies `TokenIsAppContainer` before resuming the first instruction; and
- fails closed on setup failure, crash, cancellation, or validation failure.

The user does not run `icacls`, approve an elevation prompt, or change permissions on the report,
AnnoCat folder, or resource directory. The AppContainer profile and worker staging are automatic
per-user application state. Starting suspended plus the process-creation job attribute closes the
race in which a child could run before containment.

The current command is validation-only. Canonical Parquet-schema validation, bounded extraction to
a staging directory, and atomic publication must be completed before imported files are exposed to
the results viewer. DuckDB import/query processes must separately disable external access and
extension loading and bound memory, threads, returned rows, and execution time.
