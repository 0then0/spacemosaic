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
On macOS, GUI builds use Xcode Command Line Tools to compile the small AppKit
icon bridge; core-only builds and other platforms do not compile it.

The icon source is `assets/icon.svg`; keep its checked-in 1024×1024 RGBA export,
`assets/icon.png`, the Windows `assets/icon.ico`, and Android adaptive vector
resources in sync when changing the artwork. Desktop packaging creates
all macOS ICNS resolutions from that PNG. After packaging on macOS, run
`python3 -m unittest discover -s tests -p 'test_*.py'` and check the native About
panel after a full application restart.

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

Before publishing dependency updates, refresh `THIRD_PARTY_NOTICES.txt` from the
locked crate sources and their upstream license files. The current inventory
covers the union of dependencies for the seven desktop/mobile target triples
used by release builds, conservatively including optional/build dependencies.
Preserve the separate bundled-font licenses. Use `hdiutil` on macOS and
`dpkg-deb` on Ubuntu to build the additional installer formats; no extra Cargo
packaging dependencies are needed. Windows setup uses Inno Setup 7.1.0,
bootstrapped by `scripts/setup-inno.ps1` with the official asset's pinned SHA-256.
For updates, check the latest stable upstream release, supported Windows versions,
license, and compiler options; update the version, URL, and checksum together.
Dependabot does not update this compiler pin automatically.

Only maintainers publish releases. Package artifacts with the supplied scripts,
verify SHA-256 manifests, and document signing/notarization status. Never commit
certificates, provisioning profiles, keystores, or credentials. Publishing a
release is separate from pushing source changes.
