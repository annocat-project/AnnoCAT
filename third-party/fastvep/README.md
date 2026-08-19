# Bundled fastVEP

The Windows release bundles `fastvep.exe` built from the repository, commit,
dependency lock, and license recorded in
[`config/fastvep-pin.json`](../../config/fastvep-pin.json).

The executable remains a separate release artifact and is invoked as AnnoCAT's
annotation engine. Its Apache-2.0 license is included in this directory and in
the packaged release.

Packaging verifies the exact source history and lockfile, runs the locked test
suite, builds the release binary, and checks its recorded size and SHA-256. The
maintenance process is documented in
[`docs/fastvep-maintenance.md`](../../docs/fastvep-maintenance.md).
