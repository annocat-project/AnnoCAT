# Result import security

AnnoCAT treats every result ZIP as untrusted input. A matching checksum proves
only that a file matches its manifest; it does not establish trust in the
package author or annotation content.

## Package validation

Before an imported result becomes visible, AnnoCAT:

1. reads the ZIP directory without extracting files;
2. rejects absolute names, traversal, links, duplicate names, unsafe entry
   counts, and unsafe compression ratios;
3. requires a versioned `annocat-manifest.json` and the canonical result roles;
4. streams each declared entry through size and SHA-256 checks;
5. extracts into a new staging directory;
6. validates Parquet schemas, catalogs, provenance, and declared roles; and
7. publishes the result with an atomic rename only after every check passes.

Catalog labels and field paths are data. They do not become SQL identifiers.
Unsupported value types remain display-only.

## Windows worker boundary

The packaged Windows build validates imports in
`annocat-report-worker.exe`. The worker receives a read-only inherited handle
instead of the selected path and runs in a restricted AppContainer with no
network capability. The parent also applies a single-process Job Object,
memory limits, child-process blocking, and fail-closed setup checks.

Worker failure, cancellation, or containment failure prevents publication.
DuckDB query processes separately disable external access and extension
loading and enforce bounded resources and returned rows.

Format validation does not prove that annotations are correct. Scientific
fidelity is covered by the
[annotation validation checks](annotation-validation.md).
