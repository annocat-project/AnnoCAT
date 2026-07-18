# Bundled fastVEP

AnnoCAT's Windows release bundles the native `fastvep.exe` built from the repository,
commit, dependency lock, and license recorded in `config/fastvep-pin.json`.

The executable is kept separate under `tools/fastvep` and invoked as AnnoCAT's
annotation engine. The upstream Apache-2.0 license is included unchanged in this
directory and copied into each Windows bundle.

AnnoCAT's fastVEP changes are maintained as ordinary commits on the public
`annocat/v0.1` branch of `annocat-project/fastVEP`. The exact fork commit,
upstream base, ordered change commits, Cargo lockfile, and release binary are
pinned in `config/fastvep-pin.json`; the Windows packaging script verifies that
history before running the complete locked fastVEP test suite and release build.
