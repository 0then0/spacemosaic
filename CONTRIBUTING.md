# Contributing

Use English for issues, pull requests, documentation, code comments, and commit
messages. Interface translations belong in the seven bundled JSON catalogs.

## Development environment

Install the toolchain specified in `rust-toolchain.toml`. Use the committed
lockfile and avoid updating unrelated dependencies.

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
```

The optional `gui` feature is enabled by default. Core storage and geometry
tests also run with `cargo test --locked --no-default-features`.
Mobile prerequisites and build commands are documented in the README.

## Change requirements

Preserve read-only behavior, permission boundaries, unknown metadata, and
session isolation. Do not convert content URIs into filesystem paths. Do not
follow symbolic links or reparse points implicitly. New dependencies need a
specific purpose and maintainer approval.

Use disposable test directories. Never scan a home directory, a production
volume, or a shared storage provider as part of an automated test. Add tests
for observable behavior, particularly partial errors, cancellation, aggregation,
export, and layout hit testing. See [testing](docs/testing.md).

All interface strings must have nonempty entries in `en`, `ru`, `es`, `de`,
`fr`, `ja`, and `pt-BR`. Keep machine-readable keys, paths, and export numbers
independent of display language. Check long labels and large text in both themes.

Use Conventional Commits in English: `feat`, `fix`, `refactor`, `chore`, `docs`,
 or `test`. Explain the user-visible result and actual validation in each pull
request. Distinguish build success from simulator launch and device testing.

## Dependencies

Update the egui family coherently and verify that `cargo tree -i egui` resolves
a single version. Fastframe is pinned to an immutable commit; follow the
[dependency update process](docs/architecture.md#dependency-updates).
Dependabot checks Cargo, GitHub Actions, and Android Gradle dependencies monthly.
Major upgrades are reviewed separately. Security updates remain enabled.

## Releases

Only maintainers publish releases. Package artifacts with the supplied scripts,
verify SHA-256 manifests, and document signing/notarization status. Never commit
certificates, provisioning profiles, keystores, or credentials. Publishing a
release is separate from pushing source changes.
