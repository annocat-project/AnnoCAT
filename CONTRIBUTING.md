# Contributing to AnnoCAT

AnnoCAT accepts focused bug fixes, tests, documentation corrections, and
reviewed feature changes. Open an issue before starting a large change.

## Build and test

Install a stable Rust toolchain, then run:

```text
cargo test --workspace
cargo run -p annocat-cli -- launch
```

The frontend is bundled into the Rust executable. When changing the viewer,
run its JavaScript tests as well:

```text
node --test web/tests/*.test.mjs
```

## Data rules

- Use fabricated or approved public fixtures only.
- Do not commit personal genomic data, application results, annotation caches,
  credentials, machine-specific paths, or generated build output.
- Record the source release and expected values for annotation fixtures.
- Keep scientific expectations independent of the code under test.

## fastVEP changes

AnnoCAT builds the exact fastVEP revision in
[`config/fastvep-pin.json`](config/fastvep-pin.json). Do not update the bundled
engine by changing a binary alone. Follow
[`docs/FASTVEP_FORK.md`](docs/FASTVEP_FORK.md) and update the pin, tests, and
artifact identity together.

## Pull requests

Keep each change narrow. Include the reason for the change, the tests run, and
any compatibility or data-contract effect. Do not mix generated artifacts or
unrelated formatting changes into the same pull request.
